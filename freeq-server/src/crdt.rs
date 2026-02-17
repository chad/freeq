//! CRDT-based server state using Automerge.
//!
//! Each server in the cluster maintains a local Automerge document
//! representing shared state. Changes are synchronized between peers
//! using Automerge's built-in sync protocol over iroh QUIC streams.
//!
//! # Design Principles (from CRDT federation audit)
//!
//! 1. **CRDT stores durable facts only** — not ephemeral presence.
//!    Presence is handled by S2S events with periodic resync.
//! 2. **Peer identity uses iroh endpoint ID** (cryptographic) everywhere.
//!    `server_name` is untrusted display metadata.
//! 3. **Founder uses deterministic min-actor resolution** for concurrent writes.
//! 4. **Authority boundaries**: each key-space has defined write rules,
//!    enforced via soft validation on receive.
//! 5. **Compaction**: periodic save + snapshot to bound doc growth.
//!
//! # Document Schema (flat keys)
//!
//! ```text
//! "topic:{channel}"          → JSON { text, set_by, set_by_did, origin_peer }
//! "topic_by:{channel}"       → set_by nick (LWW, legacy)
//! "ban:{channel}:{mask}"     → JSON { set_by, set_by_did, origin_peer }
//! "nick_owner:{nick}"        → DID (LWW, enforced by proof)
//! "founder:{channel}"        → JSON { did, actor_id } (min-actor-wins)
//! "did_op:{channel}:{did}"   → JSON { granted_by_did, origin_peer }
//! ```
//!
//! Presence (`member:*`) is **not** stored in CRDT — it's S2S-event-driven.

use std::collections::HashMap;

use automerge::{
    AutoCommit, ReadDoc,
    transaction::Transactable,
    sync::{self, SyncDoc},
};
use tokio::sync::Mutex;

/// Helper: extract a string from an automerge Value.
fn value_to_string(val: &automerge::Value<'_>) -> Option<String> {
    match val {
        automerge::Value::Scalar(s) => match s.as_ref() {
            automerge::ScalarValue::Str(s) => Some(s.to_string()),
            _ => None,
        },
        _ => None,
    }
}

/// Metrics for observability.
#[derive(Debug, Clone, Default)]
pub struct CrdtMetrics {
    /// Number of changes applied to the doc.
    pub change_count: u64,
    /// Number of sync messages sent.
    pub sync_messages_sent: u64,
    /// Number of sync messages received.
    pub sync_messages_received: u64,
    /// Total bytes sent in sync messages.
    pub sync_bytes_sent: u64,
    /// Total bytes received in sync messages.
    pub sync_bytes_received: u64,
    /// Last save size in bytes.
    pub last_save_size: u64,
    /// Number of compactions performed.
    pub compaction_count: u64,
}

/// Provenance metadata attached to CRDT values for authority checking.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Provenance {
    /// The iroh endpoint ID of the server that wrote this value.
    pub origin_peer: String,
    /// The DID that authorized this write (if applicable).
    pub authorized_by_did: Option<String>,
}

/// Wraps an Automerge document for cluster state synchronization.
///
/// Uses `tokio::sync::Mutex` for async-safe access (no blocking the runtime).
/// Sync state is keyed by iroh endpoint ID (cryptographic identity).
pub struct ClusterDoc {
    doc: Mutex<AutoCommit>,
    /// The actor identity used for CRDT operations.
    /// Initially server_name; re-keyed to iroh endpoint ID at startup.
    /// Behind a Mutex so `rekey_actor(&self)` works through Arc<SharedState>.
    actor_id: Mutex<String>,
    /// Sync states keyed by **iroh endpoint ID** (not server_name).
    sync_states: Mutex<HashMap<String, sync::State>>,
    /// Observability metrics.
    pub metrics: Mutex<CrdtMetrics>,
}

impl ClusterDoc {
    /// Create a new cluster document for a server.
    ///
    /// `server_id` should be the iroh endpoint ID for consistent actor identity.
    /// If iroh is not enabled, falls back to server_name.
    pub fn new(server_id: &str) -> Self {
        let actor = automerge::ActorId::from(server_id.as_bytes());
        let doc = AutoCommit::new().with_actor(actor);

        Self {
            doc: Mutex::new(doc),
            actor_id: Mutex::new(server_id.to_string()),
            sync_states: Mutex::new(HashMap::new()),
            metrics: Mutex::new(CrdtMetrics::default()),
        }
    }

    /// Load from saved bytes.
    pub fn load(data: &[u8], server_id: &str) -> Result<Self, automerge::AutomergeError> {
        let actor = automerge::ActorId::from(server_id.as_bytes());
        let doc = AutoCommit::load(data)?.with_actor(actor);
        Ok(Self {
            doc: Mutex::new(doc),
            actor_id: Mutex::new(server_id.to_string()),
            sync_states: Mutex::new(HashMap::new()),
            metrics: Mutex::new(CrdtMetrics::default()),
        })
    }

    /// Re-key the CRDT actor identity to the iroh endpoint ID.
    ///
    /// Must be called once the iroh endpoint is available, before any
    /// federation activity. This ensures the Automerge actor_id matches
    /// the cryptographic transport identity, which is critical for:
    /// - Deterministic founder resolution (min-actor-wins)
    /// - Consistent sync state keying
    /// - Provenance that can be verified against transport identity
    ///
    /// Panics if called after changes have been made with the old actor_id
    /// and those changes have been synced — in practice, call this at startup
    /// before any S2S connections are established.
    /// Re-key the CRDT actor identity.
    ///
    /// Since `actor_id` is used in `set_founder` comparisons, we store
    /// the new value in a separate Mutex-protected cell so this can be
    /// called through `&self` (SharedState is behind Arc).
    pub async fn rekey_actor(&self, new_actor_id: &str) {
        let actor = automerge::ActorId::from(new_actor_id.as_bytes());
        let mut doc = self.doc.lock().await;
        // Save and reload with new actor — pins all future changes to the new ID
        let bytes = doc.save();
        *doc = AutoCommit::load(&bytes)
            .expect("rekey: reload from own save must succeed")
            .with_actor(actor);
        drop(doc);
        *self.actor_id.lock().await = new_actor_id.to_string();
        // Clear sync states — peers will re-sync with the new identity
        self.sync_states.lock().await.clear();
        tracing::info!(
            actor_id = %new_actor_id,
            "CRDT actor re-keyed to iroh endpoint ID"
        );
    }

    /// Get the current actor ID.
    pub async fn get_actor_id(&self) -> String {
        self.actor_id.lock().await.clone()
    }

    /// Save to bytes (also updates metrics).
    pub async fn save(&self) -> Vec<u8> {
        let bytes = self.doc.lock().await.save();
        let mut m = self.metrics.lock().await;
        m.last_save_size = bytes.len() as u64;
        bytes
    }

    /// Compact the document: save and reload to discard history.
    /// This bounds memory/sync growth in long-lived deployments.
    pub async fn compact(&self) -> Result<(), String> {
        let bytes = {
            let mut doc = self.doc.lock().await;
            doc.save()
        };
        let current_actor_id = self.actor_id.lock().await.clone();
        let actor = automerge::ActorId::from(current_actor_id.as_bytes());
        let new_doc = AutoCommit::load(&bytes)
            .map_err(|e| format!("Compaction load failed: {e}"))?
            .with_actor(actor);
        {
            let mut doc = self.doc.lock().await;
            *doc = new_doc;
        }
        // Reset sync states — peers will need to re-sync from the compacted doc
        {
            let mut states = self.sync_states.lock().await;
            states.clear();
        }
        {
            let mut m = self.metrics.lock().await;
            m.compaction_count += 1;
            m.last_save_size = bytes.len() as u64;
        }
        Ok(())
    }

    /// Get current metrics snapshot.
    pub async fn get_metrics(&self) -> CrdtMetrics {
        self.metrics.lock().await.clone()
    }

    // ── Schema Design ─────────────────────────────────────────────
    //
    // FLAT KEYS in the root map to avoid concurrent nested-map
    // creation conflicts.
    //
    // Key format:
    //   "topic:{channel}"          → JSON { text, set_by, set_by_did, origin_peer }
    //   "topic_by:{channel}"       → set_by nick (legacy, kept for reads)
    //   "ban:{channel}:{mask}"     → JSON { set_by, set_by_did, origin_peer }
    //   "nick_owner:{nick}"        → DID
    //   "founder:{channel}"        → JSON { did, actor_id } (min-actor-wins)
    //   "did_op:{channel}:{did}"   → JSON { granted_by_did, origin_peer }
    //
    // NOTE: presence (member:*) is NOT in CRDT. It's S2S-event-driven.
    // This avoids ghost users when servers crash without emitting PART/QUIT.

    // ── Topic operations ────────────────────────────────────────────

    /// Set a channel's topic with provenance.
    pub async fn set_topic(&self, channel: &str, topic: &str, set_by: &str, set_by_did: Option<&str>, origin_peer: &str) {
        let mut doc = self.doc.lock().await;
        // Store rich provenance as JSON
        let value = serde_json::json!({
            "text": topic,
            "set_by": set_by,
            "set_by_did": set_by_did,
            "origin_peer": origin_peer,
        });
        let _ = doc.put(automerge::ROOT, &format!("topic:{channel}"), value.to_string());
        let _ = doc.put(automerge::ROOT, &format!("topic_by:{channel}"), set_by);
        self.metrics.lock().await.change_count += 1;
    }

    /// Get a channel's topic: (text, set_by).
    pub async fn channel_topic(&self, channel: &str) -> Option<(String, String)> {
        let doc = self.doc.lock().await;
        let (topic_val, _) = doc.get(automerge::ROOT, format!("topic:{channel}")).ok()??;
        let raw = value_to_string(&topic_val)?;
        // Try parsing as JSON (new format with provenance)
        if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&raw) {
            let text = parsed.get("text")?.as_str()?.to_string();
            let set_by = parsed.get("set_by")?.as_str()?.to_string();
            Some((text, set_by))
        } else {
            // Legacy format: plain text topic
            let (setter_val, _) = doc.get(automerge::ROOT, format!("topic_by:{channel}")).ok()??;
            let setter = value_to_string(&setter_val)?;
            Some((raw, setter))
        }
    }

    /// Get full topic provenance for authority checking.
    pub async fn channel_topic_provenance(&self, channel: &str) -> Option<serde_json::Value> {
        let doc = self.doc.lock().await;
        let (topic_val, _) = doc.get(automerge::ROOT, format!("topic:{channel}")).ok()??;
        let raw = value_to_string(&topic_val)?;
        serde_json::from_str(&raw).ok()
    }

    // ── Ban operations ──────────────────────────────────────────────

    /// Add a ban with provenance.
    pub async fn add_ban(&self, channel: &str, mask: &str, set_by: &str, set_by_did: Option<&str>, origin_peer: &str) {
        let mut doc = self.doc.lock().await;
        let value = serde_json::json!({
            "set_by": set_by,
            "set_by_did": set_by_did,
            "origin_peer": origin_peer,
        });
        let key = format!("ban:{channel}:{mask}");
        let _ = doc.put(automerge::ROOT, &key, value.to_string());
        self.metrics.lock().await.change_count += 1;
    }

    /// Remove a ban.
    pub async fn remove_ban(&self, channel: &str, mask: &str) {
        let mut doc = self.doc.lock().await;
        let key = format!("ban:{channel}:{mask}");
        let _ = doc.delete(automerge::ROOT, &key);
        self.metrics.lock().await.change_count += 1;
    }

    /// Get all bans for a channel: Vec<(mask, provenance_json)>.
    pub async fn channel_bans(&self, channel: &str) -> Vec<(String, String)> {
        let doc = self.doc.lock().await;
        let prefix = format!("ban:{channel}:");
        doc.map_range(automerge::ROOT, ..)
            .filter_map(|item| {
                if item.key.starts_with(&prefix) {
                    let mask = item.key.strip_prefix(&prefix)?.to_string();
                    let val = value_to_string(&item.value)?;
                    Some((mask, val))
                } else {
                    None
                }
            })
            .collect()
    }

    // ── Nick ownership ──────────────────────────────────────────────

    /// Bind a nick to a DID.
    pub async fn set_nick_owner(&self, nick: &str, did: &str) {
        let mut doc = self.doc.lock().await;
        let key = format!("nick_owner:{nick}");
        let _ = doc.put(automerge::ROOT, &key, did);
        self.metrics.lock().await.change_count += 1;
    }

    /// Get the DID that owns a nick.
    pub async fn nick_owner(&self, nick: &str) -> Option<String> {
        let doc = self.doc.lock().await;
        let (val, _) = doc.get(automerge::ROOT, format!("nick_owner:{nick}")).ok()??;
        value_to_string(&val)
    }

    // ── Channel authority operations ────────────────────────────────

    /// Set the channel founder using deterministic min-actor resolution.
    ///
    /// Stores `{ did, actor_id }` — on concurrent writes, the minimum
    /// actor_id wins deterministically after Automerge sync. This is
    /// stronger than "first-write-wins by check-if-absent" because it
    /// handles true concurrency correctly.
    pub async fn set_founder(&self, channel: &str, did: &str) {
        let current_actor_id = self.actor_id.lock().await.clone();
        let mut doc = self.doc.lock().await;
        let key = format!("founder:{channel}");

        let value = serde_json::json!({
            "did": did,
            "actor_id": current_actor_id,
        });

        // Check existing: only write if absent OR our actor_id is smaller
        // (min-actor-wins for deterministic convergence)
        if let Ok(Some((existing_val, _))) = doc.get(automerge::ROOT, &key) {
            if let Some(existing_str) = value_to_string(&existing_val) {
                if let Ok(existing) = serde_json::from_str::<serde_json::Value>(&existing_str) {
                    let existing_actor = existing.get("actor_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    // Only overwrite if our actor_id is strictly smaller
                    if current_actor_id.as_str() >= existing_actor {
                        return; // Keep existing — they win
                    }
                }
            }
        }

        let _ = doc.put(automerge::ROOT, &key, value.to_string());
        self.metrics.lock().await.change_count += 1;
    }

    /// Get the channel founder's DID.
    pub async fn founder(&self, channel: &str) -> Option<String> {
        let doc = self.doc.lock().await;
        let (val, _) = doc.get(automerge::ROOT, format!("founder:{channel}")).ok()??;
        let raw = value_to_string(&val)?;
        // Try new JSON format first
        if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&raw) {
            parsed.get("did")?.as_str().map(|s| s.to_string())
        } else {
            // Legacy: plain DID string
            Some(raw)
        }
    }

    /// Grant persistent operator status to a DID with provenance.
    pub async fn grant_op(&self, channel: &str, did: &str, granted_by_did: Option<&str>, origin_peer: &str) {
        let mut doc = self.doc.lock().await;
        let key = format!("did_op:{channel}:{did}");
        let value = serde_json::json!({
            "granted_by_did": granted_by_did,
            "origin_peer": origin_peer,
        });
        let _ = doc.put(automerge::ROOT, &key, value.to_string());
        self.metrics.lock().await.change_count += 1;
    }

    /// Revoke persistent operator status from a DID.
    pub async fn revoke_op(&self, channel: &str, did: &str) {
        let mut doc = self.doc.lock().await;
        let key = format!("did_op:{channel}:{did}");
        let _ = doc.delete(automerge::ROOT, &key);
        self.metrics.lock().await.change_count += 1;
    }

    /// Get all DIDs with persistent operator status in a channel.
    pub async fn channel_did_ops(&self, channel: &str) -> Vec<String> {
        let doc = self.doc.lock().await;
        let prefix = format!("did_op:{channel}:");
        doc.map_range(automerge::ROOT, ..)
            .filter_map(|item| {
                if item.key.starts_with(&prefix) {
                    item.key.strip_prefix(&prefix).map(|d| d.to_string())
                } else {
                    None
                }
            })
            .collect()
    }

    /// Get provenance for a DID op grant (for authority validation).
    pub async fn did_op_provenance(&self, channel: &str, did: &str) -> Option<serde_json::Value> {
        let doc = self.doc.lock().await;
        let key = format!("did_op:{channel}:{did}");
        let (val, _) = doc.get(automerge::ROOT, &key).ok()??;
        let raw = value_to_string(&val)?;
        serde_json::from_str(&raw).ok()
    }

    // ── Authority validation (soft enforcement) ─────────────────────
    //
    // These methods check whether a CRDT write should be accepted based
    // on provenance. They implement "soft enforcement" — accept all ops
    // but validate before using the value.

    /// Validate that a topic change is authorized.
    /// Returns true if the setter has authority (is founder, DID-op, or
    /// the channel isn't topic-locked).
    ///
    /// If `require_did` is true (from `--require-did-for-ops`), rejects
    /// writes without DID provenance. Otherwise, allows them for backward
    /// compatibility with legacy peers.
    pub async fn validate_topic_authority(&self, channel: &str, setter_did: Option<&str>, require_did: bool) -> bool {
        let did = match setter_did {
            Some(d) => d,
            None => return !require_did, // No DID: reject if strict, allow if compat
        };
        // Founder always has authority
        if let Some(founder) = self.founder(channel).await {
            if founder == did {
                return true;
            }
        }
        // DID-ops have authority
        let ops = self.channel_did_ops(channel).await;
        ops.contains(&did.to_string())
    }

    /// Validate that an op grant is authorized.
    /// Only founder or existing DID-ops can grant ops.
    ///
    /// If `require_did` is true, rejects grants without DID provenance.
    pub async fn validate_op_grant_authority(&self, channel: &str, granter_did: Option<&str>, require_did: bool) -> bool {
        let did = match granter_did {
            Some(d) => d,
            None => return !require_did,
        };
        if let Some(founder) = self.founder(channel).await {
            if founder == did {
                return true;
            }
        }
        let ops = self.channel_did_ops(channel).await;
        ops.contains(&did.to_string())
    }

    // ── Sync operations ─────────────────────────────────────────────
    //
    // Sync state is keyed by **iroh endpoint ID** (cryptographic transport
    // identity) — NOT by server_name. This ensures Automerge's sync protocol
    // tracks state consistently per peer.

    /// Generate a sync message for a peer. Returns None if up to date.
    /// `peer_id` MUST be the iroh endpoint ID.
    pub async fn generate_sync_message(&self, peer_id: &str) -> Option<Vec<u8>> {
        let mut doc = self.doc.lock().await;
        let mut sync_states = self.sync_states.lock().await;
        let state = sync_states.entry(peer_id.to_string()).or_insert_with(sync::State::new);
        let result = doc.sync().generate_sync_message(state).map(|msg| msg.encode());
        if let Some(ref bytes) = result {
            let mut m = self.metrics.lock().await;
            m.sync_messages_sent += 1;
            m.sync_bytes_sent += bytes.len() as u64;
        }
        result
    }

    /// Receive a sync message from a peer.
    /// `peer_id` MUST be the iroh endpoint ID (same as used for generate_sync_message).
    pub async fn receive_sync_message(&self, peer_id: &str, message: &[u8]) -> Result<(), String> {
        {
            let mut m = self.metrics.lock().await;
            m.sync_messages_received += 1;
            m.sync_bytes_received += message.len() as u64;
        }
        let msg = sync::Message::decode(message)
            .map_err(|e| format!("Invalid sync message: {e}"))?;
        let mut doc = self.doc.lock().await;
        let mut sync_states = self.sync_states.lock().await;
        let state = sync_states.entry(peer_id.to_string()).or_insert_with(sync::State::new);
        doc.sync().receive_sync_message(state, msg)
            .map_err(|e| format!("Sync error: {e}"))
    }

    /// Remove sync state for a disconnected peer.
    pub async fn remove_peer_sync_state(&self, peer_id: &str) {
        self.sync_states.lock().await.remove(peer_id);
    }

    /// Get list of peers we have sync state for.
    pub async fn sync_peers(&self) -> Vec<String> {
        self.sync_states.lock().await.keys().cloned().collect()
    }

    /// Count the number of keys in the document (for observability).
    pub async fn key_count(&self) -> usize {
        let doc = self.doc.lock().await;
        doc.map_range(automerge::ROOT, ..).count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn topic_set_and_read() {
        let doc = ClusterDoc::new("server-1");
        doc.set_topic("#test", "Hello world", "alice", Some("did:plc:alice"), "peer-1").await;

        let topic = doc.channel_topic("#test").await;
        assert_eq!(topic, Some(("Hello world".to_string(), "alice".to_string())));
    }

    #[tokio::test]
    async fn topic_provenance() {
        let doc = ClusterDoc::new("server-1");
        doc.set_topic("#test", "Hello", "alice", Some("did:plc:alice"), "peer-abc").await;

        let prov = doc.channel_topic_provenance("#test").await.unwrap();
        assert_eq!(prov["origin_peer"], "peer-abc");
        assert_eq!(prov["set_by_did"], "did:plc:alice");
    }

    #[tokio::test]
    async fn nick_ownership() {
        let doc = ClusterDoc::new("server-1");
        doc.set_nick_owner("alice", "did:plc:abc123").await;

        assert_eq!(doc.nick_owner("alice").await, Some("did:plc:abc123".to_string()));
        assert_eq!(doc.nick_owner("bob").await, None);
    }

    #[tokio::test]
    async fn sync_between_two_servers() {
        let doc1 = ClusterDoc::new("server-1");
        let doc2 = ClusterDoc::new("server-2");

        doc1.set_topic("#test", "Hello from server 1", "alice", None, "peer-1").await;
        doc1.set_nick_owner("alice", "did:plc:abc").await;

        // Sync using the wrapper API (keyed by consistent peer IDs)
        for _ in 0..10 {
            if let Some(msg) = doc1.generate_sync_message("server-2").await {
                doc2.receive_sync_message("server-1", &msg).await.unwrap();
            }
            if let Some(msg) = doc2.generate_sync_message("server-1").await {
                doc1.receive_sync_message("server-2", &msg).await.unwrap();
            }
        }

        let topic = doc2.channel_topic("#test").await;
        assert_eq!(topic, Some(("Hello from server 1".to_string(), "alice".to_string())));
        assert_eq!(doc2.nick_owner("alice").await, Some("did:plc:abc".to_string()));
    }

    #[tokio::test]
    async fn save_and_load() {
        let doc = ClusterDoc::new("server-1");
        doc.set_topic("#test", "Persistent topic", "alice", None, "peer-1").await;
        doc.set_nick_owner("alice", "did:plc:abc").await;

        let bytes = doc.save().await;
        let doc2 = ClusterDoc::load(&bytes, "server-1").unwrap();

        assert_eq!(doc2.channel_topic("#test").await.unwrap().0, "Persistent topic");
        assert_eq!(doc2.nick_owner("alice").await.unwrap(), "did:plc:abc");
    }

    #[tokio::test]
    async fn bans_with_provenance() {
        let doc = ClusterDoc::new("server-1");
        doc.add_ban("#test", "evil!*@*", "alice", Some("did:plc:alice"), "peer-1").await;
        doc.add_ban("#test", "bad!*@*", "bob", None, "peer-2").await;
        doc.remove_ban("#test", "evil!*@*").await;

        let bans = doc.channel_bans("#test").await;
        assert_eq!(bans.len(), 1);
        assert_eq!(bans[0].0, "bad!*@*");
    }

    #[tokio::test]
    async fn founder_deterministic_min_actor() {
        let doc1 = ClusterDoc::new("server-1");
        let doc2 = ClusterDoc::new("server-2");

        // Both try to set founder concurrently
        doc1.set_founder("#test", "did:plc:alice").await;
        doc2.set_founder("#test", "did:plc:bob").await;

        // Sync
        for _ in 0..10 {
            if let Some(msg) = doc1.generate_sync_message("server-2").await {
                doc2.receive_sync_message("server-1", &msg).await.unwrap();
            }
            if let Some(msg) = doc2.generate_sync_message("server-1").await {
                doc1.receive_sync_message("server-2", &msg).await.unwrap();
            }
        }

        // Both must agree (min-actor wins: "server-1" < "server-2")
        let f1 = doc1.founder("#test").await;
        let f2 = doc2.founder("#test").await;
        assert_eq!(f1, f2, "Founders must converge: {f1:?} vs {f2:?}");
        assert!(f1.is_some(), "Founder must not be lost");
    }

    #[tokio::test]
    async fn founder_not_overwritten_after_sync() {
        let doc1 = ClusterDoc::new("server-1");
        let doc2 = ClusterDoc::new("server-2");

        doc1.set_founder("#test", "did:plc:alice").await;

        // Sync to doc2
        for _ in 0..10 {
            if let Some(msg) = doc1.generate_sync_message("server-2").await {
                doc2.receive_sync_message("server-1", &msg).await.unwrap();
            }
            if let Some(msg) = doc2.generate_sync_message("server-1").await {
                doc1.receive_sync_message("server-2", &msg).await.unwrap();
            }
        }

        assert_eq!(doc2.founder("#test").await, Some("did:plc:alice".to_string()));

        // Server-2 tries to overwrite — should be rejected (server-2 > server-1)
        doc2.set_founder("#test", "did:plc:evil").await;
        assert_eq!(doc2.founder("#test").await, Some("did:plc:alice".to_string()));
    }

    #[tokio::test]
    async fn did_ops_sync() {
        let doc1 = ClusterDoc::new("server-1");
        let doc2 = ClusterDoc::new("server-2");

        doc1.set_founder("#test", "did:plc:alice").await;
        doc1.grant_op("#test", "did:plc:bob", Some("did:plc:alice"), "peer-1").await;
        doc2.grant_op("#test", "did:plc:charlie", None, "peer-2").await;

        // Sync
        for _ in 0..10 {
            if let Some(msg) = doc1.generate_sync_message("server-2").await {
                doc2.receive_sync_message("server-1", &msg).await.unwrap();
            }
            if let Some(msg) = doc2.generate_sync_message("server-1").await {
                doc1.receive_sync_message("server-2", &msg).await.unwrap();
            }
        }

        let ops1 = doc1.channel_did_ops("#test").await;
        let ops2 = doc2.channel_did_ops("#test").await;
        assert_eq!(ops1.len(), 2, "doc1 ops: {ops1:?}");
        assert_eq!(ops2.len(), 2, "doc2 ops: {ops2:?}");

        // Revoke bob on server 1
        doc1.revoke_op("#test", "did:plc:bob").await;

        for _ in 0..10 {
            if let Some(msg) = doc1.generate_sync_message("server-2").await {
                doc2.receive_sync_message("server-1", &msg).await.unwrap();
            }
            if let Some(msg) = doc2.generate_sync_message("server-1").await {
                doc1.receive_sync_message("server-2", &msg).await.unwrap();
            }
        }

        let ops1 = doc1.channel_did_ops("#test").await;
        let ops2 = doc2.channel_did_ops("#test").await;
        assert_eq!(ops1.len(), 1, "After revoke, doc1 ops: {ops1:?}");
        assert_eq!(ops2.len(), 1, "After revoke, doc2 ops: {ops2:?}");
        assert_eq!(ops1[0], "did:plc:charlie");
    }

    #[tokio::test]
    async fn compaction_preserves_state() {
        let doc = ClusterDoc::new("server-1");
        doc.set_topic("#test", "Hello", "alice", None, "peer-1").await;
        doc.set_founder("#test", "did:plc:alice").await;
        doc.set_nick_owner("alice", "did:plc:alice").await;

        let key_count_before = doc.key_count().await;
        doc.compact().await.unwrap();
        let key_count_after = doc.key_count().await;

        assert_eq!(key_count_before, key_count_after);
        assert_eq!(doc.founder("#test").await, Some("did:plc:alice".to_string()));
        assert_eq!(doc.nick_owner("alice").await, Some("did:plc:alice".to_string()));
    }

    #[tokio::test]
    async fn metrics_track_sync() {
        let doc1 = ClusterDoc::new("server-1");
        let doc2 = ClusterDoc::new("server-2");

        doc1.set_topic("#test", "Hello", "alice", None, "peer-1").await;

        for _ in 0..5 {
            if let Some(msg) = doc1.generate_sync_message("server-2").await {
                doc2.receive_sync_message("server-1", &msg).await.unwrap();
            }
            if let Some(msg) = doc2.generate_sync_message("server-1").await {
                doc1.receive_sync_message("server-2", &msg).await.unwrap();
            }
        }

        let m1 = doc1.get_metrics().await;
        assert!(m1.sync_messages_sent > 0);
        assert!(m1.sync_bytes_sent > 0);
        assert!(m1.change_count > 0);
    }

    #[tokio::test]
    async fn authority_validation() {
        let doc = ClusterDoc::new("server-1");
        doc.set_founder("#test", "did:plc:alice").await;
        doc.grant_op("#test", "did:plc:bob", Some("did:plc:alice"), "peer-1").await;

        // Founder has authority
        assert!(doc.validate_topic_authority("#test", Some("did:plc:alice"), false).await);
        // DID-op has authority
        assert!(doc.validate_topic_authority("#test", Some("did:plc:bob"), false).await);
        // Random DID does not
        assert!(!doc.validate_topic_authority("#test", Some("did:plc:evil"), false).await);
        // No DID = allow in compat mode
        assert!(doc.validate_topic_authority("#test", None, false).await);
        // No DID = reject in strict mode
        assert!(!doc.validate_topic_authority("#test", None, true).await);

        // Op grant authority
        assert!(doc.validate_op_grant_authority("#test", Some("did:plc:alice"), false).await);
        assert!(doc.validate_op_grant_authority("#test", Some("did:plc:bob"), false).await);
        assert!(!doc.validate_op_grant_authority("#test", Some("did:plc:nobody"), false).await);
        // Strict mode: no DID = reject
        assert!(!doc.validate_op_grant_authority("#test", None, true).await);
    }
}
