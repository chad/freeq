//! Regression test for the ALPN-stomp bug.
//!
//! Background: `iroh-live`'s `Live::builder(endpoint).with_router().spawn()`
//! calls `Router::spawn` internally, which calls `endpoint.set_alpns(...)`
//! and overwrites whatever ALPN list was registered on the endpoint at bind
//! time. If freeq registers `freeq/iroh/1` and `freeq/s2s/1` via
//! `Endpoint::builder().alpns(...)` and *then* spins up iroh-live with
//! `.with_router()`, the freeq ALPNs get replaced by iroh-live's protocol
//! ALPNs (gossip + moq), and inbound dials with `freeq/s2s/1` are rejected
//! at the TLS layer with `no_application_protocol`.
//!
//! Three tests:
//! 1. `bug_repro_iroh_live_with_router_stomps_freeq_alpns` — pins the bug
//!    in iroh-live so we notice if the upstream behavior ever changes.
//!    Asserts that calling `Live::builder.with_router()` causes a
//!    `freeq/s2s/1` dial to fail.
//! 2. `s2s_alpn_dial_succeeds_with_unified_router` — exercises the fix:
//!    build a single `iroh::protocol::Router` that registers both freeq
//!    ALPNs *and* iroh-live's protocols (via `Live::register_protocols`).
//!    Asserts that `freeq/s2s/1` dials are accepted by TLS.
//! 3. `iroh_client_alpn_dial_succeeds_with_unified_router` — same as (2)
//!    for `freeq/iroh/1`.

#![cfg(feature = "av-native")]

use std::time::Duration;

use iroh::{
    Endpoint,
    endpoint::{Connection, presets},
    protocol::{AcceptError, ProtocolHandler, Router},
};
use tokio::time::timeout;

const FREEQ_IROH_ALPN: &[u8] = b"freeq/iroh/1";
const FREEQ_S2S_ALPN: &[u8] = b"freeq/s2s/1";

/// Trivial ProtocolHandler that just drops the connection cleanly. We
/// only need TLS+ALPN to negotiate; we don't care about the IRC/S2S
/// payload for these tests.
#[derive(Clone, Debug)]
struct AcceptAndDrop;

impl ProtocolHandler for AcceptAndDrop {
    async fn accept(&self, conn: Connection) -> Result<(), AcceptError> {
        conn.close(0u32.into(), b"test ok");
        Ok(())
    }
}

fn install_crypto() {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
}

async fn server_endpoint_with_freeq_alpns() -> Endpoint {
    install_crypto();
    let ep = Endpoint::builder(presets::N0)
        .alpns(vec![FREEQ_IROH_ALPN.to_vec(), FREEQ_S2S_ALPN.to_vec()])
        .bind()
        .await
        .expect("server endpoint bind");
    ep.online().await;
    ep
}

async fn client_endpoint() -> Endpoint {
    install_crypto();
    Endpoint::builder(presets::N0)
        .bind()
        .await
        .expect("client endpoint bind")
}

#[tokio::test]
async fn bug_repro_iroh_live_with_router_stomps_freeq_alpns() {
    tracing_subscriber::fmt()
        .with_env_filter("info,iroh=warn")
        .try_init()
        .ok();

    let server_ep = server_endpoint_with_freeq_alpns().await;
    let server_addr = server_ep.addr();

    // BUG TRIGGER: Live::builder.with_router() spawns a Router that calls
    // endpoint.set_alpns(...) with iroh-live's protocols only, replacing
    // the freeq ALPNs we registered at bind time.
    let _live = iroh_live::Live::builder(server_ep.clone())
        .with_router()
        .with_gossip()
        .spawn();

    let client_ep = client_endpoint().await;
    let dial = timeout(
        Duration::from_secs(15),
        client_ep.connect(server_addr, FREEQ_S2S_ALPN),
    )
    .await;

    let err = match dial {
        Ok(Ok(_)) => panic!(
            "freeq/s2s/1 dial unexpectedly succeeded after \
             Live::with_router() — has iroh-live changed its ALPN \
             registration semantics? Re-evaluate the unified Router fix."
        ),
        Ok(Err(e)) => e,
        Err(_) => panic!("dial timed out (expected an ALPN reject, not a timeout)"),
    };
    assert!(
        err.to_string().contains("error 120") || err.to_string().contains("application protocol"),
        "expected `no_application_protocol` TLS abort, got: {err}"
    );
}

/// Mirrors the unified Router pattern in `crate::iroh::spawn_router`:
/// register freeq's two protocols on a Router we own, then thread it
/// through `Live::register_protocols` so iroh-live's gossip + MoQ are
/// mounted on the same Router. Endpoint ALPN list is set once, with all
/// four protocols.
async fn spawn_unified_router(server_ep: Endpoint) -> (Router, iroh_live::Live) {
    let live = iroh_live::Live::builder(server_ep.clone())
        // Note: NO .with_router() — we build the Router ourselves below.
        .with_gossip()
        .spawn();
    let builder = Router::builder(server_ep)
        .accept(FREEQ_IROH_ALPN, AcceptAndDrop)
        .accept(FREEQ_S2S_ALPN, AcceptAndDrop);
    let builder = live.register_protocols(builder);
    (builder.spawn(), live)
}

#[tokio::test]
async fn s2s_alpn_dial_succeeds_with_unified_router() {
    tracing_subscriber::fmt()
        .with_env_filter("info,iroh=warn")
        .try_init()
        .ok();

    let server_ep = server_endpoint_with_freeq_alpns().await;
    let server_addr = server_ep.addr();
    let (_router, _live) = spawn_unified_router(server_ep).await;

    let client_ep = client_endpoint().await;
    let dial = timeout(
        Duration::from_secs(15),
        client_ep.connect(server_addr, FREEQ_S2S_ALPN),
    )
    .await;

    match dial {
        Ok(Ok(_conn)) => {}
        Ok(Err(e)) => panic!(
            "s2s dial rejected — freeq/s2s/1 is missing from the \
             endpoint's ALPN list: {e}"
        ),
        Err(_) => panic!("s2s dial timed out"),
    }
}

#[tokio::test]
async fn iroh_client_alpn_dial_succeeds_with_unified_router() {
    tracing_subscriber::fmt()
        .with_env_filter("info,iroh=warn")
        .try_init()
        .ok();

    let server_ep = server_endpoint_with_freeq_alpns().await;
    let server_addr = server_ep.addr();
    let (_router, _live) = spawn_unified_router(server_ep).await;

    let client_ep = client_endpoint().await;
    let dial = timeout(
        Duration::from_secs(15),
        client_ep.connect(server_addr, FREEQ_IROH_ALPN),
    )
    .await;

    match dial {
        Ok(Ok(_conn)) => {}
        Ok(Err(e)) => panic!(
            "iroh client dial rejected — freeq/iroh/1 is missing from \
             the endpoint's ALPN list: {e}"
        ),
        Err(_) => panic!("client dial timed out"),
    }
}
