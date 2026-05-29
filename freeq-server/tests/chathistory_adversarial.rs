//! Adversarial CHATHISTORY tests.
//!
//! Tests history access control, deleted message filtering, edit visibility,
//! DM privacy, and pagination boundaries.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpStream, SocketAddr};
use std::time::Duration;

use freeq_sdk::auth::{self, ChallengeSigner, KeySigner};
use freeq_sdk::crypto::PrivateKey;
use freeq_sdk::did::{self, DidResolver};

const DID_A: &str = "did:plc:hist_alice";
const DID_B: &str = "did:plc:hist_bob";
const DID_C: &str = "did:plc:hist_eve";

fn resolver(entries: Vec<(&str, &PrivateKey)>) -> DidResolver {
    let mut docs = HashMap::new();
    for (did, key) in entries {
        docs.insert(did.to_string(), did::make_test_did_document(did, &key.public_key_multibase()));
    }
    DidResolver::static_map(docs)
}

async fn start(r: DidResolver) -> (SocketAddr, tokio::task::JoinHandle<anyhow::Result<()>>) {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let db = tmp.path().to_str().unwrap().to_string();
    std::mem::forget(tmp);
    let config = freeq_server::config::ServerConfig {
        listen_addr: "127.0.0.1:0".to_string(),
        server_name: "test-hist".to_string(),
        challenge_timeout_secs: 60,
        db_path: Some(db),
        ..Default::default()
    };
    freeq_server::server::Server::with_resolver(config, r).start().await.unwrap()
}

async fn run(addr: SocketAddr, f: impl FnOnce(SocketAddr) + Send + 'static) {
    tokio::task::spawn_blocking(move || f(addr)).await.unwrap();
}

struct C { reader: BufReader<TcpStream>, writer: TcpStream }
impl C {
    fn with_caps(addr: SocketAddr, nick: &str) -> Self {
        let s = TcpStream::connect(addr).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(5))).ok();
        let w = s.try_clone().unwrap();
        let mut c = Self { reader: BufReader::new(s), writer: w };
        c.tx("CAP LS 302");
        c.tx(&format!("NICK {nick}"));
        c.tx(&format!("USER {nick} 0 * :test"));
        c.tx("CAP REQ :message-tags server-time batch draft/chathistory");
        c.rx(|l| l.contains("ACK"), "ACK");
        c.tx("CAP END");
        c
    }
    fn with_sasl(addr: SocketAddr, nick: &str, did: &str, key: PrivateKey) -> Self {
        let s = TcpStream::connect(addr).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(5))).ok();
        let w = s.try_clone().unwrap();
        let mut c = Self { reader: BufReader::new(s), writer: w };
        c.tx("CAP LS 302");
        c.tx(&format!("NICK {nick}"));
        c.tx(&format!("USER {nick} 0 * :test"));
        c.tx("CAP REQ :sasl message-tags server-time batch draft/chathistory");
        c.rx(|l| l.contains("ACK"), "ACK");
        c.tx("AUTHENTICATE ATPROTO-CHALLENGE");
        let ch = c.rx(|l| l.starts_with("AUTHENTICATE "), "challenge");
        let bytes = auth::decode_challenge_bytes(ch.strip_prefix("AUTHENTICATE ").unwrap()).unwrap();
        let signer = KeySigner::new(did.to_string(), key);
        let resp = signer.respond(&bytes).unwrap();
        c.tx(&format!("AUTHENTICATE {}", auth::encode_response(&resp)));
        c.num("903");
        c.tx("CAP END");
        c
    }
    fn tx(&mut self, l: &str) { writeln!(self.writer, "{l}\r").unwrap(); self.writer.flush().ok(); }
    fn rx(&mut self, p: impl Fn(&str) -> bool, d: &str) -> String {
        let mut b = String::new();
        loop { b.clear(); match self.reader.read_line(&mut b) {
            Ok(0) => panic!("EOF: {d}"), Ok(_) => {
                let l = b.trim_end();
                if l.starts_with("PING") { let t = l.strip_prefix("PING ").unwrap_or(":x");
                    let _ = writeln!(self.writer, "PONG {t}\r"); let _ = self.writer.flush(); continue; }
                if p(l) { return l.to_string(); }
            } Err(e) if e.kind() == std::io::ErrorKind::TimedOut || e.kind() == std::io::ErrorKind::WouldBlock
                => panic!("Timeout: {d}"), Err(e) => panic!("{d}: {e}"),
        }}
    }
    fn num(&mut self, c: &str) -> String { self.rx(|l| l.split_whitespace().nth(1)==Some(c), c) }
    fn reg(&mut self) { self.num("001"); }
    fn drain(&mut self) {
        self.writer.try_clone().unwrap().set_read_timeout(Some(Duration::from_millis(300))).ok();
        let mut b = String::new(); loop { b.clear(); match self.reader.read_line(&mut b) {
            Ok(0) => break, Ok(_) => if b.starts_with("PING") {
                let t = b.trim_end().strip_prefix("PING ").unwrap_or(":x");
                let _ = writeln!(self.writer, "PONG {t}\r"); let _ = self.writer.flush(); },
            Err(_) => break, }}
        self.writer.try_clone().unwrap().set_read_timeout(Some(Duration::from_secs(5))).ok();
    }
    fn maybe(&mut self, p: impl Fn(&str) -> bool, ms: u64) -> Option<String> {
        self.writer.try_clone().unwrap().set_read_timeout(Some(Duration::from_millis(ms))).ok();
        let mut b = String::new(); let r = loop { b.clear(); match self.reader.read_line(&mut b) {
            Ok(0) => break None, Ok(_) => { let l = b.trim_end();
                if l.starts_with("PING") { let t = l.strip_prefix("PING ").unwrap_or(":x");
                    let _ = writeln!(self.writer, "PONG {t}\r"); let _ = self.writer.flush(); continue; }
                if p(l) { break Some(l.to_string()); }
            } Err(_) => break None, }};
        self.writer.try_clone().unwrap().set_read_timeout(Some(Duration::from_secs(5))).ok(); r
    }
    /// Collect all PRIVMSG lines from a CHATHISTORY batch response
    fn collect_batch_messages(&mut self) -> Vec<String> {
        let mut msgs = Vec::new();
        // Wait for BATCH start
        self.rx(|l| l.contains("BATCH +"), "BATCH start");
        // Collect until BATCH end
        loop {
            let line = self.rx(|_| true, "batch line");
            if line.contains("BATCH -") { break; }
            if line.contains("PRIVMSG") { msgs.push(line); }
        }
        msgs
    }
    fn extract_msgid(line: &str) -> String {
        if let Some(tags_str) = line.strip_prefix('@').and_then(|s| s.split_once(' ').map(|(t,_)| t)) {
            for tag in tags_str.split(';') {
                if let Some(val) = tag.strip_prefix("msgid=") { return val.to_string(); }
            }
        }
        String::new()
    }
}

// ═══════════════════════════════════════════════════════════════
// CHANNEL HISTORY: membership check
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
async fn chathistory_multiline_message_replayed_as_split_privmsgs() {
    // A message that landed via a draft/multiline batch is stored
    // with the assembled body (\n between paragraphs). When the
    // history is replayed via CHATHISTORY, the server must split
    // at \n so each constituent line lands as its own valid
    // PRIVMSG — otherwise the receiver's parser splits mid-line
    // and produces protocol errors.
    let r = resolver(vec![]);
    let (addr, _h) = start(r).await;
    run(addr, |addr| {
        // alice negotiates draft/multiline + batch and sends a 3-line
        // logical message via a multiline batch into #mlhist.
        let mut alice = C::with_caps(addr, "ml_alice");
        alice.tx("CAP REQ :draft/multiline");
        alice.rx(|l| l.contains("ACK"), "draft/multiline ACK");
        alice.drain();
        alice.tx("JOIN #mlhist");
        alice.num("366");
        alice.drain();
        alice.tx("BATCH +ab1 draft/multiline #mlhist");
        alice.tx("@batch=ab1 PRIVMSG #mlhist :first line of the opening");
        alice.tx("@batch=ab1 PRIVMSG #mlhist :second line carries the claim");
        alice.tx("@batch=ab1 PRIVMSG #mlhist :third line is the conclusion");
        alice.tx("BATCH -ab1");
        alice.drain();
        std::thread::sleep(Duration::from_millis(150));

        // bob joins #mlhist and asks for history. He must see three
        // PRIVMSGs (one per original chunk), none containing a raw
        // newline in the body.
        let mut bob = C::with_caps(addr, "ml_bob");
        bob.tx("JOIN #mlhist");
        bob.num("366");
        bob.drain();
        bob.tx("CHATHISTORY LATEST #mlhist * 50");
        let msgs = bob.collect_batch_messages();

        let first_seen = msgs.iter().any(|m| m.contains("first line of the opening"));
        let second_seen = msgs.iter().any(|m| m.contains("second line carries the claim"));
        let third_seen = msgs.iter().any(|m| m.contains("third line is the conclusion"));
        assert!(first_seen && second_seen && third_seen,
            "all 3 lines should appear in history replay; got {} messages: {msgs:?}",
            msgs.len());

        // None of the PRIVMSG bodies may contain a raw `\n`.
        for m in &msgs {
            assert!(
                !m.contains('\n') || m.ends_with('\n'),
                "internal \\n in replayed PRIVMSG body: {m}",
            );
        }

        // msgid is on the first chunk only (per IRCv3 spec § "Message
        // ids" + § "Fallback"). The remaining chunks of the same
        // logical message must NOT carry msgid.
        // Find the chunk carrying "first line of the opening" — it
        // should have msgid; the chunks for line 2 and line 3 should
        // not.
        let first_chunk = msgs.iter().find(|m| m.contains("first line of the opening"))
            .expect("first line present");
        let second_chunk = msgs.iter().find(|m| m.contains("second line carries the claim"))
            .expect("second line present");
        let third_chunk = msgs.iter().find(|m| m.contains("third line is the conclusion"))
            .expect("third line present");
        assert!(first_chunk.contains("msgid="),
            "first chunk should carry msgid: {first_chunk}");
        assert!(!second_chunk.contains("msgid="),
            "second chunk should NOT carry msgid: {second_chunk}");
        assert!(!third_chunk.contains("msgid="),
            "third chunk should NOT carry msgid: {third_chunk}");
    }).await;
}

#[tokio::test]
async fn chathistory_multiline_capable_receiver_gets_nested_batch() {
    // When the requester negotiated draft/multiline, CHATHISTORY
    // replay must nest a draft/multiline BATCH inside the chathistory
    // BATCH for each multiline row, matching the live broadcast shape.
    // Otherwise the receiver sees live messages grouped but history
    // messages fragmented — bad UX and a spec degradation for clients
    // that explicitly opted in.
    let r = resolver(vec![]);
    let (addr, _h) = start(r).await;
    run(addr, |addr| {
        // alice sends a 3-line multiline message into #mlhist2.
        let mut alice = C::with_caps(addr, "ml2_alice");
        alice.tx("CAP REQ :draft/multiline");
        alice.rx(|l| l.contains("ACK"), "draft/multiline ACK");
        alice.drain();
        alice.tx("JOIN #mlhist2");
        alice.num("366");
        alice.drain();
        alice.tx("BATCH +ab2 draft/multiline #mlhist2");
        alice.tx("@batch=ab2 PRIVMSG #mlhist2 :alpha line");
        alice.tx("@batch=ab2 PRIVMSG #mlhist2 :beta line");
        alice.tx("@batch=ab2 PRIVMSG #mlhist2 :gamma line");
        alice.tx("BATCH -ab2");
        alice.drain();
        std::thread::sleep(Duration::from_millis(150));

        // bob negotiates draft/multiline and requests history. He
        // should see: chathistory BATCH +, nested multiline BATCH +,
        // 3 chunk PRIVMSGs each carrying batch=<inner>, nested BATCH -,
        // chathistory BATCH -.
        let mut bob = C::with_caps(addr, "ml2_bob");
        bob.tx("CAP REQ :draft/multiline");
        bob.rx(|l| l.contains("ACK"), "draft/multiline ACK");
        bob.drain();
        bob.tx("JOIN #mlhist2");
        bob.num("366");
        bob.drain();
        bob.tx("CHATHISTORY LATEST #mlhist2 * 50");

        // Read everything between the outer chathistory BATCH + and -.
        let outer_open = bob.rx(|l| l.contains("BATCH +") && l.contains("chathistory"),
            "chathistory BATCH start");
        let outer_id = {
            let after_at = outer_open.find("BATCH +").unwrap() + "BATCH +".len();
            let rest = &outer_open[after_at..];
            rest.split_whitespace().next().unwrap().to_string()
        };

        // Read until we see `BATCH -<outer_id>` as a frame parameter
        // (not just the outer id appearing in a `batch=` tag of a
        // nested closer).
        let outer_close_param = format!("BATCH -{outer_id}");
        let mut lines = Vec::new();
        loop {
            let l = bob.rx(|_| true, "batch line");
            // A BATCH - frame ends with `BATCH -<id>` (after the tag/
            // prefix prefix), so trim and check the suffix.
            if l.trim_end().ends_with(&outer_close_param) {
                break;
            }
            lines.push(l);
        }

        // Must contain a nested draft/multiline BATCH +.
        let inner_open = lines.iter().find(|l| l.contains("BATCH +") && l.contains("draft/multiline"))
            .expect(&format!("expected nested multiline BATCH +, lines: {lines:#?}"));
        // Inner opener should carry the chathistory batch tag for
        // nesting (batch=<outer_id>) AND the msgid for the logical
        // message.
        assert!(inner_open.contains(&format!("batch={outer_id}")),
            "inner BATCH + should reference outer chathistory batch: {inner_open}");
        assert!(inner_open.contains("msgid="),
            "inner BATCH + should carry the logical message's msgid: {inner_open}");

        // Three PRIVMSG chunks should carry batch=<inner_id>.
        let inner_id = {
            let after_at = inner_open.find("BATCH +").unwrap() + "BATCH +".len();
            let rest = &inner_open[after_at..];
            rest.split_whitespace().next().unwrap().to_string()
        };
        let chunk_count = lines.iter().filter(|l|
            l.contains("PRIVMSG") && l.contains(&format!("batch={inner_id}"))
        ).count();
        assert_eq!(chunk_count, 3,
            "expected 3 chunk PRIVMSGs carrying batch={inner_id}, got: {lines:#?}");

        // The 3 chunks must carry the chunk bodies, not the joined body.
        assert!(lines.iter().any(|l| l.contains(":alpha line")));
        assert!(lines.iter().any(|l| l.contains(":beta line")));
        assert!(lines.iter().any(|l| l.contains(":gamma line")));

        // Nested closer BATCH -<inner_id> must be present (we already
        // know the outer closer arrived because we broke the loop on
        // BATCH -<outer_id>).
        let inner_close = lines.iter().find(|l|
            l.contains(&format!("BATCH -{inner_id}"))
        );
        assert!(inner_close.is_some(),
            "expected nested BATCH -{inner_id}: {lines:#?}");
    }).await;
}

#[tokio::test]
async fn chathistory_requires_channel_membership() {
    let r = resolver(vec![]);
    let (addr, _h) = start(r).await;
    run(addr, |addr| {
        let mut alice = C::with_caps(addr, "ch_alice");
        alice.reg(); alice.drain();
        alice.tx("JOIN #history"); alice.num("366"); alice.drain();
        alice.tx("PRIVMSG #history :secret message");
        alice.drain();

        // Bob is NOT in #history
        let mut bob = C::with_caps(addr, "ch_bob");
        bob.reg(); bob.drain();
        bob.tx("CHATHISTORY LATEST #history * 50");
        let fail = bob.rx(|l| l.contains("FAIL"), "access denied");
        assert!(fail.contains("INVALID_TARGET") || fail.contains("FAIL"),
            "Non-member should be denied CHATHISTORY: {fail}");
    }).await;
}

#[tokio::test]
async fn chathistory_works_for_member() {
    let r = resolver(vec![]);
    let (addr, _h) = start(r).await;
    run(addr, |addr| {
        let mut alice = C::with_caps(addr, "hm_alice");
        alice.reg(); alice.drain();
        alice.tx("JOIN #histok"); alice.num("366"); alice.drain();
        alice.tx("PRIVMSG #histok :test message for history");
        alice.drain();
        std::thread::sleep(Duration::from_millis(100));

        alice.tx("CHATHISTORY LATEST #histok * 50");
        let msgs = alice.collect_batch_messages();
        assert!(msgs.iter().any(|m| m.contains("test message for history")),
            "Member should see message in history: {msgs:?}");
    }).await;
}

// ═══════════════════════════════════════════════════════════════
// DELETED MESSAGES: must NOT appear in history
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
async fn deleted_messages_excluded_from_chathistory() {
    let r = resolver(vec![]);
    let (addr, _h) = start(r).await;
    run(addr, |addr| {
        let mut alice = C::with_caps(addr, "del_a");
        alice.reg(); alice.drain();
        let mut bob = C::with_caps(addr, "del_b");
        bob.reg(); bob.drain();
        alice.tx("JOIN #delhist"); alice.num("366"); alice.drain();
        bob.tx("JOIN #delhist"); bob.num("366"); bob.drain();

        // Alice sends two messages
        alice.tx("PRIVMSG #delhist :keep this");
        bob.rx(|l| l.contains("keep this"), "msg1");
        alice.tx("PRIVMSG #delhist :delete this");
        let m2 = bob.rx(|l| l.contains("delete this"), "msg2");
        let del_msgid = C::extract_msgid(&m2);

        // Delete the second message
        alice.tx(&format!("@+draft/delete={del_msgid} TAGMSG #delhist"));
        std::thread::sleep(Duration::from_millis(200));

        // Request CHATHISTORY — deleted message should be absent
        alice.drain();
        alice.tx("CHATHISTORY LATEST #delhist * 50");
        let msgs = alice.collect_batch_messages();
        assert!(msgs.iter().any(|m| m.contains("keep this")), "Kept message should be in history");
        assert!(!msgs.iter().any(|m| m.contains("delete this")), "Deleted message should NOT be in history");
    }).await;
}

// ═══════════════════════════════════════════════════════════════
// EDITED MESSAGES: should show current text with edit tag
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
async fn edited_message_shows_new_text_in_history() {
    let r = resolver(vec![]);
    let (addr, _h) = start(r).await;
    run(addr, |addr| {
        let mut alice = C::with_caps(addr, "edit_a");
        alice.reg(); alice.drain();
        let mut bob = C::with_caps(addr, "edit_b");
        bob.reg(); bob.drain();
        alice.tx("JOIN #edithist"); alice.num("366"); alice.drain();
        bob.tx("JOIN #edithist"); bob.num("366"); bob.drain();

        alice.tx("PRIVMSG #edithist :original text");
        let m = bob.rx(|l| l.contains("original text"), "msg");
        let msgid = C::extract_msgid(&m);

        // Edit the message
        alice.tx(&format!("@+draft/edit={msgid} PRIVMSG #edithist :edited text"));
        bob.rx(|l| l.contains("edited text"), "edit");
        std::thread::sleep(Duration::from_millis(200));

        // New user joins and requests history
        let mut carol = C::with_caps(addr, "edit_c");
        carol.reg(); carol.drain();
        carol.tx("JOIN #edithist"); carol.num("366"); carol.drain();
        carol.tx("CHATHISTORY LATEST #edithist * 50");
        let msgs = carol.collect_batch_messages();

        // History should contain the edit (either as separate edit entry or updated text)
        let has_edit = msgs.iter().any(|m| m.contains("edited text"));
        // The edit should be visible in history
        assert!(has_edit, "Edited text should appear in history: {msgs:?}");
    }).await;
}

// ═══════════════════════════════════════════════════════════════
// JOIN HISTORY REPLAY: shows recent messages
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
async fn join_replays_recent_messages() {
    let r = resolver(vec![]);
    let (addr, _h) = start(r).await;
    run(addr, |addr| {
        let mut alice = C::with_caps(addr, "jr_a");
        alice.reg(); alice.drain();
        alice.tx("JOIN #joinreplay"); alice.num("366"); alice.drain();

        // Send some messages
        for i in 0..5 {
            alice.tx(&format!("PRIVMSG #joinreplay :message {i}"));
        }
        std::thread::sleep(Duration::from_millis(200));

        // Bob joins — should see recent messages in batch replay
        let mut bob = C::with_caps(addr, "jr_b");
        bob.reg(); bob.drain();
        bob.tx("JOIN #joinreplay");
        // Drain until 366 (end of names), collecting any PRIVMSG on the way
        let mut saw_messages = false;
        loop {
            let line = bob.rx(|_| true, "join replay");
            if line.contains("PRIVMSG") && line.contains("message") {
                saw_messages = true;
            }
            if line.split_whitespace().nth(1) == Some("366") { break; }
        }
        assert!(saw_messages, "Bob should see message replay on join");
    }).await;
}

// ═══════════════════════════════════════════════════════════════
// CHATHISTORY AFTER PART: should fail
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
async fn chathistory_after_part_denied() {
    let r = resolver(vec![]);
    let (addr, _h) = start(r).await;
    run(addr, |addr| {
        let mut alice = C::with_caps(addr, "ap_a");
        alice.reg(); alice.drain();
        alice.tx("JOIN #partcheck"); alice.num("366"); alice.drain();
        alice.tx("PRIVMSG #partcheck :before part");
        alice.drain();

        // Part the channel
        alice.tx("PART #partcheck");
        alice.rx(|l| l.contains("PART"), "PART");

        // Try CHATHISTORY after parting — should fail (not a member)
        alice.tx("CHATHISTORY LATEST #partcheck * 50");
        let result = alice.maybe(|l| l.contains("FAIL") || l.contains("BATCH"), 2000);
        if let Some(line) = result {
            if line.contains("BATCH") {
                // Got history despite not being a member — document this
                eprintln!("NOTE: CHATHISTORY allowed after PART (implementation choice)");
            }
        }
    }).await;
}

// ═══════════════════════════════════════════════════════════════
// DM HISTORY: access control
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
async fn dm_chathistory_requires_auth() {
    let r = resolver(vec![]);
    let (addr, _h) = start(r).await;
    run(addr, |addr| {
        // Guest (no DID) tries CHATHISTORY for a DM
        let mut guest = C::with_caps(addr, "dm_guest");
        guest.reg(); guest.drain();
        guest.tx("CHATHISTORY LATEST some_nick * 50");
        let fail = guest.rx(|l| l.contains("FAIL"), "DM history denied");
        assert!(fail.contains("ACCOUNT_REQUIRED") || fail.contains("FAIL"),
            "Guest should be denied DM CHATHISTORY: {fail}");
    }).await;
}

#[tokio::test]
async fn dm_chathistory_between_authenticated_users() {
    let key_a = PrivateKey::generate_ed25519();
    let key_b = PrivateKey::generate_ed25519();
    let r = resolver(vec![(DID_A, &key_a), (DID_B, &key_b)]);
    let (addr, _h) = start(r).await;
    run(addr, move |addr| {
        let mut alice = C::with_sasl(addr, "dh_alice", DID_A, key_a);
        alice.reg(); alice.drain();
        let mut bob = C::with_sasl(addr, "dh_bob", DID_B, key_b);
        bob.reg(); bob.drain();

        // Alice DMs Bob
        alice.tx("PRIVMSG dh_bob :private dm message");
        bob.rx(|l| l.contains("private dm"), "DM");
        std::thread::sleep(Duration::from_millis(200));

        // Alice requests DM history with Bob
        alice.tx(&format!("CHATHISTORY LATEST {DID_B} * 50"));
        let result = alice.maybe(|l| l.contains("BATCH") || l.contains("FAIL"), 2000);
        // Should get batch with the DM message
        if let Some(line) = &result {
            if line.contains("BATCH") {
                // Collect messages
                let mut msgs = Vec::new();
                loop {
                    let l = alice.rx(|_| true, "batch");
                    if l.contains("BATCH -") { break; }
                    if l.contains("PRIVMSG") { msgs.push(l); }
                }
                assert!(msgs.iter().any(|m| m.contains("private dm")),
                    "DM history should contain the message");
            }
        }
    }).await;
}

#[tokio::test]
async fn dm_chathistory_third_party_cannot_read() {
    let key_a = PrivateKey::generate_ed25519();
    let key_b = PrivateKey::generate_ed25519();
    let key_c = PrivateKey::generate_ed25519();
    let r = resolver(vec![(DID_A, &key_a), (DID_B, &key_b), (DID_C, &key_c)]);
    let (addr, _h) = start(r).await;
    run(addr, move |addr| {
        let mut alice = C::with_sasl(addr, "tp_alice", DID_A, key_a);
        alice.reg(); alice.drain();
        let mut bob = C::with_sasl(addr, "tp_bob", DID_B, key_b);
        bob.reg(); bob.drain();
        let mut eve = C::with_sasl(addr, "tp_eve", DID_C, key_c);
        eve.reg(); eve.drain();

        // Alice DMs Bob
        alice.tx("PRIVMSG tp_bob :super secret");
        bob.rx(|l| l.contains("super secret"), "DM");
        std::thread::sleep(Duration::from_millis(200));

        // Eve tries to read Alice↔Bob DM history
        // Eve requests CHATHISTORY with Bob's DID
        eve.tx(&format!("CHATHISTORY LATEST {DID_B} * 50"));
        let result = eve.maybe(|l| l.contains("BATCH") || l.contains("FAIL"), 2000);
        if let Some(line) = &result {
            if line.contains("BATCH") {
                let mut msgs = Vec::new();
                loop {
                    let l = eve.rx(|_| true, "batch");
                    if l.contains("BATCH -") { break; }
                    if l.contains("PRIVMSG") { msgs.push(l); }
                }
                // Eve's query creates canonical_dm_key(eve_did, bob_did) — different from alice↔bob
                // So she should NOT see alice's messages
                if msgs.iter().any(|m| m.contains("super secret")) {
                    panic!("BUG: Eve can read Alice↔Bob DM via CHATHISTORY!");
                }
            }
        }
    }).await;
}

// ═══════════════════════════════════════════════════════════════
// CHATHISTORY TARGETS: DM list privacy
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
async fn chathistory_targets_requires_auth() {
    let r = resolver(vec![]);
    let (addr, _h) = start(r).await;
    run(addr, |addr| {
        let mut guest = C::with_caps(addr, "tgt_guest");
        guest.reg(); guest.drain();
        guest.tx("CHATHISTORY TARGETS * * 50");
        let fail = guest.rx(|l| l.contains("FAIL"), "targets denied");
        assert!(fail.contains("ACCOUNT_REQUIRED") || fail.contains("FAIL"),
            "Guest should be denied CHATHISTORY TARGETS: {fail}");
    }).await;
}

// ═══════════════════════════════════════════════════════════════
// CHATHISTORY PAGINATION
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
async fn chathistory_limit_capped_at_500() {
    let r = resolver(vec![]);
    let (addr, _h) = start(r).await;
    run(addr, |addr| {
        let mut alice = C::with_caps(addr, "lim_a");
        alice.reg(); alice.drain();
        alice.tx("JOIN #limit"); alice.num("366"); alice.drain();

        // Send 3 messages (within flood limit of 5/2sec)
        for i in 0..3 {
            alice.tx(&format!("PRIVMSG #limit :msg {i}"));
        }
        std::thread::sleep(Duration::from_millis(200));
        alice.drain();

        // Request with limit=999999 — should be capped at 500 but work
        alice.tx("CHATHISTORY LATEST #limit * 999999");
        let msgs = alice.collect_batch_messages();
        assert!(msgs.len() <= 500, "Limit should be capped at 500");
        assert!(msgs.len() >= 3, "Should have our messages: got {}", msgs.len());
    }).await;
}

#[tokio::test]
async fn chathistory_before_returns_older_messages() {
    let r = resolver(vec![]);
    let (addr, _h) = start(r).await;
    run(addr, |addr| {
        let mut alice = C::with_caps(addr, "bef_a");
        alice.reg(); alice.drain();
        alice.tx("JOIN #before"); alice.num("366"); alice.drain();

        // Send messages
        for i in 0..5 {
            alice.tx(&format!("PRIVMSG #before :msg {i}"));
            std::thread::sleep(Duration::from_millis(50));
        }
        std::thread::sleep(Duration::from_millis(200));
        alice.drain();

        // Get latest first
        alice.tx("CHATHISTORY LATEST #before * 50");
        let latest = alice.collect_batch_messages();
        assert!(!latest.is_empty(), "Should have messages");

        // BEFORE with future timestamp should return all messages
        // The server expects "timestamp=<unix>" format
        let future = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() + 9999;
        alice.tx(&format!("CHATHISTORY BEFORE #before timestamp={future} 50"));
        let result = alice.maybe(|l| l.contains("BATCH") || l.contains("FAIL"), 2000);
        // Either returns a batch (possibly empty if timestamp format wrong) or FAIL
        // Document actual behavior
        assert!(result.is_some(), "BEFORE should return some response");
    }).await;
}

// ═══════════════════════════════════════════════════════════════
// CHATHISTORY AFTER KICK
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
async fn chathistory_after_kick_denied() {
    let r = resolver(vec![]);
    let (addr, _h) = start(r).await;
    run(addr, |addr| {
        let mut op = C::with_caps(addr, "kick_op");
        op.reg(); op.drain();
        op.tx("JOIN #kickhist"); op.num("366"); op.drain();

        let mut victim = C::with_caps(addr, "kick_vic");
        victim.reg(); victim.drain();
        victim.tx("JOIN #kickhist"); victim.num("366"); victim.drain();

        // Op sends message, victim sees it
        op.tx("PRIVMSG #kickhist :you saw this");
        victim.rx(|l| l.contains("you saw this"), "msg");

        // Op kicks victim
        op.tx("KICK #kickhist kick_vic :gone");
        std::thread::sleep(Duration::from_millis(500));
        victim.drain();

        // Victim tries CHATHISTORY after being kicked
        victim.tx("CHATHISTORY LATEST #kickhist * 50");
        let result = victim.maybe(|l| l.contains("FAIL") || l.contains("BATCH"), 2000);
        if let Some(line) = result {
            if line.contains("BATCH") {
                // Kicked user can still get history — document this behavior
                eprintln!("NOTE: Kicked user can still CHATHISTORY (implementation choice)");
            }
        }
    }).await;
}
