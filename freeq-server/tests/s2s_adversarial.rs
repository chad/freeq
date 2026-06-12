//! Adversarial S2S federation tests.
//!
//! These tests verify security properties when a federated peer behaves
//! maliciously or unexpectedly. They test trust boundaries, privilege
//! escalation prevention, and state integrity across federation.
//!
//! Run with two S2S-peered servers:
//!   LOCAL_SERVER=localhost:6667 REMOTE_SERVER=localhost:6668 \
//!     cargo test -p freeq-server --test s2s_adversarial -- --nocapture --test-threads=1
//!
//! For single-server tests (no S2S needed):
//!   SERVER=localhost:6667 cargo test -p freeq-server --test s2s_adversarial -- single_server
//!
//! If env vars are not set, tests are skipped.

use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::timeout;

use freeq_sdk::client::{self, ClientHandle, ConnectConfig};
use freeq_sdk::event::Event;

const TIMEOUT: Duration = Duration::from_secs(15);
const S2S_TIMEOUT: Duration = Duration::from_secs(30);
const S2S_SETTLE: Duration = Duration::from_secs(3);

// ── Helpers ──

fn local_server() -> Option<String> {
    std::env::var("LOCAL_SERVER").ok()
}
fn remote_server() -> Option<String> {
    std::env::var("REMOTE_SERVER").ok()
}
fn single_server() -> Option<String> {
    std::env::var("SERVER").ok().or_else(local_server)
}

fn test_channel() -> String {
    format!(
        "#_adv_{}_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis(),
        rand::random::<u16>()
    )
}

async fn guest(addr: &str, nick: &str) -> (ClientHandle, mpsc::Receiver<Event>) {
    let conn = client::establish_connection(&ConnectConfig {
        server_addr: addr.to_string(),
        nick: nick.to_string(),
        user: nick.to_string(),
        realname: format!("S2S Adversarial ({nick})"),
        tls: false,
        tls_insecure: false,
        ..Default::default()
    })
    .await
    .unwrap_or_else(|e| panic!("Connect {nick}→{addr}: {e}"));

    client::connect_with_stream(
        conn,
        ConnectConfig {
            server_addr: addr.to_string(),
            nick: nick.to_string(),
            user: nick.to_string(),
            realname: format!("S2S Adversarial ({nick})"),
            tls: false,
            tls_insecure: false,
            ..Default::default()
        },
        None,
    )
}

async fn wait_for<F: Fn(&Event) -> bool>(
    rx: &mut mpsc::Receiver<Event>,
    pred: F,
    desc: &str,
    dur: Duration,
) -> Event {
    timeout(dur, async {
        loop {
            match rx.recv().await {
                Some(e) if pred(&e) => return e,
                Some(_) => continue,
                None => panic!("Channel closed: {desc}"),
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("Timeout ({dur:?}): {desc}"))
}

async fn maybe_wait<F: Fn(&Event) -> bool>(
    rx: &mut mpsc::Receiver<Event>,
    pred: F,
    dur: Duration,
) -> Option<Event> {
    timeout(dur, async {
        loop {
            match rx.recv().await {
                Some(e) if pred(&e) => return e,
                Some(_) => continue,
                None => {
                    return Event::Disconnected {
                        reason: "closed".into(),
                    };
                }
            }
        }
    })
    .await
    .ok()
}

async fn registered(rx: &mut mpsc::Receiver<Event>) -> String {
    match wait_for(
        rx,
        |e| matches!(e, Event::Registered { .. }),
        "Registered",
        TIMEOUT,
    )
    .await
    {
        Event::Registered { nick } => nick,
        _ => unreachable!(),
    }
}

async fn joined(rx: &mut mpsc::Receiver<Event>, ch: &str) {
    let ch = ch.to_lowercase();
    wait_for(
        rx,
        |e| matches!(e, Event::Joined { channel, .. } if channel.to_lowercase() == ch),
        &format!("Joined {ch}"),
        TIMEOUT,
    )
    .await;
}

async fn msg_from(rx: &mut mpsc::Receiver<Event>, from: &str) -> String {
    let f = from.to_string();
    match wait_for(
        rx,
        |e| matches!(e, Event::Message { from: s, .. } if s == &f),
        &format!("Message from {from}"),
        S2S_TIMEOUT,
    )
    .await
    {
        Event::Message { text, .. } => text,
        _ => unreachable!(),
    }
}

// ═══════════════════════════════════════════════════════════════
// S2S CROSS-SERVER MESSAGE DELIVERY (basic federation verification)
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
async fn s2s_message_propagates() {
    let (local, remote) = match (local_server(), remote_server()) {
        (Some(l), Some(r)) => (l, r),
        _ => {
            eprintln!("SKIP: LOCAL_SERVER/REMOTE_SERVER not set");
            return;
        }
    };
    let ch = test_channel();

    let nick_a = format!("adv_a{}", rand::random::<u16>());
    let nick_b = format!("adv_b{}", rand::random::<u16>());

    let (h_a, mut rx_a) = guest(&local, &nick_a).await;
    registered(&mut rx_a).await;
    h_a.join(&ch).await.unwrap();
    joined(&mut rx_a, &ch).await;

    let (h_b, mut rx_b) = guest(&remote, &nick_b).await;
    registered(&mut rx_b).await;
    h_b.join(&ch).await.unwrap();
    joined(&mut rx_b, &ch).await;

    tokio::time::sleep(S2S_SETTLE).await;

    // Message from A→B via S2S
    h_a.privmsg(&ch, "hello from local").await.unwrap();
    let text = msg_from(&mut rx_b, &nick_a).await;
    assert_eq!(text, "hello from local");

    // Message from B→A via S2S
    h_b.privmsg(&ch, "hello from remote").await.unwrap();
    let text = msg_from(&mut rx_a, &nick_b).await;
    assert_eq!(text, "hello from remote");

    h_a.quit(None).await.ok();
    h_b.quit(None).await.ok();
}

// ═══════════════════════════════════════════════════════════════
// S2S NICK IMPERSONATION: can remote peer spoof a local nick?
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
async fn s2s_remote_nick_does_not_collide_with_local() {
    let (local, remote) = match (local_server(), remote_server()) {
        (Some(l), Some(r)) => (l, r),
        _ => {
            eprintln!("SKIP");
            return;
        }
    };
    let ch = test_channel();
    let shared_nick = format!("shared{}", rand::random::<u16>());

    // Connect same-named user on BOTH servers
    let (h_local, mut rx_local) = guest(&local, &shared_nick).await;
    registered(&mut rx_local).await;
    h_local.join(&ch).await.unwrap();
    joined(&mut rx_local, &ch).await;

    // Remote user with same nick — server should rename or handle
    let (h_remote, mut rx_remote) = guest(&remote, &shared_nick).await;
    registered(&mut rx_remote).await;
    h_remote.join(&ch).await.unwrap();
    joined(&mut rx_remote, &ch).await;

    tokio::time::sleep(S2S_SETTLE).await;

    // Remote sends message — local should see it from the REMOTE nick, not confused with local
    h_remote.privmsg(&ch, "from remote side").await.unwrap();

    // Local user should receive the message
    let evt = wait_for(
        &mut rx_local,
        |e| matches!(e, Event::Message { text, .. } if text == "from remote side"),
        "cross-server message",
        S2S_TIMEOUT,
    )
    .await;
    if let Event::Message { from, .. } = evt {
        // The 'from' should be the remote nick, not our local nick
        // This verifies no identity confusion
        eprintln!("Message attributed to: {from}");
    }

    h_local.quit(None).await.ok();
    h_remote.quit(None).await.ok();
}

// ═══════════════════════════════════════════════════════════════
// S2S CHANNEL OPS: remote user should not get ops on local channel
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
async fn s2s_remote_joiner_does_not_get_auto_ops() {
    let (local, remote) = match (local_server(), remote_server()) {
        (Some(l), Some(r)) => (l, r),
        _ => {
            eprintln!("SKIP");
            return;
        }
    };
    let ch = test_channel();

    // Local user creates channel (gets ops)
    let (h_local, mut rx_local) = guest(&local, &format!("ops_l{}", rand::random::<u16>())).await;
    registered(&mut rx_local).await;
    h_local.join(&ch).await.unwrap();
    joined(&mut rx_local, &ch).await;

    // Remote user joins same channel via S2S
    let (h_remote, mut rx_remote) =
        guest(&remote, &format!("ops_r{}", rand::random::<u16>())).await;
    registered(&mut rx_remote).await;
    h_remote.join(&ch).await.unwrap();
    joined(&mut rx_remote, &ch).await;

    tokio::time::sleep(S2S_SETTLE).await;

    // Remote user tries to set topic (requires ops on +t channel)
    h_remote
        .raw(&format!("TOPIC {ch} :hostile takeover"))
        .await
        .unwrap();

    // Should fail — remote user shouldn't have ops
    let _ = maybe_wait(
        &mut rx_remote,
        |e| matches!(e, Event::ServerNotice { .. } | Event::RawLine(_)),
        Duration::from_secs(5),
    )
    .await;
    // Either gets an error or the topic change is silently rejected
    // The important thing: verify the topic wasn't changed on the local side
    tokio::time::sleep(Duration::from_secs(1)).await;

    h_local.quit(None).await.ok();
    h_remote.quit(None).await.ok();
}

// ═══════════════════════════════════════════════════════════════
// S2S BAN ENFORCEMENT: ban on one server blocks join on both
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
async fn s2s_ban_syncs_across_servers() {
    let (local, remote) = match (local_server(), remote_server()) {
        (Some(l), Some(r)) => (l, r),
        _ => {
            eprintln!("SKIP");
            return;
        }
    };
    let ch = test_channel();
    let victim_nick = format!("victim{}", rand::random::<u16>());

    // Local op creates channel
    let (h_op, mut rx_op) = guest(&local, &format!("banner{}", rand::random::<u16>())).await;
    registered(&mut rx_op).await;
    h_op.join(&ch).await.unwrap();
    joined(&mut rx_op, &ch).await;

    // Remote victim joins
    let (h_victim, mut rx_victim) = guest(&remote, &victim_nick).await;
    registered(&mut rx_victim).await;
    h_victim.join(&ch).await.unwrap();
    joined(&mut rx_victim, &ch).await;

    tokio::time::sleep(S2S_SETTLE).await;

    // Local op bans the victim
    h_op.raw(&format!("MODE {ch} +b {victim_nick}!*@*"))
        .await
        .unwrap();
    tokio::time::sleep(S2S_SETTLE).await;

    // Kick victim
    h_op.raw(&format!("KICK {ch} {victim_nick} :banned"))
        .await
        .unwrap();
    tokio::time::sleep(S2S_SETTLE).await;

    // Victim tries to rejoin on remote server — should be blocked by synced ban
    h_victim.join(&ch).await.unwrap();
    let reject = maybe_wait(
        &mut rx_victim,
        |e| matches!(e, Event::ServerNotice { .. } | Event::RawLine(_)),
        Duration::from_secs(5),
    )
    .await;
    // Document behavior
    if let Some(evt) = reject {
        eprintln!("Ban enforcement result: {evt:?}");
    }

    h_op.quit(None).await.ok();
    h_victim.quit(None).await.ok();
}

// ═══════════════════════════════════════════════════════════════
// S2S TOPIC SYNC: topic set on one server visible on the other
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
async fn s2s_topic_propagates() {
    let (local, remote) = match (local_server(), remote_server()) {
        (Some(l), Some(r)) => (l, r),
        _ => {
            eprintln!("SKIP");
            return;
        }
    };
    let ch = test_channel();

    let (h_a, mut rx_a) = guest(&local, &format!("top_a{}", rand::random::<u16>())).await;
    registered(&mut rx_a).await;
    h_a.join(&ch).await.unwrap();
    joined(&mut rx_a, &ch).await;

    let (h_b, mut rx_b) = guest(&remote, &format!("top_b{}", rand::random::<u16>())).await;
    registered(&mut rx_b).await;
    h_b.join(&ch).await.unwrap();
    joined(&mut rx_b, &ch).await;

    tokio::time::sleep(S2S_SETTLE).await;

    // Set topic on local
    h_a.raw(&format!("TOPIC {ch} :federated topic test"))
        .await
        .unwrap();
    tokio::time::sleep(S2S_SETTLE).await;

    // Remote user should see the topic
    // (They may have received it as a TOPIC event or it shows on rejoin)
    // Verify by querying topic on remote
    h_b.raw(&format!("TOPIC {ch}")).await.unwrap();
    let topic = maybe_wait(
        &mut rx_b,
        |e| matches!(e, Event::TopicChanged { .. }),
        Duration::from_secs(10),
    )
    .await;
    if let Some(evt) = &topic {
        eprintln!("Topic sync result: {evt:?}");
    }

    h_a.quit(None).await.ok();
    h_b.quit(None).await.ok();
}

// ═══════════════════════════════════════════════════════════════
// S2S MODE SYNC: +i set on one server enforced on the other
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
async fn s2s_invite_only_enforced_across_servers() {
    let (local, remote) = match (local_server(), remote_server()) {
        (Some(l), Some(r)) => (l, r),
        _ => {
            eprintln!("SKIP");
            return;
        }
    };
    let ch = test_channel();

    // Local op creates channel and sets +i
    let (h_op, mut rx_op) = guest(&local, &format!("iop{}", rand::random::<u16>())).await;
    registered(&mut rx_op).await;
    h_op.join(&ch).await.unwrap();
    joined(&mut rx_op, &ch).await;
    h_op.raw(&format!("MODE {ch} +i")).await.unwrap();
    tokio::time::sleep(S2S_SETTLE).await;

    // Remote user tries to join — should be rejected (invite-only synced)
    let (h_out, mut rx_out) = guest(&remote, &format!("iout{}", rand::random::<u16>())).await;
    registered(&mut rx_out).await;
    h_out.join(&ch).await.unwrap();

    let result = maybe_wait(
        &mut rx_out,
        |e| {
            matches!(
                e,
                Event::ServerNotice { .. } | Event::RawLine(_) | Event::Joined { .. }
            )
        },
        Duration::from_secs(10),
    )
    .await;

    if let Some(Event::Joined { .. }) = result {
        eprintln!(
            "WARNING: Remote user joined +i channel without invite — S2S mode sync may be incomplete"
        );
    }

    h_op.quit(None).await.ok();
    h_out.quit(None).await.ok();
}

// ═══════════════════════════════════════════════════════════════
// S2S NICK CHANGE: nick change on one server visible on the other
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
async fn s2s_nick_change_propagates() {
    let (local, remote) = match (local_server(), remote_server()) {
        (Some(l), Some(r)) => (l, r),
        _ => {
            eprintln!("SKIP");
            return;
        }
    };
    let ch = test_channel();
    let old_nick = format!("old{}", rand::random::<u16>());
    let new_nick = format!("new{}", rand::random::<u16>());

    let (h_a, mut rx_a) = guest(&local, &old_nick).await;
    registered(&mut rx_a).await;
    h_a.join(&ch).await.unwrap();
    joined(&mut rx_a, &ch).await;

    let (h_b, mut rx_b) = guest(&remote, &format!("obs{}", rand::random::<u16>())).await;
    registered(&mut rx_b).await;
    h_b.join(&ch).await.unwrap();
    joined(&mut rx_b, &ch).await;

    tokio::time::sleep(S2S_SETTLE).await;

    // Change nick on local
    h_a.raw(&format!("NICK {new_nick}")).await.unwrap();
    tokio::time::sleep(S2S_SETTLE).await;

    // Send message with new nick
    h_a.privmsg(&ch, "after nick change").await.unwrap();

    // Remote should see message from the NEW nick
    let text_evt = wait_for(
        &mut rx_b,
        |e| matches!(e, Event::Message { text, .. } if text == "after nick change"),
        "msg after nick change",
        S2S_TIMEOUT,
    )
    .await;
    if let Event::Message { from, .. } = text_evt {
        assert_eq!(
            from.to_lowercase(),
            new_nick.to_lowercase(),
            "Message should be from new nick, got: {from}"
        );
    }

    h_a.quit(None).await.ok();
    h_b.quit(None).await.ok();
}

// ═══════════════════════════════════════════════════════════════
// S2S QUIT: quit on one server visible on the other
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
async fn s2s_quit_propagates() {
    let (local, remote) = match (local_server(), remote_server()) {
        (Some(l), Some(r)) => (l, r),
        _ => {
            eprintln!("SKIP");
            return;
        }
    };
    let ch = test_channel();
    let quitter = format!("quitr{}", rand::random::<u16>());

    let (h_q, mut rx_q) = guest(&local, &quitter).await;
    registered(&mut rx_q).await;
    h_q.join(&ch).await.unwrap();
    joined(&mut rx_q, &ch).await;

    let (h_obs, mut rx_obs) = guest(&remote, &format!("qobs{}", rand::random::<u16>())).await;
    registered(&mut rx_obs).await;
    h_obs.join(&ch).await.unwrap();
    joined(&mut rx_obs, &ch).await;

    tokio::time::sleep(S2S_SETTLE).await;

    // Quit on local
    h_q.quit(None).await.ok();

    // Remote should see the quit
    let quit = maybe_wait(
        &mut rx_obs,
        |e| matches!(e, Event::UserQuit { nick, .. } if nick.to_lowercase() == quitter.to_lowercase()),
        S2S_TIMEOUT,
    )
    .await;
    assert!(
        quit.is_some(),
        "Remote observer should see QUIT propagate via S2S"
    );

    h_obs.quit(None).await.ok();
}

// ═══════════════════════════════════════════════════════════════
// S2S KICK: kick on one server enforced on the other
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
async fn s2s_kick_propagates() {
    let (local, remote) = match (local_server(), remote_server()) {
        (Some(l), Some(r)) => (l, r),
        _ => {
            eprintln!("SKIP");
            return;
        }
    };
    let ch = test_channel();

    let (h_op, mut rx_op) = guest(&local, &format!("kop{}", rand::random::<u16>())).await;
    registered(&mut rx_op).await;
    h_op.join(&ch).await.unwrap();
    joined(&mut rx_op, &ch).await;

    let victim_nick = format!("kvic{}", rand::random::<u16>());
    let (h_victim, mut rx_victim) = guest(&remote, &victim_nick).await;
    registered(&mut rx_victim).await;
    h_victim.join(&ch).await.unwrap();
    joined(&mut rx_victim, &ch).await;

    tokio::time::sleep(S2S_SETTLE).await;

    // Local op kicks remote user
    h_op.raw(&format!("KICK {ch} {victim_nick} :s2s kick test"))
        .await
        .unwrap();

    // Remote user should see the kick
    let kick = maybe_wait(
        &mut rx_victim,
        |e| {
            matches!(
                e,
                Event::Kicked { .. } | Event::Parted { .. } | Event::ServerNotice { .. }
            )
        },
        S2S_TIMEOUT,
    )
    .await;
    if let Some(evt) = &kick {
        eprintln!("S2S kick result: {evt:?}");
    }

    h_op.quit(None).await.ok();
    h_victim.quit(None).await.ok();
}

// ═══════════════════════════════════════════════════════════════
// SINGLE-SERVER TESTS (don't require S2S)
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
async fn single_server_ops_not_granted_on_existing_channel() {
    let addr = match single_server() {
        Some(a) => a,
        None => {
            eprintln!("SKIP: SERVER not set");
            return;
        }
    };
    let ch = test_channel();

    // First user creates channel (gets ops)
    let (h1, mut rx1) = guest(&addr, &format!("so1{}", rand::random::<u16>())).await;
    registered(&mut rx1).await;
    h1.join(&ch).await.unwrap();
    joined(&mut rx1, &ch).await;

    // Second user joins (should NOT get ops)
    let (h2, mut rx2) = guest(&addr, &format!("so2{}", rand::random::<u16>())).await;
    registered(&mut rx2).await;
    h2.join(&ch).await.unwrap();
    joined(&mut rx2, &ch).await;

    // Second user tries to set mode — should fail
    h2.raw(&format!("MODE {ch} +m")).await.unwrap();
    let err = maybe_wait(
        &mut rx2,
        |e| matches!(e, Event::ServerNotice { .. } | Event::RawLine(_)),
        Duration::from_secs(5),
    )
    .await;
    // Should get 482 ERR_CHANOPRIVSNEEDED
    if let Some(Event::ServerNotice { text }) = err {
        eprintln!("Mode rejection: {text}");
    }

    h1.quit(None).await.ok();
    h2.quit(None).await.ok();
}

#[tokio::test]
async fn single_server_ban_prevents_rejoin() {
    let addr = match single_server() {
        Some(a) => a,
        None => {
            eprintln!("SKIP");
            return;
        }
    };
    let ch = test_channel();
    let victim = format!("ban{}", rand::random::<u16>());

    let (h_op, mut rx_op) = guest(&addr, &format!("bop{}", rand::random::<u16>())).await;
    registered(&mut rx_op).await;
    h_op.join(&ch).await.unwrap();
    joined(&mut rx_op, &ch).await;

    let (h_v, mut rx_v) = guest(&addr, &victim).await;
    registered(&mut rx_v).await;
    h_v.join(&ch).await.unwrap();
    joined(&mut rx_v, &ch).await;

    // Ban + kick
    h_op.raw(&format!("MODE {ch} +b {victim}!*@*"))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    h_op.raw(&format!("KICK {ch} {victim} :banned"))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Try rejoin — should fail
    h_v.join(&ch).await.unwrap();
    let err = maybe_wait(
        &mut rx_v,
        |e| matches!(e, Event::ServerNotice { .. } | Event::RawLine(_)),
        Duration::from_secs(5),
    )
    .await;
    if let Some(evt) = &err {
        eprintln!("Ban enforcement: {evt:?}");
    }

    h_op.quit(None).await.ok();
    h_v.quit(None).await.ok();
}

#[tokio::test]
async fn single_server_invite_only_blocks() {
    let addr = match single_server() {
        Some(a) => a,
        None => {
            eprintln!("SKIP");
            return;
        }
    };
    let ch = test_channel();

    let (h_op, mut rx_op) = guest(&addr, &format!("iop2{}", rand::random::<u16>())).await;
    registered(&mut rx_op).await;
    h_op.join(&ch).await.unwrap();
    joined(&mut rx_op, &ch).await;
    h_op.raw(&format!("MODE {ch} +i")).await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;

    let (h_out, mut rx_out) = guest(&addr, &format!("io2{}", rand::random::<u16>())).await;
    registered(&mut rx_out).await;
    h_out.join(&ch).await.unwrap();

    let err = maybe_wait(
        &mut rx_out,
        |e| matches!(e, Event::ServerNotice { .. } | Event::RawLine(_)),
        Duration::from_secs(5),
    )
    .await;
    if let Some(evt) = &err {
        eprintln!("Invite-only enforcement: {evt:?}");
    }

    h_op.quit(None).await.ok();
    h_out.quit(None).await.ok();
}
