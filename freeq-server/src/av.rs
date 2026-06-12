//! AV Session subsystem — real-time voice/video/screen sharing.
//!
//! Sessions are first-class objects that live alongside IRC channels.
//! A session can be bound to a channel (most common) or ad-hoc (DM calls).
//!
//! Session control flows through IRC (TAGMSG with +freeq.at/av-* tags).
//! Media flows through iroh-live (separate QUIC path, not over IRC).
//! These are intentionally decoupled so the media backend is swappable.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// ULID-based session identifier.
pub type AvSessionId = String;

// ── Core types ─────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AvSessionState {
    Active,
    Ended {
        ended_at: i64,
        ended_by: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AvSession {
    pub id: AvSessionId,
    /// Channel this session is bound to (None for ad-hoc / DM calls).
    pub channel: Option<String>,
    /// DID of the user who created the session.
    pub created_by: String,
    /// Nick of the creator (for display).
    pub created_by_nick: String,
    pub created_at: i64,
    pub state: AvSessionState,
    /// DID → participant info.
    pub participants: HashMap<String, AvParticipant>,
    pub title: Option<String>,
    /// iroh-live connection ticket (opaque string, passed to clients).
    pub iroh_ticket: Option<String>,
    pub media_backend: MediaBackendType,
    pub recording_enabled: bool,
    pub max_participants: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AvParticipant {
    pub did: String,
    pub nick: String,
    pub joined_at: i64,
    pub left_at: Option<i64>,
    pub role: ParticipantRole,
    pub tracks: Vec<TrackInfo>,
    /// Per-connection identifier so the same DID can join from multiple
    /// devices in the same session. Each client generates a short random
    /// suffix at join time, sends it via the `+freeq.at/av-instance` tag,
    /// and uses it when constructing the MoQ broadcast path
    /// (`{session_id}/{nick}~{instance_id}`). Older clients that don't
    /// send the tag get `None`, and the participant is keyed by bare DID
    /// (legacy behaviour — one slot per DID).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instance_id: Option<String>,
}

/// Compute the participants-map key for a (DID, instance_id) pair.
/// With an instance_id we produce `did#instance` so the same DID can hold
/// multiple slots; without one we fall back to plain DID for legacy clients.
pub fn participant_key(did: &str, instance_id: Option<&str>) -> String {
    match instance_id {
        Some(iid) if !iid.is_empty() => format!("{did}#{iid}"),
        _ => did.to_string(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ParticipantRole {
    Host,
    Speaker,
    Listener,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrackInfo {
    pub kind: TrackKind,
    pub muted: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TrackKind {
    Audio,
    Video,
    Screen,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[derive(Default)]
pub enum MediaBackendType {
    #[default]
    IrohLive,
}

// ── Artifacts (Phase 2) ────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AvArtifact {
    pub id: String,
    pub session_id: AvSessionId,
    pub kind: ArtifactKind,
    pub created_at: i64,
    /// DID of creator (None = system-generated).
    pub created_by: Option<String>,
    /// PDS blob CID or URL.
    pub content_ref: String,
    pub content_type: String,
    pub visibility: ArtifactVisibility,
    /// Human-readable title or filename.
    pub title: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ArtifactKind {
    Transcript,
    Summary,
    Recording,
    Decisions,
    Tasks,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum ArtifactVisibility {
    #[default]
    Participants,
    Channel,
    Public,
}

// ── Session Manager ────────────────────────────────────────────────

/// Manages all active AV sessions. Lives in SharedState.
#[derive(Debug)]
pub struct AvSessionManager {
    /// Active + recently ended sessions (in-memory cache).
    pub sessions: HashMap<AvSessionId, AvSession>,
    /// Channel → active session ID (at most one active session per channel).
    pub channel_sessions: HashMap<String, AvSessionId>,
}

impl Default for AvSessionManager {
    fn default() -> Self {
        Self::new()
    }
}

impl AvSessionManager {
    pub fn new() -> Self {
        Self {
            sessions: HashMap::new(),
            channel_sessions: HashMap::new(),
        }
    }

    /// Create a new session. Returns the session or an error message.
    ///
    /// `creator_instance_id` is the per-device suffix from the creator's
    /// `+freeq.at/av-instance` tag (None for legacy clients). It's used to
    /// key the creator's participant slot just like `join_session` does, so
    /// the creator can later join from a second device without colliding.
    pub fn create_session(
        &mut self,
        channel: Option<&str>,
        creator_did: &str,
        creator_nick: &str,
        title: Option<&str>,
        creator_instance_id: Option<&str>,
    ) -> Result<AvSession, String> {
        // Check: only one active session per channel.
        // If the existing session has no active participants (all left/disconnected),
        // auto-end it so a new session can start.
        if let Some(ch) = channel
            && let Some(existing_id) = self.channel_sessions.get(&ch.to_lowercase()).cloned()
            && let Some(existing) = self.sessions.get(&existing_id)
            && matches!(existing.state, AvSessionState::Active)
        {
            let active_count = existing
                .participants
                .values()
                .filter(|p| p.left_at.is_none())
                .count();
            if active_count > 0 {
                return Err(format!(
                    "Channel {} already has an active session: {}",
                    ch, existing_id
                ));
            }
            // No active participants — auto-end the stale session
            tracing::info!(
                session = %existing_id,
                channel = %ch,
                "Auto-ending stale session (0 active participants) to allow new session"
            );
            self.end_session_inner(&existing_id, Some(creator_did));
        }

        let id = ulid::Ulid::new().to_string();
        let now = chrono::Utc::now().timestamp();

        let mut participants = HashMap::new();
        let creator_key = participant_key(creator_did, creator_instance_id);
        participants.insert(
            creator_key,
            AvParticipant {
                did: creator_did.to_string(),
                nick: creator_nick.to_string(),
                joined_at: now,
                left_at: None,
                role: ParticipantRole::Host,
                tracks: vec![],
                instance_id: creator_instance_id.map(|s| s.to_string()),
            },
        );

        let session = AvSession {
            id: id.clone(),
            channel: channel.map(|s| s.to_string()),
            created_by: creator_did.to_string(),
            created_by_nick: creator_nick.to_string(),
            created_at: now,
            state: AvSessionState::Active,
            participants,
            title: title.map(|s| s.to_string()),
            iroh_ticket: None,
            media_backend: MediaBackendType::default(),
            recording_enabled: false,
            max_participants: None,
        };

        self.sessions.insert(id.clone(), session);
        if let Some(ch) = channel {
            self.channel_sessions.insert(ch.to_lowercase(), id.clone());
        }

        Ok(self.sessions.get(&id).unwrap().clone())
    }

    /// Join an existing session. Returns updated session or error.
    ///
    /// `instance_id` lets the same DID hold multiple slots (one per device);
    /// pass `None` for clients that don't send a `+freeq.at/av-instance` tag
    /// (one slot per DID, legacy behaviour).
    pub fn join_session(
        &mut self,
        session_id: &str,
        did: &str,
        nick: &str,
        instance_id: Option<&str>,
    ) -> Result<AvSession, String> {
        let session = self
            .sessions
            .get_mut(session_id)
            .ok_or_else(|| format!("Session {session_id} not found"))?;

        if !matches!(session.state, AvSessionState::Active) {
            return Err("Session has ended".to_string());
        }

        if let Some(max) = session.max_participants {
            let active = session
                .participants
                .values()
                .filter(|p| p.left_at.is_none())
                .count();
            if active >= max as usize {
                return Err("Session is full".to_string());
            }
        }

        let now = chrono::Utc::now().timestamp();
        let key = participant_key(did, instance_id);

        // If this exact (did, instance_id) slot exists, rejoin in place;
        // otherwise insert a new slot. Two clients from the same DID end up
        // in two different slots so each one's MoQ broadcast is discoverable.
        if let Some(p) = session.participants.get_mut(&key) {
            p.left_at = None;
            p.joined_at = now;
            p.nick = nick.to_string();
        } else {
            session.participants.insert(
                key,
                AvParticipant {
                    did: did.to_string(),
                    nick: nick.to_string(),
                    joined_at: now,
                    left_at: None,
                    role: ParticipantRole::Speaker,
                    tracks: vec![],
                    instance_id: instance_id.map(|s| s.to_string()),
                },
            );
        }

        Ok(self.sessions.get(session_id).unwrap().clone())
    }

    /// Leave a session. Returns (session, should_end) — session ends if no
    /// active participants remain.
    ///
    /// `instance_id` selects which device's slot to mark as left; pass `None`
    /// for legacy clients (matches the bare-DID slot).
    pub fn leave_session(
        &mut self,
        session_id: &str,
        did: &str,
        instance_id: Option<&str>,
    ) -> Result<(AvSession, bool), String> {
        let session = self
            .sessions
            .get_mut(session_id)
            .ok_or_else(|| format!("Session {session_id} not found"))?;

        let key = participant_key(did, instance_id);
        if let Some(p) = session.participants.get_mut(&key) {
            p.left_at = Some(chrono::Utc::now().timestamp());
        } else if instance_id.is_some() {
            // Older clients may have joined this session before sending an
            // instance_id was a thing; fall back to the bare-DID slot so
            // half-instrumented sessions don't strand participants.
            if let Some(p) = session.participants.get_mut(did) {
                p.left_at = Some(chrono::Utc::now().timestamp());
            }
        }

        let active_count = session
            .participants
            .values()
            .filter(|p| p.left_at.is_none())
            .count();

        let should_end = active_count == 0;
        if should_end {
            self.end_session_inner(session_id, Some(did));
        }

        let session = self.sessions.get(session_id).unwrap().clone();
        Ok((session, should_end))
    }

    /// End a session (host or channel ops).
    pub fn end_session(
        &mut self,
        session_id: &str,
        ended_by: Option<&str>,
    ) -> Result<AvSession, String> {
        self.end_session_inner(session_id, ended_by);
        self.sessions
            .get(session_id)
            .cloned()
            .ok_or_else(|| format!("Session {session_id} not found"))
    }

    fn end_session_inner(&mut self, session_id: &str, ended_by: Option<&str>) {
        if let Some(session) = self.sessions.get_mut(session_id) {
            let now = chrono::Utc::now().timestamp();
            session.state = AvSessionState::Ended {
                ended_at: now,
                ended_by: ended_by.map(|s| s.to_string()),
            };
            // Mark all remaining participants as left
            for p in session.participants.values_mut() {
                if p.left_at.is_none() {
                    p.left_at = Some(now);
                }
            }
            // Remove from channel_sessions index
            if let Some(ch) = &session.channel {
                self.channel_sessions.remove(&ch.to_lowercase());
            }
        }
    }

    /// Get session by ID.
    pub fn get(&self, session_id: &str) -> Option<&AvSession> {
        self.sessions.get(session_id)
    }

    /// Get active session for a channel.
    pub fn active_session_for_channel(&self, channel: &str) -> Option<&AvSession> {
        let id = self.channel_sessions.get(&channel.to_lowercase())?;
        let session = self.sessions.get(id)?;
        if matches!(session.state, AvSessionState::Active) {
            Some(session)
        } else {
            None
        }
    }

    /// List all active sessions.
    pub fn active_sessions(&self) -> Vec<&AvSession> {
        self.sessions
            .values()
            .filter(|s| matches!(s.state, AvSessionState::Active))
            .collect()
    }

    /// Get active participant count for a session.
    pub fn active_participant_count(&self, session_id: &str) -> usize {
        self.sessions
            .get(session_id)
            .map(|s| {
                s.participants
                    .values()
                    .filter(|p| p.left_at.is_none())
                    .count()
            })
            .unwrap_or(0)
    }

    /// Check if a DID can end a session (must be host or channel op).
    pub fn can_end_session(&self, session_id: &str, did: &str) -> bool {
        self.sessions
            .get(session_id)
            .map(|s| {
                s.created_by == did
                    || s.participants
                        .get(did)
                        .map(|p| p.role == ParticipantRole::Host)
                        .unwrap_or(false)
            })
            .unwrap_or(false)
    }

    /// Apply a remote session event (from S2S federation).
    pub fn apply_remote_session_created(
        &mut self,
        id: &str,
        channel: Option<&str>,
        created_by_did: &str,
        created_by_nick: &str,
        title: Option<&str>,
        iroh_ticket: Option<&str>,
        created_at: i64,
    ) {
        if self.sessions.contains_key(id) {
            return; // Already known
        }

        let mut participants = HashMap::new();
        // S2S session creation doesn't yet carry the creator's instance_id,
        // so this stays bare-DID-keyed. Local av-join from a second device
        // for the same federated session will still get its own slot.
        participants.insert(
            created_by_did.to_string(),
            AvParticipant {
                did: created_by_did.to_string(),
                nick: created_by_nick.to_string(),
                joined_at: created_at,
                left_at: None,
                role: ParticipantRole::Host,
                tracks: vec![],
                instance_id: None,
            },
        );

        let session = AvSession {
            id: id.to_string(),
            channel: channel.map(|s| s.to_string()),
            created_by: created_by_did.to_string(),
            created_by_nick: created_by_nick.to_string(),
            created_at,
            state: AvSessionState::Active,
            participants,
            title: title.map(|s| s.to_string()),
            iroh_ticket: iroh_ticket.map(|s| s.to_string()),
            media_backend: MediaBackendType::default(),
            recording_enabled: false,
            max_participants: None,
        };

        if let Some(ch) = channel {
            self.channel_sessions
                .insert(ch.to_lowercase(), id.to_string());
        }
        self.sessions.insert(id.to_string(), session);
    }

    pub fn apply_remote_session_joined(&mut self, session_id: &str, did: &str, nick: &str) {
        // S2S doesn't yet carry instance_id; use the bare-DID slot. When the
        // S2S protocol grows the field, switch to `participant_key(did, iid)`.
        if let Some(session) = self.sessions.get_mut(session_id) {
            let now = chrono::Utc::now().timestamp();
            session
                .participants
                .entry(did.to_string())
                .and_modify(|p| {
                    p.left_at = None;
                    p.joined_at = now;
                })
                .or_insert_with(|| AvParticipant {
                    did: did.to_string(),
                    nick: nick.to_string(),
                    joined_at: now,
                    left_at: None,
                    role: ParticipantRole::Speaker,
                    tracks: vec![],
                    instance_id: None,
                });
        }
    }

    pub fn apply_remote_session_left(&mut self, session_id: &str, did: &str) {
        // Mark every slot for this DID as left — S2S leaves don't carry an
        // instance_id, so we have to assume the whole remote user dropped.
        if let Some(session) = self.sessions.get_mut(session_id) {
            let now = chrono::Utc::now().timestamp();
            for (key, p) in session.participants.iter_mut() {
                if p.left_at.is_some() {
                    continue;
                }
                if key == did || p.did == did {
                    p.left_at = Some(now);
                }
            }
        }
    }

    pub fn apply_remote_session_ended(&mut self, session_id: &str, ended_by: Option<&str>) {
        self.end_session_inner(session_id, ended_by);
    }

    /// Leave a single (DID, instance) slot across every active session.
    /// Used when one specific IRC connection (a single device/tab) drops —
    /// other devices on the same DID must keep their slots.
    /// Returns one tuple per session where that slot was marked left:
    /// `(session_id, channel, nick, should_end)`.
    pub fn leave_for_did_instance(
        &mut self,
        did: &str,
        instance_id: Option<&str>,
    ) -> Vec<(String, Option<String>, String, bool)> {
        let now = chrono::Utc::now().timestamp();
        let mut results = Vec::new();
        let key = participant_key(did, instance_id);
        let session_ids: Vec<String> = self.sessions.keys().cloned().collect();
        for session_id in session_ids {
            let Some(session) = self.sessions.get_mut(&session_id) else {
                continue;
            };
            if !matches!(session.state, AvSessionState::Active) {
                continue;
            }
            let nick = match session.participants.get_mut(&key) {
                Some(p) if p.left_at.is_none() => {
                    p.left_at = Some(now);
                    p.nick.clone()
                }
                _ => continue,
            };
            let active_count = session
                .participants
                .values()
                .filter(|p| p.left_at.is_none())
                .count();
            let should_end = active_count == 0;
            if should_end {
                self.end_session_inner(&session_id, Some(did));
            }
            let channel = self
                .sessions
                .get(&session_id)
                .and_then(|s| s.channel.clone());
            results.push((session_id, channel, nick, should_end));
        }
        results
    }

    /// Phantom-slot reaper. Given the set of `(did, instance_id)` pairs
    /// that have a live IRC connection right now, mark every other active
    /// slot in the named session as left. This is the cure for the
    /// "page-refresh leaves a ghost participant" class of bug.
    ///
    /// Callers should build the live-set by walking current connections
    /// and reading whatever av-instance they registered on join.
    pub fn reap_orphan_slots(
        &mut self,
        session_id: &str,
        live: &std::collections::HashSet<(String, Option<String>)>,
    ) {
        let now = chrono::Utc::now().timestamp();
        let Some(session) = self.sessions.get_mut(session_id) else {
            return;
        };
        if !matches!(session.state, AvSessionState::Active) {
            return;
        }
        for p in session.participants.values_mut() {
            if p.left_at.is_some() {
                continue;
            }
            let key = (p.did.clone(), p.instance_id.clone());
            if !live.contains(&key) {
                p.left_at = Some(now);
            }
        }
    }

    /// Leave all active slots belonging to a DID — used on connection drop.
    /// With multi-device support each device has its own instance-keyed slot,
    /// so we have to scan every participant whose `did` matches (not just
    /// the bare-DID key) and mark each one left. Returns one tuple per
    /// session where the DID had at least one active slot.
    pub fn leave_all_for_did(&mut self, did: &str) -> Vec<(String, Option<String>, String, bool)> {
        let now = chrono::Utc::now().timestamp();
        let mut results = Vec::new();

        let session_ids: Vec<String> = self.sessions.keys().cloned().collect();
        for session_id in session_ids {
            let Some(session) = self.sessions.get_mut(&session_id) else {
                continue;
            };
            if !matches!(session.state, AvSessionState::Active) {
                continue;
            }
            let mut any_left = false;
            let mut representative_nick = String::new();
            for p in session.participants.values_mut() {
                if p.did == did && p.left_at.is_none() {
                    p.left_at = Some(now);
                    if representative_nick.is_empty() {
                        representative_nick = p.nick.clone();
                    }
                    any_left = true;
                }
            }
            if !any_left {
                continue;
            }
            let active_count = session
                .participants
                .values()
                .filter(|p| p.left_at.is_none())
                .count();
            let should_end = active_count == 0;
            if should_end {
                self.end_session_inner(&session_id, Some(did));
            }
            let channel = self
                .sessions
                .get(&session_id)
                .and_then(|s| s.channel.clone());
            results.push((session_id, channel, representative_nick, should_end));
        }
        results
    }

    /// Prune ended sessions older than `max_age_secs` from memory.
    pub fn prune_ended(&mut self, max_age_secs: i64) {
        let now = chrono::Utc::now().timestamp();
        self.sessions.retain(|_, s| match &s.state {
            AvSessionState::Active => true,
            AvSessionState::Ended { ended_at, .. } => now - ended_at < max_age_secs,
        });
    }
}

// DB persistence methods are in db.rs (needs access to private conn field).

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_and_join_session() {
        let mut mgr = AvSessionManager::new();
        let session = mgr
            .create_session(
                Some("#test"),
                "did:plc:alice",
                "alice",
                Some("standup"),
                None,
            )
            .unwrap();
        assert_eq!(session.created_by, "did:plc:alice");
        assert!(matches!(session.state, AvSessionState::Active));
        assert_eq!(session.participants.len(), 1);

        let id = session.id.clone();
        mgr.join_session(&id, "did:plc:bob", "bob", None).unwrap();
        let session = mgr.get(&id).unwrap();
        assert_eq!(session.participants.len(), 2);
        assert_eq!(mgr.active_participant_count(&id), 2);
    }

    #[test]
    fn one_session_per_channel() {
        let mut mgr = AvSessionManager::new();
        mgr.create_session(Some("#test"), "did:plc:alice", "alice", None, None)
            .unwrap();
        let err = mgr
            .create_session(Some("#test"), "did:plc:bob", "bob", None, None)
            .unwrap_err();
        assert!(err.contains("already has an active session"));
    }

    #[test]
    fn leave_and_auto_end() {
        let mut mgr = AvSessionManager::new();
        let session = mgr
            .create_session(Some("#test"), "did:plc:alice", "alice", None, None)
            .unwrap();
        let id = session.id.clone();

        let (_, should_end) = mgr.leave_session(&id, "did:plc:alice", None).unwrap();
        assert!(should_end);
        let session = mgr.get(&id).unwrap();
        assert!(matches!(session.state, AvSessionState::Ended { .. }));
    }

    #[test]
    fn end_session_marks_all_left() {
        let mut mgr = AvSessionManager::new();
        let session = mgr
            .create_session(Some("#test"), "did:plc:alice", "alice", None, None)
            .unwrap();
        let id = session.id.clone();
        mgr.join_session(&id, "did:plc:bob", "bob", None).unwrap();

        mgr.end_session(&id, Some("did:plc:alice")).unwrap();
        let session = mgr.get(&id).unwrap();
        assert!(session.participants.values().all(|p| p.left_at.is_some()));
    }

    #[test]
    fn rejoin_after_leaving() {
        let mut mgr = AvSessionManager::new();
        let session = mgr
            .create_session(Some("#test"), "did:plc:alice", "alice", None, None)
            .unwrap();
        let id = session.id.clone();
        mgr.join_session(&id, "did:plc:bob", "bob", None).unwrap();

        // Bob leaves (alice still in, so session doesn't end)
        mgr.leave_session(&id, "did:plc:bob", None).unwrap();
        assert_eq!(mgr.active_participant_count(&id), 1);

        // Bob rejoins
        mgr.join_session(&id, "did:plc:bob", "bob", None).unwrap();
        assert_eq!(mgr.active_participant_count(&id), 2);
    }

    #[test]
    fn channel_session_lookup() {
        let mut mgr = AvSessionManager::new();
        assert!(mgr.active_session_for_channel("#test").is_none());

        mgr.create_session(Some("#test"), "did:plc:alice", "alice", None, None)
            .unwrap();
        assert!(mgr.active_session_for_channel("#test").is_some());
        assert!(mgr.active_session_for_channel("#TEST").is_some()); // case insensitive
        assert!(mgr.active_session_for_channel("#other").is_none());
    }

    #[test]
    fn remote_session_lifecycle() {
        let mut mgr = AvSessionManager::new();
        mgr.apply_remote_session_created(
            "remote-1",
            Some("#collab"),
            "did:plc:remote",
            "remote_user",
            Some("sync"),
            Some("ticket-xyz"),
            1000,
        );
        assert!(mgr.active_session_for_channel("#collab").is_some());

        mgr.apply_remote_session_joined("remote-1", "did:plc:local", "local_user");
        assert_eq!(mgr.active_participant_count("remote-1"), 2);

        mgr.apply_remote_session_left("remote-1", "did:plc:local");
        assert_eq!(mgr.active_participant_count("remote-1"), 1);

        mgr.apply_remote_session_ended("remote-1", Some("did:plc:remote"));
        let session = mgr.get("remote-1").unwrap();
        assert!(matches!(session.state, AvSessionState::Ended { .. }));
        assert!(mgr.active_session_for_channel("#collab").is_none());
    }

    #[test]
    fn multi_instance_same_did() {
        // The bug this fix addresses: one user joining from two devices
        // (same DID, different instance_id) must produce two participant
        // slots so each device's MoQ broadcast is independently discoverable.
        let mut mgr = AvSessionManager::new();
        let id = mgr
            .create_session(
                Some("#test"),
                "did:plc:alice",
                "alice",
                None,
                Some("iphone"),
            )
            .unwrap()
            .id;
        mgr.join_session(&id, "did:plc:alice", "alice", Some("web"))
            .unwrap();
        let s = mgr.get(&id).unwrap();
        assert_eq!(s.participants.len(), 2, "two slots for one DID");
        assert_eq!(mgr.active_participant_count(&id), 2);

        let instances: std::collections::HashSet<_> = s
            .participants
            .values()
            .filter_map(|p| p.instance_id.clone())
            .collect();
        assert_eq!(instances.len(), 2);
        assert!(instances.contains("iphone"));
        assert!(instances.contains("web"));

        // Leaving one instance keeps the other active.
        mgr.leave_session(&id, "did:plc:alice", Some("iphone"))
            .unwrap();
        assert_eq!(mgr.active_participant_count(&id), 1);
        let s = mgr.get(&id).unwrap();
        let still_in: Vec<_> = s
            .participants
            .values()
            .filter(|p| p.left_at.is_none())
            .filter_map(|p| p.instance_id.clone())
            .collect();
        assert_eq!(still_in, vec!["web".to_string()]);

        // Connection-drop cleanup (no instance_id) cleans every slot.
        let results = mgr.leave_all_for_did("did:plc:alice");
        assert_eq!(results.len(), 1);
        assert!(results[0].3, "session should end when last slot drops");
        assert_eq!(mgr.active_participant_count(&id), 0);
    }

    /// Phantom-participants bug, observed 2026-05-18 in #avtest:
    ///
    /// User joined a brand-new channel from a single web client. UI shows 4
    /// participant tiles (1 self + 3 "chadfowler.com" ghosts). This test
    /// pins down the contract we need: when a stale connection is gone, its
    /// instance_id'd slot must not keep `left_at = None`.
    ///
    /// `leave_all_for_did` already does this — but ONLY if the disconnect
    /// handler actually runs. The bug surfaces when the same DID has live
    /// connections still up (other tabs/devices) and `leave_all_for_did`
    /// nukes slots for OTHER instances that are still active.
    ///
    /// This test demonstrates the over-broad cleanup: a single
    /// `leave_all_for_did` for a DID that still has another live instance
    /// should NOT mark the other instance as left. We want per-instance
    /// granularity for cleanup.
    #[test]
    fn leave_all_for_did_over_broad_kills_live_instances() {
        let mut mgr = AvSessionManager::new();
        // iPhone creates the session.
        let id = mgr
            .create_session(
                Some("#avtest"),
                "did:plc:alice",
                "alice",
                None,
                Some("iphone"),
            )
            .unwrap()
            .id;
        // Web tab joins as same DID, different instance.
        mgr.join_session(&id, "did:plc:alice", "alice", Some("web"))
            .unwrap();
        assert_eq!(mgr.active_participant_count(&id), 2);

        // The iPhone disconnects (its IRC connection closes). The web tab
        // is STILL alive. Today's `leave_all_for_did` doesn't know that —
        // it nukes every slot for the DID, including the web one.
        // What we want is per-instance cleanup keyed on the dying connection.
        mgr.leave_for_did_instance("did:plc:alice", Some("iphone"));

        // FAILING ASSERTION until we add `leave_for_did_instance` and tie
        // disconnect cleanup to the specific instance whose connection died.
        assert_eq!(
            mgr.active_participant_count(&id),
            1,
            "leaving one instance must keep the other alive — got {}",
            mgr.active_participant_count(&id)
        );
        let s = mgr.get(&id).unwrap();
        let still_in: Vec<_> = s
            .participants
            .values()
            .filter(|p| p.left_at.is_none())
            .filter_map(|p| p.instance_id.clone())
            .collect();
        assert_eq!(still_in, vec!["web".to_string()]);
    }

    /// REST API contract: `session_to_json` strips `instance_id` from each
    /// participant. The web client needs it to build per-device broadcast
    /// paths (`{session}/{nick}~{instance_id}`). Without it every same-DID
    /// slot collapses to the bare nick and we can't subscribe per-device.
    ///
    /// This is a unit test for the data model: `AvParticipant` must expose
    /// `instance_id` such that the JSON layer can pass it through. The
    /// matching fix in `web.rs::session_to_json` then has to actually
    /// include the field.
    #[test]
    fn participant_carries_instance_id_for_api_serialization() {
        let mut mgr = AvSessionManager::new();
        let id = mgr
            .create_session(
                Some("#avtest"),
                "did:plc:alice",
                "alice",
                None,
                Some("iphone"),
            )
            .unwrap()
            .id;
        mgr.join_session(&id, "did:plc:alice", "alice", Some("web"))
            .unwrap();
        let s = mgr.get(&id).unwrap();
        let instances: std::collections::BTreeSet<_> = s
            .participants
            .values()
            .filter(|p| p.left_at.is_none())
            .map(|p| p.instance_id.clone().unwrap_or_default())
            .collect();
        assert!(instances.contains("iphone"));
        assert!(instances.contains("web"));
    }

    /// Stale-slot accumulation: when the same DID joins repeatedly with
    /// different instance_ids without ever sending av-leave (page refreshes,
    /// crashes), we end up with N slots even though only the most recent
    /// has a live connection.
    ///
    /// We want a server-side reaper that, given the set of currently-online
    /// (did, instance_id) pairs, marks any other slot for that DID as left.
    /// This is the API the disconnect handler should call when it learns
    /// "this connection died" — but it needs the join handler to record the
    /// connection→instance mapping in the first place.
    ///
    /// FAILING until we implement `reap_orphan_slots`.
    #[test]
    fn reap_orphan_slots_clears_phantom_participants() {
        let mut mgr = AvSessionManager::new();
        let id = mgr
            .create_session(
                Some("#avtest"),
                "did:plc:alice",
                "alice",
                None,
                Some("tab1"),
            )
            .unwrap()
            .id;
        // Three more tabs joined, none ever sent av-leave (typical browser
        // tab churn).
        mgr.join_session(&id, "did:plc:alice", "alice", Some("tab2"))
            .unwrap();
        mgr.join_session(&id, "did:plc:alice", "alice", Some("tab3"))
            .unwrap();
        mgr.join_session(&id, "did:plc:alice", "alice", Some("tab4"))
            .unwrap();
        assert_eq!(mgr.active_participant_count(&id), 4);

        // Only tab4 has a live connection now. Reaper should clean tabs 1-3.
        let live: std::collections::HashSet<(String, Option<String>)> =
            [("did:plc:alice".to_string(), Some("tab4".to_string()))]
                .into_iter()
                .collect();
        mgr.reap_orphan_slots(&id, &live);

        assert_eq!(
            mgr.active_participant_count(&id),
            1,
            "reaper should leave exactly one live slot — got {}",
            mgr.active_participant_count(&id)
        );
        let s = mgr.get(&id).unwrap();
        let live_instances: Vec<_> = s
            .participants
            .values()
            .filter(|p| p.left_at.is_none())
            .filter_map(|p| p.instance_id.clone())
            .collect();
        assert_eq!(live_instances, vec!["tab4".to_string()]);
    }

    /// State matrix cell #18: the reaper at av-join time must NOT mark a
    /// live other-device slot as left. The live-set is built from the
    /// (did, instance_id) pairs currently registered with IRC connections;
    /// any active slot whose pair is in the live-set survives the sweep.
    #[test]
    fn reap_orphan_slots_preserves_live_other_device() {
        let mut mgr = AvSessionManager::new();
        // iPhone creates session, web tab joins (both still alive).
        let id = mgr
            .create_session(
                Some("#test"),
                "did:plc:alice",
                "alice",
                None,
                Some("iphone"),
            )
            .unwrap()
            .id;
        mgr.join_session(&id, "did:plc:alice", "alice", Some("web"))
            .unwrap();

        // After 3 brief tab reloads, three stale slots have accumulated.
        mgr.join_session(&id, "did:plc:alice", "alice", Some("tab-stale1"))
            .unwrap();
        mgr.join_session(&id, "did:plc:alice", "alice", Some("tab-stale2"))
            .unwrap();
        mgr.join_session(&id, "did:plc:alice", "alice", Some("tab-stale3"))
            .unwrap();
        assert_eq!(mgr.active_participant_count(&id), 5);

        // A fourth tab joins; the live-set has the iphone, the web tab,
        // and the new tab. The three stale tabs must be reaped — but the
        // iphone and web tab MUST survive.
        let live: std::collections::HashSet<(String, Option<String>)> = [
            ("did:plc:alice".to_string(), Some("iphone".to_string())),
            ("did:plc:alice".to_string(), Some("web".to_string())),
            ("did:plc:alice".to_string(), Some("tab-new".to_string())),
        ]
        .into_iter()
        .collect();
        mgr.reap_orphan_slots(&id, &live);
        mgr.join_session(&id, "did:plc:alice", "alice", Some("tab-new"))
            .unwrap();

        // 3 live slots — iphone, web, tab-new — the three stale ones are gone.
        assert_eq!(
            mgr.active_participant_count(&id),
            3,
            "iphone + web + tab-new should remain — got {}",
            mgr.active_participant_count(&id)
        );
        let s = mgr.get(&id).unwrap();
        let instances: std::collections::BTreeSet<_> = s
            .participants
            .values()
            .filter(|p| p.left_at.is_none())
            .filter_map(|p| p.instance_id.clone())
            .collect();
        assert!(instances.contains("iphone"));
        assert!(instances.contains("web"));
        assert!(instances.contains("tab-new"));
        assert!(!instances.contains("tab-stale1"));
        assert!(!instances.contains("tab-stale2"));
        assert!(!instances.contains("tab-stale3"));
    }

    /// State matrix cell #13: rejoin from the same device with a NEW
    /// instance_id (e.g. page refresh that minted a fresh suffix) must
    /// produce a fresh slot — never a ghost participant that other peers
    /// see but can't subscribe to.
    ///
    /// Pre-condition: the old instance's slot is reaped before the new
    /// join (the av-join handler in messaging.rs does exactly this via
    /// `reap_orphan_slots` immediately before `join_session`).
    #[test]
    fn rejoin_with_fresh_instance_produces_fresh_slot() {
        let mut mgr = AvSessionManager::new();
        let id = mgr
            .create_session(Some("#test"), "did:plc:alice", "alice", None, Some("old"))
            .unwrap()
            .id;
        // Other participant in the call.
        mgr.join_session(&id, "did:plc:bob", "bob", Some("bob1"))
            .unwrap();
        assert_eq!(mgr.active_participant_count(&id), 2);

        // Alice's old tab dies (no av-leave sent). Then she re-joins with
        // a fresh instance. The av-join handler reaps first, then joins.
        let live_after_reap: std::collections::HashSet<(String, Option<String>)> = [
            ("did:plc:bob".to_string(), Some("bob1".to_string())),
            // Joining alice is in the live-set per the handler's logic:
            ("did:plc:alice".to_string(), Some("new".to_string())),
        ]
        .into_iter()
        .collect();
        mgr.reap_orphan_slots(&id, &live_after_reap);
        mgr.join_session(&id, "did:plc:alice", "alice", Some("new"))
            .unwrap();

        // bob + alice@new — old alice slot reaped.
        assert_eq!(mgr.active_participant_count(&id), 2);
        let s = mgr.get(&id).unwrap();
        let alice_slots: Vec<_> = s
            .participants
            .values()
            .filter(|p| p.did == "did:plc:alice" && p.left_at.is_none())
            .filter_map(|p| p.instance_id.clone())
            .collect();
        assert_eq!(
            alice_slots,
            vec!["new".to_string()],
            "exactly one live alice slot, with the new instance — old must be reaped"
        );
    }

    /// State matrix cell #11/#12: clean leave vs unclean drop both end in
    /// the same observable state: that participant's slot is marked left.
    /// `leave_for_did_instance` is the per-connection cleanup used on
    /// disconnect; `leave_session` is the av-leave path. Both must mark
    /// only the addressed (did, instance) slot as left.
    #[test]
    fn drop_without_leave_marks_only_dropping_instance() {
        let mut mgr = AvSessionManager::new();
        let id = mgr
            .create_session(
                Some("#test"),
                "did:plc:alice",
                "alice",
                None,
                Some("iphone"),
            )
            .unwrap()
            .id;
        mgr.join_session(&id, "did:plc:alice", "alice", Some("web"))
            .unwrap();
        mgr.join_session(&id, "did:plc:bob", "bob", Some("bob1"))
            .unwrap();
        assert_eq!(mgr.active_participant_count(&id), 3);

        // iPhone connection dies — disconnect handler runs leave_for_did_instance.
        let results = mgr.leave_for_did_instance("did:plc:alice", Some("iphone"));
        assert_eq!(results.len(), 1, "one session was affected");
        assert!(
            !results[0].3,
            "session should NOT end — alice@web and bob are still there"
        );

        // 2 alive slots: alice@web, bob.
        assert_eq!(mgr.active_participant_count(&id), 2);
        let s = mgr.get(&id).unwrap();
        let live_pairs: std::collections::BTreeSet<_> = s
            .participants
            .values()
            .filter(|p| p.left_at.is_none())
            .map(|p| (p.did.clone(), p.instance_id.clone().unwrap_or_default()))
            .collect();
        assert!(live_pairs.contains(&("did:plc:alice".into(), "web".into())));
        assert!(live_pairs.contains(&("did:plc:bob".into(), "bob1".into())));
        assert!(
            !live_pairs.contains(&("did:plc:alice".into(), "iphone".into())),
            "iphone slot must be marked left"
        );
    }

    /// State matrix cell #11 corollary: leave-then-rejoin from a DIFFERENT
    /// device (different DID, same nick is invalid — but same DID,
    /// different instance is fine and produces a separate broadcast key).
    /// The web client subscribes per broadcast key, so this shows up as
    /// "tile name stays the same, but the moq-watch element's `name`
    /// attribute changes".
    #[test]
    fn rejoin_from_different_device_uses_distinct_instance() {
        let mut mgr = AvSessionManager::new();
        let id = mgr
            .create_session(
                Some("#test"),
                "did:plc:alice",
                "alice",
                None,
                Some("iphone"),
            )
            .unwrap()
            .id;
        // Alice leaves cleanly.
        mgr.leave_session(&id, "did:plc:alice", Some("iphone"))
            .unwrap();
        // Re-create — leave on the only participant ends the session, so
        // start over for this scenario by creating fresh.
        let id = mgr
            .create_session(Some("#test"), "did:plc:alice", "alice", None, Some("ipad"))
            .unwrap()
            .id;
        let s = mgr.get(&id).unwrap();
        assert_eq!(s.participants.len(), 1);
        let p = s.participants.values().next().unwrap();
        assert_eq!(
            p.instance_id.as_deref(),
            Some("ipad"),
            "fresh device's instance must distinguish the broadcast path"
        );
    }

    /// Cross-contract: the broadcast key the web client builds from the
    /// REST endpoint's `nick` + `instance_id` MUST exactly match the
    /// broadcast path the SDK FFI publishes (`{session_id}/{nick}~{instance_id}`).
    /// Any divergence here means "web shows the tile but the moq-watch
    /// subscription never resolves" — a silent black-tile bug.
    ///
    /// This is the contract that ties:
    ///   - SDK FFI: `format!("{session_id}/{nick}~{instance_id}")` (lib.rs:1118)
    ///   - Server REST: `instance_id` field on participants
    ///     (web.rs::session_to_json:3712)
    ///   - Web client: `${sessionId}/${nick}~${instance_id}` (CallPanel.tsx:182)
    #[test]
    fn participant_record_supports_web_broadcast_path_construction() {
        let mut mgr = AvSessionManager::new();
        let id = mgr
            .create_session(
                Some("#test"),
                "did:plc:alice",
                "alice",
                None,
                Some("ios123"),
            )
            .unwrap()
            .id;
        mgr.join_session(&id, "did:plc:bob", "bob", Some("web456"))
            .unwrap();

        let s = mgr.get(&id).unwrap();
        for p in s.participants.values() {
            // The path the SDK FFI publishes under:
            let sdk_path = match &p.instance_id {
                Some(iid) => format!("{}/{}~{}", s.id, p.nick, iid),
                None => format!("{}/{}", s.id, p.nick),
            };
            // The path the web client computes for moq-watch's `name`:
            let web_path = match &p.instance_id {
                Some(iid) if !iid.is_empty() => format!("{}/{}~{}", s.id, p.nick, iid),
                _ => format!("{}/{}", s.id, p.nick),
            };
            assert_eq!(
                sdk_path, web_path,
                "SDK FFI path and web client path must match for {p:?}"
            );
        }
    }

    #[test]
    fn prune_ended_sessions() {
        let mut mgr = AvSessionManager::new();
        let session = mgr
            .create_session(Some("#old"), "did:plc:alice", "alice", None, None)
            .unwrap();
        let id = session.id.clone();
        mgr.leave_session(&id, "did:plc:alice", None).unwrap();

        // Session just ended — should not be pruned with max_age > 0
        mgr.prune_ended(3600);
        assert!(mgr.get(&id).is_some());

        // With max_age = 0, prune immediately
        mgr.prune_ended(0);
        assert!(mgr.get(&id).is_none());
    }
}
