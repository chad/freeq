//! Diagnostic event recorder.
//!
//! In-memory ring buffer of structured server events. Hooked from a
//! few critical sites today (session lifecycle, message persistence)
//! and intentionally exposed as a free function so future hooks can
//! be added without threading state through call paths.
//!
//! Sensitive fields (raw token, raw message body, full policy
//! expression, etc.) MUST NOT be recorded here. The recorder lives
//! inside the trust boundary — its responsibility is to capture the
//! *facts about* an event, not the secret payload.

use parking_lot::Mutex;
use serde::Serialize;
use std::collections::VecDeque;
use std::sync::LazyLock;

/// Maximum events retained in the in-memory ring buffer.
///
/// Sized for short-window diagnosis (recent reconnects, recent message
/// flow). Trace_recent_events queries should add their own time-window
/// filter.
const RING_CAPACITY: usize = 10_000;

/// Categories of recorded events. Names map to the event types listed
/// in §5.1 of the assistance interface spec.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    SessionOpened,
    SessionClosed,
    AuthAttempt,
    SaslChallenge,
    CapNegotiation,
    Join,
    Part,
    Kick,
    ModeChange,
    PolicyEvaluation,
    MessageAccepted,
    MessageDelivered,
    MessageRejected,
    AckReceived,
    ResumeRequested,
    ResumeReplay,
    FederationInbound,
    FederationOutbound,
    SignatureValidation,
    CrdtMerge,
    RateLimitDecision,
}

/// One captured server-side event.
///
/// Fields default to `None` so call sites can record only what they
/// know. The `disclosure_level` field is the minimum [`super::types::DisclosureLevel`]
/// (as a string for storage simplicity) required to view this event.
#[derive(Debug, Clone, Serialize)]
pub struct DiagnosticEvent {
    pub time_unix: i64,
    pub kind: EventKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub did: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub msgid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_sequence: Option<i64>,
    /// Brief reason or result code (e.g. `"JOIN_POLICY_DENIED"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Disclosure level required to view this event (lowercase string
    /// matching the `DisclosureLevel` serde name).
    pub disclosure_level: &'static str,
}

impl DiagnosticEvent {
    /// Convenience: minimal event tagged with `kind` and the current time.
    pub fn now(kind: EventKind) -> Self {
        Self {
            time_unix: chrono::Utc::now().timestamp(),
            kind,
            did: None,
            session_id: None,
            channel: None,
            msgid: None,
            server_sequence: None,
            reason: None,
            disclosure_level: "account",
        }
    }
}

/// Ring buffer holding recent events.
pub struct EventRecorder {
    inner: Mutex<VecDeque<DiagnosticEvent>>,
    cap: usize,
}

impl EventRecorder {
    fn new(cap: usize) -> Self {
        Self {
            inner: Mutex::new(VecDeque::with_capacity(cap.min(1024))),
            cap,
        }
    }

    /// Append an event, evicting the oldest if at capacity.
    pub fn record(&self, event: DiagnosticEvent) {
        let mut buf = self.inner.lock();
        if buf.len() == self.cap {
            buf.pop_front();
        }
        buf.push_back(event);
    }

    /// Snapshot a filtered slice of events. Callers should keep the
    /// filter narrow; this clones what it returns.
    pub fn query<F>(&self, mut keep: F) -> Vec<DiagnosticEvent>
    where
        F: FnMut(&DiagnosticEvent) -> bool,
    {
        self.inner
            .lock()
            .iter()
            .filter(|e| keep(e))
            .cloned()
            .collect()
    }

    /// Total events currently retained (for tests / health).
    pub fn len(&self) -> usize {
        self.inner.lock().len()
    }

    pub fn is_empty(&self) -> bool {
        self.inner.lock().is_empty()
    }

    /// Drop everything (used in tests).
    #[cfg(test)]
    pub fn clear(&self) {
        self.inner.lock().clear();
    }
}

/// Process-wide singleton recorder. Hooked via free [`record`] below
/// so call sites don't need to thread state.
pub static RECORDER: LazyLock<EventRecorder> = LazyLock::new(|| EventRecorder::new(RING_CAPACITY));

/// Record an event into the global ring buffer.
///
/// Call sites pass a fully-populated [`DiagnosticEvent`] (use
/// [`DiagnosticEvent::now`] as a starting point). Cheap: a single
/// mutex acquire and a `VecDeque` push.
pub fn record(event: DiagnosticEvent) {
    RECORDER.record(event);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_evicts_oldest() {
        let r = EventRecorder::new(3);
        for i in 0..5 {
            let mut e = DiagnosticEvent::now(EventKind::MessageAccepted);
            e.msgid = Some(format!("m{i}"));
            r.record(e);
        }
        let all = r.query(|_| true);
        assert_eq!(all.len(), 3);
        assert_eq!(all[0].msgid.as_deref(), Some("m2"));
        assert_eq!(all[2].msgid.as_deref(), Some("m4"));
    }

    #[test]
    fn query_filters_by_kind() {
        let r = EventRecorder::new(10);
        r.record(DiagnosticEvent::now(EventKind::SessionOpened));
        r.record(DiagnosticEvent::now(EventKind::MessageAccepted));
        r.record(DiagnosticEvent::now(EventKind::SessionClosed));
        let opens = r.query(|e| matches!(e.kind, EventKind::SessionOpened));
        assert_eq!(opens.len(), 1);
    }
}
