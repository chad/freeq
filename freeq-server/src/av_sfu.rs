//! AV SFU (Selective Forwarding Unit).
//!
//! Accepts MoQ connections via:
//! - QUIC/WebTransport (direct UDP, for native clients or when ports are exposed)
//! - WebSocket (through the HTTP server, works through any reverse proxy)
//!
//! Uses moq_relay::Cluster to route audio streams between all participants.

#[cfg(feature = "av-native")]
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

/// Shared SFU state, accessible from the web server for WebSocket MoQ connections.
#[cfg(feature = "av-native")]
pub struct SfuState {
    pub cluster: moq_relay::Cluster,
    pub auth: moq_relay::Auth,
    pub conn_id: AtomicU64,
}

/// Initialize the SFU cluster and return shared state.
/// Also spawns the QUIC accept loop if a port is provided.
#[cfg(feature = "av-native")]
pub async fn init_sfu(quic_port: Option<u16>) -> anyhow::Result<Arc<SfuState>> {
    use moq_relay::{Auth, AuthConfig, Cluster, ClusterConfig};

    // QUIC server config (also used for cluster's internal client)
    let mut client_config = moq_native::ClientConfig::default();
    client_config.max_streams = Some(moq_relay::DEFAULT_MAX_STREAMS);
    let client = client_config.init()?;

    let mut auth_config = AuthConfig::default();
    auth_config.public = Some("/".to_string()); // All paths public for staging
    let auth = Auth::new(auth_config).await?;

    let cluster = Cluster::new(ClusterConfig::default(), client);
    let cluster_run = cluster.clone();
    tokio::spawn(async move {
        if let Err(e) = cluster_run.run().await {
            tracing::error!("SFU cluster failed: {e}");
        }
    });

    let state = Arc::new(SfuState {
        cluster,
        auth,
        conn_id: AtomicU64::new(0),
    });

    // Optionally start QUIC accept loop (for direct connections bypassing HTTP proxy)
    if let Some(port) = quic_port {
        let state2 = state.clone();
        tokio::spawn(async move {
            if let Err(e) = run_quic_accept(port, state2).await {
                // QUIC is optional — WebSocket MoQ still works without it
                tracing::warn!("SFU QUIC listener failed (WebSocket still active): {e}");
            }
        });
    }

    tracing::info!("AV SFU initialized (WebSocket enabled)");
    Ok(state)
}

#[cfg(feature = "av-native")]
async fn run_quic_accept(port: u16, state: Arc<SfuState>) -> anyhow::Result<()> {
    let mut server_config = moq_native::ServerConfig::default();
    server_config.bind = Some(format!("[::]:{port}").parse()?);
    server_config.backend = Some(moq_native::QuicBackend::Noq);
    server_config.max_streams = Some(moq_relay::DEFAULT_MAX_STREAMS);

    // QUIC/WebTransport TLS. With a publicly-trusted cert (FREEQ_AV_TLS_CERT
    // / FREEQ_AV_TLS_KEY) browsers can WebTransport straight to this
    // listener — the proper low-latency media transport. Without it we
    // fall back to a self-signed cert, which only native clients (cert
    // verification disabled) can use; browsers are stuck on the staticky
    // MoQ-over-WebSocket path. See docs/AV-QUIC-MIGRATION.md.
    match (
        std::env::var("FREEQ_AV_TLS_CERT"),
        std::env::var("FREEQ_AV_TLS_KEY"),
    ) {
        (Ok(cert), Ok(key)) => {
            tracing::info!(%cert, %key, "AV SFU QUIC: using configured TLS cert");
            server_config.tls.cert = vec![cert.into()];
            server_config.tls.key = vec![key.into()];
        }
        _ => {
            tracing::warn!(
                "AV SFU QUIC: FREEQ_AV_TLS_CERT/KEY unset — self-signed cert \
                 (native clients only; browsers cannot WebTransport)"
            );
            server_config.tls.generate = vec!["localhost".to_string()];
        }
    }

    let mut server = server_config.init()?;
    tracing::info!("AV SFU QUIC on :{port} (WebTransport + MoQ)");

    while let Some(request) = server.accept().await {
        let id = state.conn_id.fetch_add(1, Ordering::Relaxed);
        let cluster = state.cluster.clone();
        let auth = state.auth.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_quic_connection(id, request, cluster, auth).await {
                tracing::debug!(conn = id, "SFU QUIC session ended: {e}");
            }
        });
    }

    Ok(())
}

#[cfg(feature = "av-native")]
async fn handle_quic_connection(
    id: u64,
    request: moq_native::Request,
    cluster: moq_relay::Cluster,
    auth: moq_relay::Auth,
) -> anyhow::Result<()> {
    use moq_relay::AuthParams;

    let transport = request.transport();
    // Root EVERY connection at the cluster root (""), regardless of the URL
    // path the client dialed (e.g. "/av/moq"). The WebSocket entry point
    // (`handle_ws_moq`) already roots at "". If QUIC instead rooted at the
    // URL path, native (QUIC) and web (WebSocket) clients would publish into
    // DISJOINT namespaces inside the SAME `moq_relay::Cluster` and never see
    // each other — observed live: an iOS QUIC client and a web WS client in
    // one `#chadtest` session, mutually invisible (no audio, no video, not
    // even in each other's participant list). `AuthParams::from_url` still
    // parses any jwt/register query params; we only force the root path so
    // both transports share one broadcast namespace.
    let params = match request.url() {
        Some(url) => {
            let mut p = AuthParams::from_url(url);
            p.path = String::new();
            p
        }
        None => AuthParams::default(),
    };

    let token = auth.verify(&params)?;
    let publish = cluster.publisher(&token);
    let subscribe = cluster.subscriber(&token);
    let _registration = cluster.register(&token);

    tracing::info!(conn = id, %transport, "SFU: client connected (QUIC)");

    let mut request = request;
    if let Some(p) = publish {
        request = request.with_consume(p);
    }
    if let Some(s) = subscribe {
        request = request.with_publish(s);
    }
    let session = request.ok().await?;

    tracing::info!(conn = id, "SFU: session active");
    let _ = session.closed().await;
    tracing::info!(conn = id, "SFU: session closed");

    Ok(())
}

/// Handle a WebSocket upgrade for MoQ — called from the web server's route handler.
#[cfg(feature = "av-native")]
pub async fn handle_ws_moq(
    state: Arc<SfuState>,
    path: String,
    socket: axum::extract::ws::WebSocket,
) {
    use futures::{SinkExt, StreamExt};

    let id = state.conn_id.fetch_add(1, Ordering::Relaxed);

    let params = moq_relay::AuthParams {
        path,
        jwt: None,
        register: None,
    };

    let token = match state.auth.verify(&params) {
        Ok(t) => t,
        Err(e) => {
            tracing::warn!(conn = id, "SFU WS auth failed: {e}");
            return;
        }
    };

    let publish = state.cluster.publisher(&token);
    let subscribe = state.cluster.subscriber(&token);
    let _registration = state.cluster.register(&token);

    // Convert axum WebSocket to tungstenite format for qmux
    let socket = socket
        .map(axum_to_tungstenite)
        .sink_map_err(|err| {
            tracing::warn!(%err, "WebSocket error");
            qmux::tungstenite::Error::ConnectionClosed
        })
        .with(tungstenite_to_axum);

    let ws = qmux::ws::accept(socket, None);
    // moq_lite::Server semantics (opposite of moq_native::Request):
    //   with_publish(subscribe) = send cluster's subscriber stream TO the client
    //   with_consume(publish) = consume client's stream and feed INTO cluster publisher
    let session = match moq_lite::Server::new()
        .with_publish(subscribe)
        .with_consume(publish)
        .accept(ws)
        .await
    {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(conn = id, "SFU WS session setup failed: {e}");
            return;
        }
    };

    tracing::info!(conn = id, "SFU: client connected (WebSocket)");
    let _ = session.closed().await;
    tracing::info!(conn = id, "SFU: session closed (WebSocket)");
}

// ── WebSocket message conversion (axum ↔ tungstenite) ─────────────

#[cfg(feature = "av-native")]
#[allow(clippy::result_large_err)]
fn axum_to_tungstenite(
    message: Result<axum::extract::ws::Message, axum::Error>,
) -> Result<qmux::tungstenite::Message, qmux::tungstenite::Error> {
    use qmux::tungstenite;
    match message {
        Ok(msg) => Ok(match msg {
            axum::extract::ws::Message::Text(text) => {
                tungstenite::Message::Text(text.to_string().into())
            }
            axum::extract::ws::Message::Binary(bin) => {
                tungstenite::Message::Binary(Vec::from(bin).into())
            }
            axum::extract::ws::Message::Ping(ping) => {
                tungstenite::Message::Ping(Vec::from(ping).into())
            }
            axum::extract::ws::Message::Pong(pong) => {
                tungstenite::Message::Pong(Vec::from(pong).into())
            }
            axum::extract::ws::Message::Close(close) => {
                tungstenite::Message::Close(close.map(|c| tungstenite::protocol::CloseFrame {
                    code: c.code.into(),
                    reason: c.reason.to_string().into(),
                }))
            }
        }),
        Err(_err) => Err(qmux::tungstenite::Error::ConnectionClosed),
    }
}

#[cfg(feature = "av-native")]
#[allow(clippy::result_large_err)]
fn tungstenite_to_axum(
    message: qmux::tungstenite::Message,
) -> std::pin::Pin<
    Box<
        dyn std::future::Future<
                Output = Result<axum::extract::ws::Message, qmux::tungstenite::Error>,
            > + Send
            + Sync,
    >,
> {
    use qmux::tungstenite;
    Box::pin(async move {
        Ok(match message {
            tungstenite::Message::Text(text) => {
                axum::extract::ws::Message::Text(text.to_string().into())
            }
            tungstenite::Message::Binary(bin) => {
                axum::extract::ws::Message::Binary(Vec::from(bin).into())
            }
            tungstenite::Message::Ping(ping) => {
                axum::extract::ws::Message::Ping(Vec::from(ping).into())
            }
            tungstenite::Message::Pong(pong) => {
                axum::extract::ws::Message::Pong(Vec::from(pong).into())
            }
            tungstenite::Message::Frame(_) => unreachable!(),
            tungstenite::Message::Close(close) => {
                axum::extract::ws::Message::Close(close.map(|c| axum::extract::ws::CloseFrame {
                    code: c.code.into(),
                    reason: c.reason.to_string().into(),
                }))
            }
        })
    })
}
