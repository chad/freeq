//! Media backend for AV sessions.
//!
//! With `av-native` feature: real iroh-live rooms for native client audio.
//! Without: stub backend, browser audio uses WebRTC (signaled through IRC TAGMSG).

use std::collections::HashMap;

pub type MediaTicket = String;

pub trait MediaBackend: Send + Sync {
    fn create_room(
        &self,
        session_id: &str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<MediaTicket, String>> + Send + '_>>;

    fn close_room(
        &self,
        session_id: &str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send + '_>>;
}

// ── Real iroh-live backend (av-native feature) ─────────────────────

#[cfg(feature = "av-native")]
pub struct IrohLiveBackend {
    live: std::sync::Arc<iroh_live::Live>,
    rooms: parking_lot::Mutex<HashMap<String, ActiveRoom>>,
    /// Room events waiting to be consumed by the bridge.
    pending_events: parking_lot::Mutex<HashMap<String, iroh_live::rooms::RoomEvents>>,
}

#[cfg(feature = "av-native")]
pub struct ActiveRoom {
    pub ticket: String,
    pub handle: iroh_live::rooms::RoomHandle,
}

#[cfg(feature = "av-native")]
impl IrohLiveBackend {
    pub async fn new(endpoint: iroh::Endpoint) -> Result<Self, String> {
        // NB: do NOT call .with_router() here. iroh-live's internal Router
        // calls `endpoint.set_alpns(...)` which would replace the freeq
        // ALPNs (`freeq/iroh/1`, `freeq/s2s/1`) registered at endpoint bind
        // time, breaking inbound client + S2S dials with TLS
        // `no_application_protocol`. The unified Router is built in
        // `crate::iroh::spawn_router`, which calls
        // `Live::register_protocols` on the shared RouterBuilder so
        // gossip + MoQ are mounted alongside freeq's protocols.
        let live = iroh_live::Live::builder(endpoint).with_gossip().spawn();
        tracing::info!(
            endpoint_id = %live.endpoint().id(),
            "iroh-live media backend initialized"
        );
        Ok(Self {
            live: std::sync::Arc::new(live),
            rooms: parking_lot::Mutex::new(HashMap::new()),
            pending_events: parking_lot::Mutex::new(HashMap::new()),
        })
    }

    pub fn live(&self) -> &iroh_live::Live {
        &self.live
    }

    /// Take the Room handle and events for a session (for bridge setup).
    pub fn take_room_for_bridge(
        &self,
        session_id: &str,
    ) -> Option<(iroh_live::rooms::RoomHandle, iroh_live::rooms::RoomEvents)> {
        let handle = self.rooms.lock().get(session_id)?.handle.clone();
        let events = self.pending_events.lock().remove(session_id)?;
        Some((handle, events))
    }
}

#[cfg(feature = "av-native")]
impl MediaBackend for IrohLiveBackend {
    fn create_room(
        &self,
        session_id: &str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<MediaTicket, String>> + Send + '_>>
    {
        let session_id = session_id.to_string();
        let live = self.live.clone();
        Box::pin(async move {
            let ticket = iroh_live::rooms::RoomTicket::generate();
            let room = iroh_live::rooms::Room::new(&live, ticket.clone())
                .await
                .map_err(|e| format!("Failed to create room: {e}"))?;

            let ticket_str = room.ticket().to_string();
            let (events, handle) = room.split();

            // Store room — events are consumed by the bridge (av_bridge.rs)
            self.rooms.lock().insert(
                session_id.clone(),
                ActiveRoom {
                    ticket: ticket_str.clone(),
                    handle: handle.clone(),
                },
            );

            // Store events separately for the bridge to consume
            self.pending_events
                .lock()
                .insert(session_id.clone(), events);

            tracing::info!(session_id = %session_id, ticket = %ticket_str, "Created iroh-live media room");
            Ok(ticket_str)
        })
    }

    fn close_room(
        &self,
        session_id: &str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send + '_>> {
        let removed = self.rooms.lock().remove(session_id);
        let sid = session_id.to_string();
        Box::pin(async move {
            if removed.is_some() {
                tracing::info!(session_id = %sid, "Closed iroh-live media room");
            }
            Ok(())
        })
    }
}

// ── Stub backend (no av-native feature) ────────────────────────────

#[cfg(not(feature = "av-native"))]
pub struct IrohLiveBackend {
    rooms: parking_lot::Mutex<HashMap<String, String>>,
}

#[cfg(not(feature = "av-native"))]
impl Default for IrohLiveBackend {
    fn default() -> Self {
        Self::new()
    }
}

// Stub backend used only when `av-native` is OFF. Without this gate it also
// compiles under `av-native`, colliding with the async `new(endpoint)` above
// (duplicate `new`) and failing to set the av-native-only
// `live`/`pending_events` fields — which broke the production build.
#[cfg(not(feature = "av-native"))]
impl IrohLiveBackend {
    pub fn new() -> Self {
        Self {
            rooms: parking_lot::Mutex::new(HashMap::new()),
        }
    }
}

#[cfg(not(feature = "av-native"))]
impl MediaBackend for IrohLiveBackend {
    fn create_room(
        &self,
        session_id: &str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<MediaTicket, String>> + Send + '_>>
    {
        let session_id = session_id.to_string();
        Box::pin(async move {
            let ticket = format!("webrtc://{session_id}");
            self.rooms.lock().insert(session_id.clone(), ticket.clone());
            tracing::info!(session_id = %session_id, "Created AV room (WebRTC signaling only)");
            Ok(ticket)
        })
    }

    fn close_room(
        &self,
        session_id: &str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send + '_>> {
        self.rooms.lock().remove(session_id);
        let sid = session_id.to_string();
        Box::pin(async move {
            tracing::info!(session_id = %sid, "Closed AV room");
            Ok(())
        })
    }
}

// ── Server init (feature-gated) ────────────────────────────────────

#[cfg(feature = "av-native")]
pub async fn init_backend(endpoint: iroh::Endpoint) -> Option<std::sync::Arc<IrohLiveBackend>> {
    match IrohLiveBackend::new(endpoint).await {
        Ok(backend) => {
            tracing::info!("iroh-live AV media backend initialized (native audio enabled)");
            Some(std::sync::Arc::new(backend))
        }
        Err(e) => {
            tracing::warn!("Failed to init iroh-live: {e}");
            None
        }
    }
}

#[cfg(not(feature = "av-native"))]
pub fn init_backend_stub() -> std::sync::Arc<IrohLiveBackend> {
    tracing::info!("AV media backend initialized (WebRTC signaling only)");
    std::sync::Arc::new(IrohLiveBackend::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn stub_create_and_close() {
        #[cfg(not(feature = "av-native"))]
        {
            let backend = IrohLiveBackend::new();
            let ticket = MediaBackend::create_room(&backend, "test").await.unwrap();
            assert!(ticket.contains("test"));
            MediaBackend::close_room(&backend, "test").await.unwrap();
        }
    }
}
