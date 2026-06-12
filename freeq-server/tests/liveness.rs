//! Liveness-probe eviction tests.
//!
//! When a new session authenticates with a DID that already has sessions,
//! the server PINGs the existing sessions. Sessions that don't answer within
//! the probe deadline (zombie sockets left by frozen/resumed agent VMs) are
//! evicted in seconds instead of waiting out the ~90s ping timeout. Healthy
//! multi-device siblings answer the PING and are untouched.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::{Duration, Instant};

use freeq_sdk::auth::{self, ChallengeSigner, KeySigner};
use freeq_sdk::crypto::PrivateKey;
use freeq_sdk::did::{self, DidResolver};

const DID: &str = "did:plc:liveness_agent";

fn resolver(key: &PrivateKey) -> DidResolver {
    let mut docs = HashMap::new();
    docs.insert(
        DID.to_string(),
        did::make_test_did_document(DID, &key.public_key_multibase()),
    );
    DidResolver::static_map(docs)
}

async fn start(r: DidResolver) -> (SocketAddr, tokio::task::JoinHandle<anyhow::Result<()>>) {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let db = tmp.path().to_str().unwrap().to_string();
    std::mem::forget(tmp);
    let config = freeq_server::config::ServerConfig {
        listen_addr: "127.0.0.1:0".to_string(),
        server_name: "test-liveness".to_string(),
        challenge_timeout_secs: 60,
        db_path: Some(db),
        ..Default::default()
    };
    freeq_server::server::Server::with_resolver(config, r)
        .start()
        .await
        .unwrap()
}

async fn run(addr: SocketAddr, f: impl FnOnce(SocketAddr) + Send + 'static) {
    tokio::task::spawn_blocking(move || f(addr)).await.unwrap();
}

struct C {
    reader: BufReader<TcpStream>,
    writer: TcpStream,
}
impl C {
    fn with_sasl(addr: SocketAddr, nick: &str, key: PrivateKey) -> Self {
        let s = TcpStream::connect(addr).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(5))).ok();
        let w = s.try_clone().unwrap();
        let mut c = Self { reader: BufReader::new(s), writer: w };
        c.tx("CAP LS 302");
        c.tx(&format!("NICK {nick}"));
        c.tx(&format!("USER {nick} 0 * :test"));
        c.tx("CAP REQ :sasl");
        c.rx(|l| l.contains("ACK"), "ACK");
        c.tx("AUTHENTICATE ATPROTO-CHALLENGE");
        let ch = c.rx(|l| l.starts_with("AUTHENTICATE "), "challenge");
        let bytes =
            auth::decode_challenge_bytes(ch.strip_prefix("AUTHENTICATE ").unwrap()).unwrap();
        let signer = KeySigner::new(DID.to_string(), key);
        let resp = signer.respond(&bytes).unwrap();
        c.tx(&format!("AUTHENTICATE {}", auth::encode_response(&resp)));
        c.rx(|l| l.split_whitespace().nth(1) == Some("903"), "903");
        c.tx("CAP END");
        c.rx(|l| l.split_whitespace().nth(1) == Some("001"), "001");
        c
    }
    fn tx(&mut self, l: &str) {
        writeln!(self.writer, "{l}\r").unwrap();
        self.writer.flush().ok();
    }
    /// Read lines (auto-answering PINGs) until the predicate matches.
    fn rx(&mut self, p: impl Fn(&str) -> bool, d: &str) -> String {
        let mut b = String::new();
        loop {
            b.clear();
            match self.reader.read_line(&mut b) {
                Ok(0) => panic!("EOF: {d}"),
                Ok(_) => {
                    let l = b.trim_end();
                    if l.starts_with("PING") {
                        let t = l.strip_prefix("PING ").unwrap_or(":x");
                        let _ = writeln!(self.writer, "PONG {t}\r");
                        let _ = self.writer.flush();
                        continue;
                    }
                    if p(l) {
                        return l.to_string();
                    }
                }
                Err(e)
                    if e.kind() == std::io::ErrorKind::TimedOut
                        || e.kind() == std::io::ErrorKind::WouldBlock =>
                {
                    panic!("Timeout: {d}")
                }
                Err(e) => panic!("{d}: {e}"),
            }
        }
    }
    /// Keep the connection healthy (reading + answering PINGs) for `dur`.
    fn stay_alive(&mut self, dur: Duration) {
        let deadline = Instant::now() + dur;
        self.writer
            .try_clone()
            .unwrap()
            .set_read_timeout(Some(Duration::from_millis(250)))
            .ok();
        let mut b = String::new();
        while Instant::now() < deadline {
            b.clear();
            match self.reader.read_line(&mut b) {
                Ok(0) => panic!("healthy sibling was disconnected"),
                Ok(_) => {
                    if b.starts_with("PING") {
                        let t = b.trim_end().strip_prefix("PING ").unwrap_or(":x");
                        let _ = writeln!(self.writer, "PONG {t}\r");
                        let _ = self.writer.flush();
                    }
                }
                Err(_) => {} // read timeout — keep waiting
            }
        }
        self.writer
            .try_clone()
            .unwrap()
            .set_read_timeout(Some(Duration::from_secs(5)))
            .ok();
    }
    /// Wait for the server to close this connection. Returns the elapsed
    /// time. Panics if still open after `max`. Does NOT answer PINGs —
    /// this simulates a frozen process holding a zombie socket.
    fn wait_for_eviction(&mut self, max: Duration) -> Duration {
        let start = Instant::now();
        self.writer
            .try_clone()
            .unwrap()
            .set_read_timeout(Some(Duration::from_millis(500)))
            .ok();
        let mut b = String::new();
        while start.elapsed() < max {
            b.clear();
            match self.reader.read_line(&mut b) {
                Ok(0) => return start.elapsed(), // server closed us
                Ok(_) => {}                      // swallow lines, never PONG
                Err(_) => {}                     // read timeout — keep waiting
            }
        }
        panic!("zombie session not evicted within {max:?}");
    }
}

#[tokio::test]
async fn zombie_same_did_session_is_evicted_quickly() {
    let key = PrivateKey::generate_ed25519();
    let k1 = PrivateKey::ed25519_from_bytes(&key.secret_bytes()).unwrap();
    let k2 = PrivateKey::ed25519_from_bytes(&key.secret_bytes()).unwrap();
    let (addr, _h) = start(resolver(&key)).await;
    run(addr, move |addr| {
        let mut zombie = C::with_sasl(addr, "agent", k1);
        zombie.tx("JOIN #wake");
        zombie.rx(|l| l.contains("JOIN"), "join echo");
        // zombie now goes silent — frozen VM. It will not answer PINGs.

        let mut fresh = C::with_sasl(addr, "agent", k2);
        // The new session attaches (multi-device) and triggers the probe.
        // The zombie must be evicted in ~LIVENESS_PROBE_SECS (10s), well
        // under the ~90s ping timeout.
        let elapsed = zombie.wait_for_eviction(Duration::from_secs(25));
        assert!(
            elapsed < Duration::from_secs(20),
            "eviction took {elapsed:?}, expected ~10s probe deadline"
        );

        // The fresh session is unaffected and fully functional.
        fresh.tx("WHOIS agent");
        fresh.rx(|l| l.split_whitespace().nth(1) == Some("318"), "end of WHOIS");
    })
    .await;
}

#[tokio::test]
async fn healthy_sibling_survives_probe() {
    let key = PrivateKey::generate_ed25519();
    let k1 = PrivateKey::ed25519_from_bytes(&key.secret_bytes()).unwrap();
    let k2 = PrivateKey::ed25519_from_bytes(&key.secret_bytes()).unwrap();
    let (addr, _h) = start(resolver(&key)).await;
    run(addr, move |addr| {
        let mut phone = C::with_sasl(addr, "agent", k1);
        let mut laptop = C::with_sasl(addr, "agent", k2);
        // phone answers the liveness PING (stay_alive auto-PONGs) across
        // the probe deadline and must NOT be evicted.
        phone.stay_alive(Duration::from_secs(13));

        // Both sessions still work.
        phone.tx("WHOIS agent");
        phone.rx(|l| l.split_whitespace().nth(1) == Some("318"), "phone WHOIS");
        laptop.tx("WHOIS agent");
        laptop.rx(|l| l.split_whitespace().nth(1) == Some("318"), "laptop WHOIS");
    })
    .await;
}
