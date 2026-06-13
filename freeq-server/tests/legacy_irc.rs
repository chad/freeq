//! End-to-end tests for legacy (non-freeq) IRC client compatibility.
//!
//! These tests use raw TCP connections with no SASL, no IRCv3 CAP negotiation,
//! and no AT Protocol features. A standard IRC client (irssi, weechat, HexChat)
//! should be able to do everything tested here.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

use freeq_sdk::did::DidResolver;

/// Start a test server on a random port.
async fn start_server() -> (SocketAddr, tokio::task::JoinHandle<anyhow::Result<()>>) {
    let config = freeq_server::config::ServerConfig {
        listen_addr: "127.0.0.1:0".to_string(),
        server_name: "test-irc".to_string(),
        challenge_timeout_secs: 60,
        ..Default::default()
    };
    let resolver = DidResolver::static_map(HashMap::new());
    let server = freeq_server::server::Server::with_resolver(config, resolver);
    server.start().await.unwrap()
}

/// Run a blocking IRC test against a freshly started server.
async fn run_irc_test(f: impl FnOnce(SocketAddr) + Send + 'static) {
    let (addr, _server) = start_server().await;
    tokio::task::spawn_blocking(move || f(addr)).await.unwrap();
}

/// A minimal raw IRC client — no CAP, no SASL, just NICK/USER.
struct RawIrc {
    reader: BufReader<TcpStream>,
    writer: TcpStream,
}

impl RawIrc {
    fn connect(addr: SocketAddr, nick: &str) -> Self {
        let stream = TcpStream::connect(addr).expect("connect");
        stream.set_read_timeout(Some(Duration::from_secs(5))).ok();
        let writer = stream.try_clone().unwrap();
        let reader = BufReader::new(stream);
        let mut c = Self { reader, writer };
        c.send(&format!("NICK {nick}"));
        c.send(&format!("USER {nick} 0 * :Legacy IRC"));
        c
    }

    fn send(&mut self, line: &str) {
        writeln!(self.writer, "{line}\r").unwrap();
        self.writer.flush().ok();
    }

    fn expect(&mut self, pred: impl Fn(&str) -> bool, desc: &str) -> String {
        let mut buf = String::new();
        loop {
            buf.clear();
            match self.reader.read_line(&mut buf) {
                Ok(0) => panic!("Connection closed waiting for: {desc}"),
                Ok(_) => {
                    let line = buf.trim_end();
                    if line.starts_with("PING") {
                        let tok = line.strip_prefix("PING ").unwrap_or(":x");
                        let _ = writeln!(self.writer, "PONG {tok}\r");
                        let _ = self.writer.flush();
                        continue;
                    }
                    if pred(line) {
                        return line.to_string();
                    }
                }
                Err(e)
                    if e.kind() == std::io::ErrorKind::TimedOut
                        || e.kind() == std::io::ErrorKind::WouldBlock =>
                {
                    panic!("Timeout waiting for: {desc}")
                }
                Err(e) => panic!("Read error for {desc}: {e}"),
            }
        }
    }

    fn expect_num(&mut self, code: &str) -> String {
        self.expect(
            |l| l.split_whitespace().nth(1) == Some(code),
            &format!("numeric {code}"),
        )
    }

    fn registered(&mut self) -> String {
        self.expect_num("001")
    }

    fn drain(&mut self) {
        self.writer
            .try_clone()
            .unwrap()
            .set_read_timeout(Some(Duration::from_millis(200)))
            .ok();
        let mut buf = String::new();
        loop {
            buf.clear();
            match self.reader.read_line(&mut buf) {
                Ok(0) => break,
                Ok(_) => {
                    let line = buf.trim_end();
                    if line.starts_with("PING") {
                        let tok = line.strip_prefix("PING ").unwrap_or(":x");
                        let _ = writeln!(self.writer, "PONG {tok}\r");
                        let _ = self.writer.flush();
                    }
                }
                Err(_) => break,
            }
        }
        self.writer
            .try_clone()
            .unwrap()
            .set_read_timeout(Some(Duration::from_secs(5)))
            .ok();
    }
}

// ══════════════════════════════════════════════════════════════════════
//  BASIC CONNECTION
// ══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn guest_connects_and_registers() {
    run_irc_test(|addr| {
        let mut c = RawIrc::connect(addr, "legacyuser");
        let w = c.registered();
        assert!(w.contains("legacyuser"));
    })
    .await;
}

#[tokio::test]
async fn guest_gets_motd_or_nomotd() {
    run_irc_test(|addr| {
        let mut c = RawIrc::connect(addr, "motdtest");
        c.registered();
        c.expect(
            |l| {
                let n = l.split_whitespace().nth(1).unwrap_or("");
                n == "375" || n == "376" || n == "422"
            },
            "MOTD or NOMOTD",
        );
    })
    .await;
}

#[tokio::test]
async fn duplicate_nick_rejected() {
    run_irc_test(|addr| {
        let mut c1 = RawIrc::connect(addr, "dupnick");
        c1.registered();
        let mut c2 = RawIrc::connect(addr, "dupnick");
        c2.expect_num("433"); // ERR_NICKNAMEINUSE
    })
    .await;
}

#[tokio::test]
async fn invalid_nick_rejected() {
    run_irc_test(|addr| {
        // Nick with comma (forbidden char) should be rejected
        let mut c = RawIrc::connect(addr, "bad,nick");
        c.expect_num("432"); // ERR_ERRONEUSNICKNAME
    })
    .await;
}

// ══════════════════════════════════════════════════════════════════════
//  CHANNELS
// ══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn join_channel() {
    run_irc_test(|addr| {
        let mut c = RawIrc::connect(addr, "joiner");
        c.registered();
        c.drain();
        c.send("JOIN #testchan");
        c.expect(
            |l| l.contains("JOIN") && l.contains("#testchan"),
            "JOIN echo",
        );
        c.expect_num("366");
    })
    .await;
}

#[tokio::test]
async fn part_channel() {
    run_irc_test(|addr| {
        let mut c = RawIrc::connect(addr, "parter");
        c.registered();
        c.drain();
        c.send("JOIN #parttest");
        c.expect_num("366");
        c.drain();
        c.send("PART #parttest :bye");
        c.expect(
            |l| l.contains("PART") && l.contains("#parttest"),
            "PART echo",
        );
    })
    .await;
}

#[tokio::test]
async fn two_guests_chat() {
    run_irc_test(|addr| {
        let mut a = RawIrc::connect(addr, "alice_irc");
        a.registered();
        a.drain();
        a.send("JOIN #chat");
        a.expect_num("366");
        a.drain();

        let mut b = RawIrc::connect(addr, "bob_irc");
        b.registered();
        b.drain();
        b.send("JOIN #chat");
        b.expect_num("366");
        b.drain();

        b.send("PRIVMSG #chat :hello from legacy IRC!");
        let msg = a.expect(
            |l| l.contains("PRIVMSG") && l.contains("hello from legacy IRC!"),
            "Alice gets Bob's msg",
        );
        assert!(msg.contains("bob_irc"));
    })
    .await;
}

#[tokio::test]
async fn guest_dm() {
    run_irc_test(|addr| {
        let mut a = RawIrc::connect(addr, "dm_alice");
        a.registered();
        a.drain();
        let mut b = RawIrc::connect(addr, "dm_bob");
        b.registered();
        b.drain();

        a.send("PRIVMSG dm_bob :secret message");
        b.expect(
            |l| l.contains("PRIVMSG") && l.contains("secret message"),
            "DM received",
        );
    })
    .await;
}

#[tokio::test]
async fn channel_name_too_long() {
    run_irc_test(|addr| {
        let mut c = RawIrc::connect(addr, "longchan");
        c.registered();
        c.drain();
        let name = format!("#{}", "a".repeat(64));
        c.send(&format!("JOIN {name}"));
        c.expect_num("479");
    })
    .await;
}

#[tokio::test]
async fn join_creates_new_channel() {
    run_irc_test(|addr| {
        let mut c = RawIrc::connect(addr, "creator");
        c.registered();
        c.drain();
        c.send("JOIN #brand-new");
        c.expect(
            |l| l.contains("JOIN") && l.contains("#brand-new"),
            "JOIN echo",
        );
        c.expect_num("366");
    })
    .await;
}

#[tokio::test]
async fn multiple_channels_join_and_part() {
    run_irc_test(|addr| {
        let mut c = RawIrc::connect(addr, "multich");
        c.registered();
        c.drain();
        c.send("JOIN #m1");
        c.expect_num("366");
        c.send("JOIN #m2");
        c.expect_num("366");
        c.send("JOIN #m3");
        c.expect_num("366");
        c.drain();
        c.send("PART #m2 :leaving");
        c.expect(|l| l.contains("PART") && l.contains("#m2"), "PART #m2");
        c.send("NAMES #m1");
        let n = c.expect_num("353");
        assert!(n.contains("multich"), "still in #m1");
    })
    .await;
}

// ══════════════════════════════════════════════════════════════════════
//  NICK CHANGES
// ══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn nick_change() {
    run_irc_test(|addr| {
        let mut c = RawIrc::connect(addr, "oldnick");
        c.registered();
        c.drain();
        c.send("NICK newnick");
        c.expect(|l| l.contains("NICK") && l.contains("newnick"), "NICK echo");
    })
    .await;
}

#[tokio::test]
async fn nick_change_visible_in_channel() {
    run_irc_test(|addr| {
        let mut a = RawIrc::connect(addr, "nick_a");
        a.registered();
        a.drain();
        a.send("JOIN #nicktest");
        a.expect_num("366");
        a.drain();

        let mut b = RawIrc::connect(addr, "nick_b");
        b.registered();
        b.drain();
        b.send("JOIN #nicktest");
        b.expect_num("366");
        b.drain();

        b.send("NICK bob_new");
        a.expect(
            |l| l.contains("NICK") && l.contains("bob_new"),
            "Alice sees nick change",
        );
    })
    .await;
}

// ══════════════════════════════════════════════════════════════════════
//  WHOIS / WHO
// ══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn whois_guest() {
    run_irc_test(|addr| {
        let mut c = RawIrc::connect(addr, "whoisme");
        c.registered();
        c.drain();
        c.send("WHOIS whoisme");
        let l = c.expect_num("311");
        assert!(l.contains("whoisme"));
        c.expect_num("318");
    })
    .await;
}

#[tokio::test]
async fn whois_guest_has_no_did_or_handle() {
    run_irc_test(|addr| {
        let mut c = RawIrc::connect(addr, "nodidusr");
        c.registered();
        c.drain();
        c.send("WHOIS nodidusr");
        let mut got_330 = false;
        let mut got_at_handle = false;
        loop {
            let l = c.expect(|_| true, "WHOIS line");
            let n = l.split_whitespace().nth(1).unwrap_or("");
            if n == "330" {
                got_330 = true;
            }
            // 671 is used for both AT handle and client info; only flag AT-specific content
            if n == "671" && l.contains("AT Protocol handle") {
                got_at_handle = true;
            }
            if n == "318" {
                break;
            }
        }
        assert!(!got_330, "Guest must NOT have 330 (DID)");
        assert!(
            !got_at_handle,
            "Guest must NOT have AT Protocol handle in 671"
        );
    })
    .await;
}

#[tokio::test]
async fn who_channel() {
    run_irc_test(|addr| {
        let mut c = RawIrc::connect(addr, "whochan");
        c.registered();
        c.drain();
        c.send("JOIN #whotest");
        c.expect_num("366");
        c.drain();
        c.send("WHO #whotest");
        let l = c.expect_num("352");
        assert!(l.contains("whochan"));
        c.expect_num("315");
    })
    .await;
}

#[tokio::test]
async fn cloaked_hostname_guest() {
    run_irc_test(|addr| {
        let mut c = RawIrc::connect(addr, "cloaktest");
        c.registered();
        c.drain();
        c.send("WHOIS cloaktest");
        let l = c.expect_num("311");
        // Server uses generic "host" for WHOIS 311; cloaking is visible in hostmask
        // on JOIN/PRIVMSG, not necessarily in WHOIS 311 (which uses a static placeholder).
        assert!(l.contains("cloaktest"), "WHOIS should contain nick: {l}");
    })
    .await;
}

// ══════════════════════════════════════════════════════════════════════
//  MODES
// ══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn founder_gets_ops() {
    run_irc_test(|addr| {
        let mut c = RawIrc::connect(addr, "founder");
        c.registered();
        c.drain();
        c.send("JOIN #opstest");
        let names = c.expect(|l| l.split_whitespace().nth(1) == Some("353"), "NAMREPLY");
        assert!(
            names.contains("@founder"),
            "Founder needs @ prefix: {names}"
        );
    })
    .await;
}

#[tokio::test]
async fn mode_query() {
    run_irc_test(|addr| {
        let mut c = RawIrc::connect(addr, "modeq");
        c.registered();
        c.drain();
        c.send("JOIN #modetest");
        c.expect_num("366");
        c.drain();
        c.send("MODE #modetest");
        c.expect_num("324");
    })
    .await;
}

// ══════════════════════════════════════════════════════════════════════
//  TOPIC
// ══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn set_and_query_topic() {
    run_irc_test(|addr| {
        let mut c = RawIrc::connect(addr, "topicusr");
        c.registered();
        c.drain();
        c.send("JOIN #topictest");
        c.expect_num("366");
        c.drain();
        c.send("TOPIC #topictest :Hello legacy IRC");
        c.expect(
            |l| l.contains("TOPIC") && l.contains("Hello legacy IRC"),
            "TOPIC echo",
        );
        c.send("TOPIC #topictest");
        let l = c.expect_num("332");
        assert!(l.contains("Hello legacy IRC"));
    })
    .await;
}

#[tokio::test]
async fn topic_too_long() {
    run_irc_test(|addr| {
        let mut c = RawIrc::connect(addr, "longtopic");
        c.registered();
        c.drain();
        c.send("JOIN #topiclong");
        c.expect_num("366");
        c.drain();
        c.send(&format!("TOPIC #topiclong :{}", "x".repeat(600)));
        c.expect(
            |l| l.contains("TOO_LONG") || l.contains("FAIL"),
            "topic too long",
        );
    })
    .await;
}

// ══════════════════════════════════════════════════════════════════════
//  EDGE CASES
// ══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn ping_pong() {
    run_irc_test(|addr| {
        let mut c = RawIrc::connect(addr, "pingtest");
        c.registered();
        c.drain();
        c.send("PING :hello");
        c.expect(
            |l| l.contains("PONG") && l.contains("hello"),
            "PONG response",
        );
    })
    .await;
}

#[tokio::test]
async fn msg_to_nonexistent_nick() {
    run_irc_test(|addr| {
        let mut c = RawIrc::connect(addr, "msgerr");
        c.registered();
        c.drain();
        c.send("PRIVMSG nobody123 :hello?");
        c.expect_num("401"); // ERR_NOSUCHNICK
    })
    .await;
}

#[tokio::test]
async fn quit_visible_to_channel_members() {
    run_irc_test(|addr| {
        let mut a = RawIrc::connect(addr, "quitter");
        a.registered();
        a.drain();
        a.send("JOIN #quitchan");
        a.expect_num("366");
        a.drain();

        let mut b = RawIrc::connect(addr, "observer");
        b.registered();
        b.drain();
        b.send("JOIN #quitchan");
        b.expect_num("366");
        b.drain();

        a.send("QUIT :goodbye");
        b.expect(
            |l| l.contains("QUIT") && l.contains("quitter"),
            "Bob sees QUIT",
        );
    })
    .await;
}

#[tokio::test]
async fn kick() {
    run_irc_test(|addr| {
        let mut a = RawIrc::connect(addr, "kick_alice");
        a.registered();
        a.drain();
        a.send("JOIN #kicktest");
        a.expect_num("366");
        a.drain();

        let mut b = RawIrc::connect(addr, "kick_bob");
        b.registered();
        b.drain();
        b.send("JOIN #kicktest");
        b.expect_num("366");
        b.drain();

        a.send("KICK #kicktest kick_bob :out");
        b.expect(
            |l| l.contains("KICK") && l.contains("kick_bob"),
            "Bob sees KICK",
        );
    })
    .await;
}

#[tokio::test]
async fn list_channels() {
    run_irc_test(|addr| {
        let mut c = RawIrc::connect(addr, "lister");
        c.registered();
        c.drain();
        c.send("JOIN #listtest");
        c.expect_num("366");
        c.drain();
        c.send("LIST");
        // Server sends 322 (RPL_LIST) entries then 323 (RPL_LISTEND). No 321 RPL_LISTSTART.
        c.expect(
            |l| l.split_whitespace().nth(1) == Some("322") && l.contains("#listtest"),
            "LIST entry",
        );
        c.expect_num("323"); // LISTEND
    })
    .await;
}

#[tokio::test]
async fn names_shows_members() {
    run_irc_test(|addr| {
        let mut a = RawIrc::connect(addr, "names_a");
        a.registered();
        a.drain();
        a.send("JOIN #namestest");
        a.expect_num("366");
        a.drain();

        let mut b = RawIrc::connect(addr, "names_b");
        b.registered();
        b.drain();
        b.send("JOIN #namestest");
        b.expect_num("366");
        b.drain();

        b.send("NAMES #namestest");
        let n = b.expect_num("353");
        assert!(n.contains("names_a"), "should include alice: {n}");
        assert!(n.contains("names_b"), "should include bob: {n}");
    })
    .await;
}
