//! EventEnvelope — versioned wrapper around DomainEvent for the C# callback.

use serde::Serialize;

use crate::event::DomainEvent;

/// Versioned envelope wrapping every event dispatched to the C# layer.
///
/// Fields:
/// - `version`: Schema version (always 1 for now).
/// - `seq`: Monotonically increasing sequence number per client instance.
/// - `timestamp_ms`: UTC milliseconds when the envelope was created.
/// - `event`: The domain event payload.
#[derive(Debug, Clone, Serialize)]
pub struct EventEnvelope {
    pub version: u32,
    pub seq: u64,
    pub timestamp_ms: i64,
    pub event: DomainEvent,
}

impl EventEnvelope {
    /// Create a new envelope with the given sequence number and event.
    pub fn new(seq: u64, event: DomainEvent) -> Self {
        Self {
            version: 1,
            seq,
            timestamp_ms: chrono::Utc::now().timestamp_millis(),
            event,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_envelope_serialization() {
        let envelope = EventEnvelope::new(42, DomainEvent::Connected);
        let json = serde_json::to_string(&envelope).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed["version"], 1);
        assert_eq!(parsed["seq"], 42);
        assert!(parsed["timestamp_ms"].as_i64().unwrap() > 0);
        assert_eq!(parsed["event"]["type"], "connected");
    }

    #[test]
    fn test_envelope_with_message_event() {
        let event = DomainEvent::Message(crate::event::MessageData {
            from_nick: "alice".to_string(),
            target: "#test".to_string(),
            text: "hello".to_string(),
            msgid: Some("msg1".to_string()),
            reply_to: None,
            edit_of: None,
            batch_id: None,
            is_action: false,
            timestamp_ms: 1700000000000,
        });
        let envelope = EventEnvelope::new(1, event);
        let json = serde_json::to_string(&envelope).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed["event"]["type"], "message");
        assert_eq!(parsed["event"]["data"]["from_nick"], "alice");
        assert_eq!(parsed["seq"], 1);
    }
}
