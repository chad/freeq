//! Firehose indexer — fold `at.freeq.*` records from a Jetstream feed
//! into the fork graph, so lineage and fork counts reflect the whole
//! network rather than only records pushed to this server.
//!
//! [Jetstream](https://github.com/bluesky-social/jetstream) is a JSON
//! firehose: each commit event carries `{ did, kind, commit: { operation,
//! collection, rkey, record } }`. For a create/update of an
//! `at.freeq.persona` record that carries `forkedFrom`, we derive the
//! same fork edge [`crate::records::persona_fork_edge`] does and record
//! it (idempotently). This is the automated counterpart to the
//! `POST /api/v1/personas/record` push endpoint — same trust model: we
//! only believe the signed `forkedFrom`, and the forker is the record's
//! own repo DID.
//!
//! Opt-in via `--firehose-jetstream-url`. The parsing core is pure and
//! unit-tested; the WebSocket loop is thin glue over it.

use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt;
use serde_json::Value;
use tokio_tungstenite::tungstenite::Message;

use crate::records::{persona_fork_edge, AtUri, PersonaRecord, PERSONA_NSID};
use crate::server::SharedState;

/// Parse a Jetstream commit event into a persona fork edge — `Some`
/// only for a create/update of an `at.freeq.persona` record carrying
/// `forkedFrom`. Returns `(parent_uri, child_uri, forked_by_did)`.
pub fn persona_fork_edge_from_event(event: &Value) -> Option<(String, String, Option<String>)> {
    if event.get("kind").and_then(Value::as_str) != Some("commit") {
        return None;
    }
    let did = event.get("did").and_then(Value::as_str)?;
    let commit = event.get("commit")?;
    match commit.get("operation").and_then(Value::as_str)? {
        "create" | "update" => {}
        _ => return None,
    }
    let collection = commit.get("collection").and_then(Value::as_str)?;
    if collection != PERSONA_NSID {
        return None;
    }
    let rkey = commit.get("rkey").and_then(Value::as_str)?;
    let rec: PersonaRecord = serde_json::from_value(commit.get("record")?.clone()).ok()?;
    let uri = AtUri {
        authority: did.to_string(),
        collection: collection.to_string(),
        rkey: rkey.to_string(),
    }
    .to_string();
    persona_fork_edge(&uri, &rec)
}

/// Fold a single event into the fork graph (idempotent per child).
/// Returns true if a new edge was recorded.
pub fn index_event(state: &Arc<SharedState>, event: &Value) -> bool {
    let Some((parent, child, forked_by)) = persona_fork_edge_from_event(event) else {
        return false;
    };
    let already = state
        .with_db(|db| Ok(db.get_fork_by_child("persona", &child)))
        .flatten()
        .is_some();
    if already {
        return false;
    }
    let fork_id = ulid::Ulid::new().to_string();
    state
        .with_db(|db| db.record_fork(&fork_id, "persona", &parent, &child, forked_by.as_deref(), None))
        .is_some()
}

/// Subscribe to a Jetstream feed and index `at.freeq.persona` records
/// into the fork graph, reconnecting with capped backoff. Runs until the
/// process exits.
pub async fn run_indexer(state: Arc<SharedState>, base_url: String) {
    // Restrict the feed to the collection we index.
    let url = if base_url.contains("wantedCollections") {
        base_url
    } else {
        let sep = if base_url.contains('?') { '&' } else { '?' };
        format!("{base_url}{sep}wantedCollections={PERSONA_NSID}")
    };
    let mut backoff = Duration::from_secs(1);
    loop {
        match tokio_tungstenite::connect_async(url.as_str()).await {
            Ok((mut ws, _)) => {
                tracing::info!(url = %url, "firehose indexer connected");
                backoff = Duration::from_secs(1);
                while let Some(msg) = ws.next().await {
                    match msg {
                        Ok(Message::Text(txt)) => {
                            if let Ok(event) = serde_json::from_str::<Value>(txt.as_str()) {
                                if index_event(&state, &event) {
                                    tracing::debug!("firehose: recorded a fork edge");
                                }
                            }
                        }
                        Ok(Message::Close(_)) | Err(_) => break,
                        _ => {}
                    }
                }
                tracing::warn!("firehose indexer disconnected; reconnecting");
            }
            Err(e) => tracing::warn!(error = %e, "firehose connect failed; retrying"),
        }
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(Duration::from_secs(60));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn commit_event(operation: &str, collection: &str, record: Value) -> Value {
        json!({
            "did": "did:plc:me",
            "kind": "commit",
            "commit": { "operation": operation, "collection": collection, "rkey": "child1", "record": record }
        })
    }

    fn persona(forked_from: Option<&str>) -> Value {
        let mut r = json!({
            "$type": "at.freeq.persona",
            "name": "Cassandra",
            "systemPrompt": "warn",
            "createdAt": "2026-06-06T00:00:00Z"
        });
        if let Some(p) = forked_from {
            r["forkedFrom"] = json!(p);
        }
        r
    }

    #[test]
    fn derives_edge_from_a_forked_persona_commit() {
        let ev = commit_event("create", "at.freeq.persona", persona(Some("at://did:plc:orig/at.freeq.persona/p1")));
        let (parent, child, by) = persona_fork_edge_from_event(&ev).unwrap();
        assert_eq!(parent, "at://did:plc:orig/at.freeq.persona/p1");
        assert_eq!(child, "at://did:plc:me/at.freeq.persona/child1");
        assert_eq!(by.as_deref(), Some("did:plc:me"));
    }

    #[test]
    fn ignores_non_fork_and_irrelevant_events() {
        // Original (no forkedFrom) → no edge.
        assert!(persona_fork_edge_from_event(&commit_event("create", "at.freeq.persona", persona(None))).is_none());
        // Deletes → no edge.
        assert!(persona_fork_edge_from_event(&commit_event("delete", "at.freeq.persona", persona(Some("at://x/y/z")))).is_none());
        // Other collections → ignored.
        assert!(persona_fork_edge_from_event(&commit_event("create", "app.bsky.feed.post", persona(Some("at://x/y/z")))).is_none());
        // Non-commit (identity/account) events → ignored.
        assert!(persona_fork_edge_from_event(&json!({ "did": "did:plc:me", "kind": "identity" })).is_none());
    }
}
