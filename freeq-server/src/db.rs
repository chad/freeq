//! SQLite persistence layer.
//!
//! Stores message history, channel state, bans, and DID-nick identity bindings.
//! Uses WAL mode for concurrent reads during writes.

use std::collections::HashMap;
use std::path::Path;

use rusqlite::{Connection, Result as SqlResult, params};

use crate::server::{BanEntry, ChannelState, TopicInfo};

/// Prefix for encrypted-at-rest message content.
const EAR_PREFIX: &str = "EAR1:";

/// Encrypt text with AES-256-GCM for storage at rest.
/// Panics on encryption failure — this indicates a broken key or AES implementation
/// and must not silently degrade to plaintext storage.
fn encrypt_at_rest(key: &[u8; 32], plaintext: &str) -> String {
    use aes_gcm::{Aes256Gcm, KeyInit, Nonce, aead::Aead};
    let cipher = Aes256Gcm::new(key.into());
    let nonce_bytes: [u8; 12] = rand::random();
    let nonce = Nonce::from_slice(&nonce_bytes);
    let ct = cipher
        .encrypt(nonce, plaintext.as_bytes())
        .expect("AES-256-GCM encryption failed — this should never happen with a valid key");
    use base64::Engine;
    let mut combined = Vec::with_capacity(12 + ct.len());
    combined.extend_from_slice(&nonce_bytes);
    combined.extend_from_slice(&ct);
    format!(
        "{EAR_PREFIX}{}",
        base64::engine::general_purpose::STANDARD.encode(&combined)
    )
}

/// Decrypt text from at-rest storage.
/// Legacy unencrypted data (without EAR1: prefix) is returned as-is with a warning.
/// Decryption failures on encrypted data return an error placeholder and log at ERROR.
fn decrypt_at_rest(key: &[u8; 32], stored: &str) -> String {
    if !stored.starts_with(EAR_PREFIX) {
        // Legacy plaintext data — return as-is but log so operators can identify
        // unencrypted records during migration.
        if !stored.is_empty() {
            tracing::debug!("Returning unencrypted legacy message — consider re-encrypting historical data");
        }
        return stored.to_string();
    }
    use aes_gcm::{Aes256Gcm, KeyInit, Nonce, aead::Aead};
    use base64::Engine;
    let b64 = &stored[EAR_PREFIX.len()..];
    match base64::engine::general_purpose::STANDARD.decode(b64) {
        Ok(combined) if combined.len() > 12 => {
            let nonce = Nonce::from_slice(&combined[..12]);
            let ct = &combined[12..];
            let cipher = Aes256Gcm::new(key.into());
            match cipher.decrypt(nonce, ct) {
                Ok(pt) => String::from_utf8_lossy(&pt).to_string(),
                Err(e) => {
                    tracing::error!(
                        "Decryption failed (wrong key or corrupt data): {e} — \
                         returning placeholder. Check db-encryption-key.secret."
                    );
                    "[decryption failed]".to_string()
                }
            }
        }
        _ => {
            tracing::error!("Malformed encrypted message (bad base64 or too short)");
            "[decryption failed]".to_string()
        }
    }
}

/// Compute a canonical DM channel key from two DIDs.
/// The key is `dm:<did_a>,<did_b>` where the DIDs are alphabetically sorted.
/// This ensures both participants produce the same key regardless of who sends.
pub fn canonical_dm_key(did_a: &str, did_b: &str) -> String {
    if did_a <= did_b {
        format!("dm:{did_a},{did_b}")
    } else {
        format!("dm:{did_b},{did_a}")
    }
}

/// Database handle wrapping a SQLite connection.
pub struct Db {
    conn: Connection,
    /// AES-256-GCM key for encrypting message content at rest.
    /// Derived from the server's signing key. If None, messages stored as plaintext.
    encryption_key: Option<[u8; 32]>,
}

/// A persisted reaction row.
#[derive(Debug, Clone)]
pub struct ReactionRow {
    pub target_msgid: String,
    pub channel: String,
    pub reactor_nick: String,
    pub reactor_did: Option<String>,
    pub emoji: String,
    pub timestamp: u64,
}

/// A persisted message row.
#[derive(Debug, Clone)]
pub struct MessageRow {
    pub id: i64,
    pub channel: String,
    pub sender: String,
    pub text: String,
    pub timestamp: u64,
    pub tags: HashMap<String, String>,
    /// ULID message ID (IRCv3 `msgid` tag).
    pub msgid: Option<String>,
    /// If this is an edit, the msgid of the original message it replaces.
    pub replaces_msgid: Option<String>,
    /// Unix timestamp when this message was deleted (soft delete).
    pub deleted_at: Option<u64>,
    /// DID of the sender (if authenticated at send time).
    pub sender_did: Option<String>,
}

/// A persisted identity (DID-nick binding).
#[derive(Debug, Clone)]
pub struct IdentityRow {
    pub did: String,
    pub nick: String,
}

impl Db {
    /// Open (or create) the database at the given path.
    pub fn open<P: AsRef<Path>>(path: P) -> SqlResult<Self> {
        let conn = Connection::open(path)?;
        let db = Self {
            conn,
            encryption_key: None,
        };
        db.init()?;
        Ok(db)
    }

    /// Open a database with encryption at rest for message content.
    pub fn open_encrypted<P: AsRef<Path>>(path: P, key: [u8; 32]) -> SqlResult<Self> {
        let conn = Connection::open(path)?;
        let db = Self {
            conn,
            encryption_key: Some(key),
        };
        db.init()?;
        Ok(db)
    }

    /// Open an in-memory database (for testing).
    pub fn open_memory() -> SqlResult<Self> {
        let conn = Connection::open_in_memory()?;
        let db = Self {
            conn,
            encryption_key: None,
        };
        db.init()?;
        Ok(db)
    }

    /// Open an in-memory database with encryption at rest (for testing).
    pub fn open_encrypted_memory(key: [u8; 32]) -> SqlResult<Self> {
        let conn = Connection::open_in_memory()?;
        let db = Self {
            conn,
            encryption_key: Some(key),
        };
        db.init()?;
        Ok(db)
    }

    fn init(&self) -> SqlResult<()> {
        self.conn.execute_batch("PRAGMA journal_mode=WAL;")?;
        self.conn.execute_batch("PRAGMA foreign_keys=ON;")?;
        self.conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS channels (
                name        TEXT PRIMARY KEY,
                topic_text  TEXT,
                topic_set_by TEXT,
                topic_set_at INTEGER,
                topic_locked INTEGER NOT NULL DEFAULT 0,
                invite_only  INTEGER NOT NULL DEFAULT 0,
                no_ext_msg   INTEGER NOT NULL DEFAULT 0,
                moderated    INTEGER NOT NULL DEFAULT 0,
                key          TEXT,
                founder_did  TEXT,
                did_ops_json TEXT NOT NULL DEFAULT '[]'
            );

            CREATE TABLE IF NOT EXISTS bans (
                id       INTEGER PRIMARY KEY AUTOINCREMENT,
                channel  TEXT NOT NULL,
                mask     TEXT NOT NULL,
                set_by   TEXT NOT NULL,
                set_at   INTEGER NOT NULL,
                UNIQUE(channel, mask)
            );

            CREATE TABLE IF NOT EXISTS messages (
                id        INTEGER PRIMARY KEY AUTOINCREMENT,
                channel   TEXT NOT NULL,
                sender    TEXT NOT NULL,
                text      TEXT NOT NULL,
                timestamp INTEGER NOT NULL,
                tags_json TEXT NOT NULL DEFAULT '{}'
            );

            CREATE INDEX IF NOT EXISTS idx_messages_channel_ts
                ON messages(channel, timestamp DESC);

            CREATE TABLE IF NOT EXISTS identities (
                did  TEXT PRIMARY KEY,
                nick TEXT NOT NULL UNIQUE
            );

            CREATE TABLE IF NOT EXISTS prekey_bundles (
                did         TEXT PRIMARY KEY,
                bundle_json TEXT NOT NULL,
                updated_at  INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS signing_keys (
                did            TEXT PRIMARY KEY,
                pubkey         BLOB NOT NULL,         -- raw 32-byte ed25519 public key
                registered_at  INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS user_channels (
                did     TEXT NOT NULL,
                channel TEXT NOT NULL,
                PRIMARY KEY (did, channel)
            );
            ",
        )?;

        // Migrate existing databases: add columns that may not exist yet.
        // ALTER TABLE ADD COLUMN is idempotent-safe via error suppression.
        let migrations = [
            "ALTER TABLE channels ADD COLUMN no_ext_msg INTEGER NOT NULL DEFAULT 0",
            "ALTER TABLE channels ADD COLUMN moderated INTEGER NOT NULL DEFAULT 0",
            "ALTER TABLE channels ADD COLUMN founder_did TEXT",
            "ALTER TABLE channels ADD COLUMN did_ops_json TEXT NOT NULL DEFAULT '[]'",
            "ALTER TABLE messages ADD COLUMN msgid TEXT",
            "ALTER TABLE messages ADD COLUMN replaces_msgid TEXT",
            "ALTER TABLE messages ADD COLUMN deleted_at INTEGER",
            "ALTER TABLE messages ADD COLUMN sender_did TEXT",
        ];
        for sql in &migrations {
            // Ignore "duplicate column name" errors — means column already exists
            let _ = self.conn.execute(sql, []);
        }

        // Phase 2: agent governance tables
        self.conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS agent_capability_grants (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                channel TEXT NOT NULL,
                agent_did TEXT NOT NULL,
                capability TEXT NOT NULL,
                scope TEXT,
                ttl_seconds INTEGER DEFAULT 0,
                requires_approval INTEGER DEFAULT 0,
                rate_limit INTEGER DEFAULT 0,
                granted_by TEXT NOT NULL,
                granted_at INTEGER NOT NULL,
                expires_at INTEGER,
                revoked_at INTEGER,
                UNIQUE(channel, agent_did, capability, scope)
            );

            CREATE TABLE IF NOT EXISTS governance_log (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                channel TEXT,
                target_did TEXT NOT NULL,
                action TEXT NOT NULL,
                issued_by TEXT NOT NULL,
                reason TEXT,
                timestamp INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS pending_approvals (
                id TEXT PRIMARY KEY,
                channel TEXT NOT NULL,
                agent_did TEXT NOT NULL,
                capability TEXT NOT NULL,
                resource TEXT,
                requested_at INTEGER NOT NULL,
                granted_by TEXT,
                granted_at INTEGER,
                denied_by TEXT,
                denied_at INTEGER,
                deny_reason TEXT,
                expires_at INTEGER
            );
            ",
        )?;

        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS agent_manifests (
                agent_did TEXT PRIMARY KEY,
                manifest_json TEXT NOT NULL,
                manifest_url TEXT,
                registered_by TEXT NOT NULL,
                registered_at INTEGER NOT NULL,
                active INTEGER DEFAULT 1
            );
            CREATE TABLE IF NOT EXISTS spawned_agents (
                child_did TEXT PRIMARY KEY,
                parent_did TEXT NOT NULL,
                parent_session TEXT NOT NULL,
                nick TEXT NOT NULL,
                channel TEXT NOT NULL,
                capabilities_json TEXT NOT NULL DEFAULT '[]',
                ttl_seconds INTEGER,
                task_ref TEXT,
                spawned_at INTEGER NOT NULL,
                despawned_at INTEGER
            );
            CREATE INDEX IF NOT EXISTS idx_spawn_parent ON spawned_agents(parent_did);
            CREATE INDEX IF NOT EXISTS idx_spawn_channel ON spawned_agents(channel);
            ",
        )?;

        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS agent_spend (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                channel TEXT NOT NULL,
                agent_did TEXT NOT NULL,
                amount REAL NOT NULL,
                unit TEXT NOT NULL,
                description TEXT,
                task_ref TEXT,
                timestamp INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_spend_channel_agent ON agent_spend(channel, agent_did, timestamp);
            CREATE INDEX IF NOT EXISTS idx_spend_period ON agent_spend(channel, agent_did, unit, timestamp);

            CREATE TABLE IF NOT EXISTS channel_budgets (
                channel TEXT NOT NULL,
                agent_did TEXT,
                budget_json TEXT NOT NULL,
                set_by TEXT NOT NULL,
                set_at INTEGER NOT NULL,
                PRIMARY KEY(channel, agent_did)
            );
            ",
        )?;

        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS coordination_events (
                event_id TEXT PRIMARY KEY,
                event_type TEXT NOT NULL,
                actor_did TEXT NOT NULL,
                channel TEXT NOT NULL,
                ref_id TEXT,
                payload_json TEXT NOT NULL DEFAULT '{}',
                signature TEXT,
                timestamp INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_coord_channel ON coordination_events(channel, timestamp);
            CREATE INDEX IF NOT EXISTS idx_coord_ref ON coordination_events(ref_id);
            CREATE INDEX IF NOT EXISTS idx_coord_actor ON coordination_events(actor_did, timestamp);
            ",
        )?;

        // Phase 3: reactions + pins tables
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS pins (
                channel      TEXT NOT NULL,
                msgid        TEXT NOT NULL,
                pinned_by    TEXT NOT NULL,
                pinned_at    INTEGER NOT NULL,
                UNIQUE(channel, msgid)
            );
            CREATE INDEX IF NOT EXISTS idx_pins_channel ON pins(channel, pinned_at DESC);
            ",
        )?;
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS reactions (
                target_msgid TEXT NOT NULL,
                channel      TEXT NOT NULL,
                reactor_nick TEXT NOT NULL,
                reactor_did  TEXT,
                emoji        TEXT NOT NULL,
                timestamp    INTEGER NOT NULL,
                UNIQUE(target_msgid, reactor_nick, emoji)
            );
            CREATE INDEX IF NOT EXISTS idx_reactions_msgid ON reactions(target_msgid);
            CREATE INDEX IF NOT EXISTS idx_reactions_channel ON reactions(channel);
            ",
        )?;

        // AV sessions tables
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS av_sessions (
                id               TEXT PRIMARY KEY,
                channel          TEXT,
                created_by       TEXT NOT NULL,
                created_at       INTEGER NOT NULL,
                ended_at         INTEGER,
                ended_by         TEXT,
                title            TEXT,
                iroh_ticket      TEXT,
                backend          TEXT NOT NULL DEFAULT '\"iroh-live\"',
                recording        BOOLEAN NOT NULL DEFAULT FALSE,
                max_participants INTEGER
            );
            CREATE INDEX IF NOT EXISTS idx_av_sessions_channel ON av_sessions(channel, created_at DESC);

            CREATE TABLE IF NOT EXISTS av_participants (
                session_id  TEXT NOT NULL REFERENCES av_sessions(id),
                did         TEXT NOT NULL,
                nick        TEXT NOT NULL,
                joined_at   INTEGER NOT NULL,
                left_at     INTEGER,
                role        TEXT NOT NULL DEFAULT '\"speaker\"',
                PRIMARY KEY (session_id, did)
            );

            CREATE TABLE IF NOT EXISTS av_artifacts (
                id           TEXT PRIMARY KEY,
                session_id   TEXT NOT NULL REFERENCES av_sessions(id),
                kind         TEXT NOT NULL,
                created_at   INTEGER NOT NULL,
                created_by   TEXT,
                content_ref  TEXT NOT NULL,
                content_type TEXT NOT NULL,
                visibility   TEXT NOT NULL DEFAULT '\"participants\"',
                title        TEXT
            );
            CREATE INDEX IF NOT EXISTS idx_av_artifacts_session ON av_artifacts(session_id);
            ",
        )?;

        Ok(())
    }

    // ── Channel state ──────────────────────────────────────────────────

    /// Save or update a channel's metadata (topic, modes, key).
    pub fn save_channel(&self, name: &str, ch: &ChannelState) -> SqlResult<()> {
        let did_ops_json = serde_json::to_string(&ch.did_ops.iter().collect::<Vec<_>>())
            .unwrap_or_else(|_| "[]".to_string());
        self.conn.execute(
            "INSERT INTO channels (name, topic_text, topic_set_by, topic_set_at, topic_locked, invite_only, no_ext_msg, moderated, key, founder_did, did_ops_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
             ON CONFLICT(name) DO UPDATE SET
                topic_text=excluded.topic_text,
                topic_set_by=excluded.topic_set_by,
                topic_set_at=excluded.topic_set_at,
                topic_locked=excluded.topic_locked,
                invite_only=excluded.invite_only,
                no_ext_msg=excluded.no_ext_msg,
                moderated=excluded.moderated,
                key=excluded.key,
                founder_did=excluded.founder_did,
                did_ops_json=excluded.did_ops_json",
            params![
                name,
                ch.topic.as_ref().map(|t| &t.text),
                ch.topic.as_ref().map(|t| &t.set_by),
                ch.topic.as_ref().map(|t| t.set_at as i64),
                ch.topic_locked as i32,
                ch.invite_only as i32,
                ch.no_ext_msg as i32,
                ch.moderated as i32,
                ch.key.as_deref(),
                ch.founder_did.as_deref(),
                did_ops_json,
            ],
        )?;
        Ok(())
    }

    /// Delete a channel from the database (when it becomes empty and should be cleaned up).
    pub fn delete_channel(&self, name: &str) -> SqlResult<()> {
        self.conn
            .execute("DELETE FROM channels WHERE name = ?1", params![name])?;
        self.conn
            .execute("DELETE FROM bans WHERE channel = ?1", params![name])?;
        Ok(())
    }

    /// Load all persisted channels (metadata + bans). Does not load messages
    /// or runtime-only state (members, ops, voiced, invites).
    pub fn load_channels(&self) -> SqlResult<HashMap<String, ChannelState>> {
        let mut channels = HashMap::new();

        let mut stmt = self.conn.prepare(
            "SELECT name, topic_text, topic_set_by, topic_set_at, topic_locked, invite_only, key, no_ext_msg, moderated, founder_did, did_ops_json
             FROM channels"
        )?;
        let rows = stmt.query_map([], |row| {
            let name: String = row.get(0)?;
            let topic_text: Option<String> = row.get(1)?;
            let topic_set_by: Option<String> = row.get(2)?;
            let topic_set_at: Option<i64> = row.get(3)?;
            let topic_locked: bool = row.get::<_, i32>(4)? != 0;
            let invite_only: bool = row.get::<_, i32>(5)? != 0;
            let key: Option<String> = row.get(6)?;
            let no_ext_msg: bool = row.get::<_, Option<i32>>(7)?.unwrap_or(0) != 0;
            let moderated: bool = row.get::<_, Option<i32>>(8)?.unwrap_or(0) != 0;
            let founder_did: Option<String> = row.get(9)?;
            let did_ops_json: String = row
                .get::<_, Option<String>>(10)?
                .unwrap_or_else(|| "[]".to_string());

            let topic = match (topic_text, topic_set_by, topic_set_at) {
                (Some(text), Some(set_by), Some(set_at)) => Some(TopicInfo {
                    text,
                    set_by,
                    set_at: set_at as u64,
                }),
                _ => None,
            };

            let did_ops: std::collections::HashSet<String> =
                serde_json::from_str(&did_ops_json).unwrap_or_default();

            let ch = ChannelState {
                topic,
                topic_locked,
                invite_only,
                no_ext_msg,
                moderated,
                key,
                founder_did,
                did_ops,
                ..Default::default()
            };
            Ok((name, ch))
        })?;

        for row in rows {
            let (name, ch) = row?;
            channels.insert(name, ch);
        }

        // Load bans
        let mut stmt = self
            .conn
            .prepare("SELECT channel, mask, set_by, set_at FROM bans")?;
        let ban_rows = stmt.query_map([], |row| {
            let channel: String = row.get(0)?;
            let mask: String = row.get(1)?;
            let set_by: String = row.get(2)?;
            let set_at: i64 = row.get(3)?;
            Ok((
                channel,
                BanEntry {
                    mask,
                    set_by,
                    set_at: set_at as u64,
                },
            ))
        })?;

        for row in ban_rows {
            let (channel, ban) = row?;
            if let Some(ch) = channels.get_mut(&channel) {
                ch.bans.push(ban);
            }
        }

        // Load pins
        let mut stmt = self
            .conn
            .prepare("SELECT channel, msgid, pinned_by, pinned_at FROM pins ORDER BY pinned_at DESC")?;
        let pin_rows = stmt.query_map([], |row| {
            let channel: String = row.get(0)?;
            let msgid: String = row.get(1)?;
            let pinned_by: String = row.get(2)?;
            let pinned_at: i64 = row.get(3)?;
            Ok((
                channel,
                crate::server::PinnedMessage {
                    msgid,
                    pinned_by,
                    pinned_at: pinned_at as u64,
                },
            ))
        })?;

        for row in pin_rows {
            let (channel, pin) = row?;
            if let Some(ch) = channels.get_mut(&channel) {
                ch.pins.push(pin);
            }
        }

        Ok(channels)
    }

    // ── Bans ───────────────────────────────────────────────────────────

    /// Add a ban to a channel.
    pub fn add_ban(&self, channel: &str, ban: &BanEntry) -> SqlResult<()> {
        self.conn.execute(
            "INSERT OR IGNORE INTO bans (channel, mask, set_by, set_at) VALUES (?1, ?2, ?3, ?4)",
            params![channel, ban.mask, ban.set_by, ban.set_at as i64],
        )?;
        Ok(())
    }

    /// Remove a ban from a channel.
    pub fn remove_ban(&self, channel: &str, mask: &str) -> SqlResult<()> {
        self.conn.execute(
            "DELETE FROM bans WHERE channel = ?1 AND mask = ?2",
            params![channel, mask],
        )?;
        Ok(())
    }

    // ── Messages ───────────────────────────────────────────────────────

    /// Store a message.
    pub fn insert_message(
        &self,
        channel: &str,
        sender: &str,
        text: &str,
        timestamp: u64,
        tags: &HashMap<String, String>,
        msgid: Option<&str>,
        sender_did: Option<&str>,
    ) -> SqlResult<()> {
        let tags_json = serde_json::to_string(tags).unwrap_or_else(|_| "{}".to_string());
        let stored_text = if let Some(ref key) = self.encryption_key {
            encrypt_at_rest(key, text)
        } else {
            text.to_string()
        };
        self.conn.execute(
            "INSERT INTO messages (channel, sender, text, timestamp, tags_json, msgid, sender_did)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                channel,
                sender,
                stored_text,
                timestamp as i64,
                tags_json,
                msgid,
                sender_did
            ],
        )?;
        // Record into the agent-assist diagnostic ring buffer. We
        // capture only the fact that a message was accepted — never
        // the body or tags. The auto-increment row id is the canonical
        // server sequence used by `diagnose_message_ordering`.
        let mut ev = crate::agent_assist::recorder::DiagnosticEvent::now(
            crate::agent_assist::recorder::EventKind::MessageAccepted,
        );
        ev.channel = Some(channel.to_string());
        ev.msgid = msgid.map(|s| s.to_string());
        ev.did = sender_did.map(|s| s.to_string());
        ev.server_sequence = Some(self.conn.last_insert_rowid());
        crate::agent_assist::recorder::record(ev);
        Ok(())
    }

    /// Fetch recent messages for a channel, ordered oldest-first.
    /// `limit`: max number of messages to return.
    /// `before`: if Some, only return messages with timestamp < this value (for pagination).
    pub fn get_messages(
        &self,
        channel: &str,
        limit: usize,
        before: Option<u64>,
    ) -> SqlResult<Vec<MessageRow>> {
        let mut rows_vec = if let Some(before_ts) = before {
            let mut stmt = self.conn.prepare(
                "SELECT id, channel, sender, text, timestamp, tags_json, msgid, replaces_msgid, deleted_at, sender_did
                 FROM messages
                 WHERE channel = ?1 AND deleted_at IS NULL AND timestamp < ?2
                 ORDER BY timestamp DESC, id DESC
                 LIMIT ?3"
            )?;
            let rows = stmt.query_map(
                params![channel, before_ts as i64, limit as i64],
                map_message_row,
            )?;
            rows.collect::<SqlResult<Vec<_>>>()?
        } else {
            let mut stmt = self.conn.prepare(
                "SELECT id, channel, sender, text, timestamp, tags_json, msgid, replaces_msgid, deleted_at, sender_did
                 FROM messages
                 WHERE channel = ?1 AND deleted_at IS NULL
                 ORDER BY timestamp DESC, id DESC
                 LIMIT ?2"
            )?;
            let rows = stmt.query_map(params![channel, limit as i64], map_message_row)?;
            rows.collect::<SqlResult<Vec<_>>>()?
        };
        // Reverse to oldest-first order
        rows_vec.reverse();
        // Decrypt at-rest encryption if enabled
        if let Some(ref key) = self.encryption_key {
            for row in &mut rows_vec {
                row.text = decrypt_at_rest(key, &row.text);
            }
        }
        Ok(rows_vec)
    }

    /// Get messages after a timestamp (oldest first).
    pub fn get_messages_after(
        &self,
        channel: &str,
        after: u64,
        limit: usize,
    ) -> SqlResult<Vec<MessageRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, channel, sender, text, timestamp, tags_json, msgid, replaces_msgid, deleted_at, sender_did
             FROM messages
             WHERE channel = ?1 AND deleted_at IS NULL AND timestamp > ?2
             ORDER BY timestamp ASC, id ASC
             LIMIT ?3"
        )?;
        let rows = stmt.query_map(
            params![channel, after as i64, limit as i64],
            map_message_row,
        )?;
        let mut result = rows.collect::<SqlResult<Vec<_>>>()?;
        if let Some(ref key) = self.encryption_key {
            for row in &mut result {
                row.text = decrypt_at_rest(key, &row.text);
            }
        }
        Ok(result)
    }

    /// Get messages between two timestamps (oldest first).
    pub fn get_messages_between(
        &self,
        channel: &str,
        after: u64,
        before: u64,
        limit: usize,
    ) -> SqlResult<Vec<MessageRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, channel, sender, text, timestamp, tags_json, msgid, replaces_msgid, deleted_at, sender_did
             FROM messages
             WHERE channel = ?1 AND deleted_at IS NULL AND timestamp > ?2 AND timestamp < ?3
             ORDER BY timestamp ASC, id ASC
             LIMIT ?4"
        )?;
        let rows = stmt.query_map(
            params![channel, after as i64, before as i64, limit as i64],
            map_message_row,
        )?;
        let mut result = rows.collect::<SqlResult<Vec<_>>>()?;
        if let Some(ref key) = self.encryption_key {
            for row in &mut result {
                row.text = decrypt_at_rest(key, &row.text);
            }
        }
        Ok(result)
    }

    /// Prune old messages for a channel, keeping only the most recent `max_keep`.
    pub fn prune_messages(&self, channel: &str, max_keep: usize) -> SqlResult<()> {
        self.conn.execute(
            "DELETE FROM messages WHERE channel = ?1 AND id NOT IN (
                SELECT id FROM messages WHERE channel = ?1 ORDER BY timestamp DESC, id DESC LIMIT ?2
            )",
            params![channel, max_keep as i64],
        )?;
        Ok(())
    }

    /// Find a message by its msgid. Returns the sender (hostmask) for authorship check.
    pub fn get_message_by_msgid(
        &self,
        channel: &str,
        msgid: &str,
    ) -> SqlResult<Option<MessageRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, channel, sender, text, timestamp, tags_json, msgid, replaces_msgid, deleted_at, sender_did
             FROM messages
             WHERE channel = ?1 AND msgid = ?2
             LIMIT 1"
        )?;
        let mut rows = stmt.query_map(params![channel, msgid], map_message_row)?;
        match rows.next() {
            Some(row) => {
                let mut msg = row?;
                if let Some(ref key) = self.encryption_key {
                    msg.text = decrypt_at_rest(key, &msg.text);
                }
                Ok(Some(msg))
            }
            None => Ok(None),
        }
    }

    /// Find a message by msgid across all channels.
    pub fn find_message_by_msgid(&self, msgid: &str) -> SqlResult<Option<MessageRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, channel, sender, text, timestamp, tags_json, msgid, replaces_msgid, deleted_at, sender_did
             FROM messages
             WHERE msgid = ?1 AND deleted_at IS NULL
             LIMIT 1",
        )?;
        let mut rows = stmt.query_map(params![msgid], map_message_row)?;
        match rows.next() {
            Some(row) => {
                let mut msg = row?;
                if let Some(ref key) = self.encryption_key {
                    msg.text = decrypt_at_rest(key, &msg.text);
                }
                Ok(Some(msg))
            }
            None => Ok(None),
        }
    }

    /// Soft-delete a message by setting deleted_at timestamp.
    pub fn soft_delete_message(&self, channel: &str, msgid: &str) -> SqlResult<usize> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let changed = self.conn.execute(
            "UPDATE messages SET deleted_at = ?1 WHERE channel = ?2 AND msgid = ?3 AND deleted_at IS NULL",
            params![now as i64, channel, msgid],
        )?;
        Ok(changed)
    }

    /// Store an edit (a new message that replaces an old one).
    pub fn insert_edit(
        &self,
        channel: &str,
        sender: &str,
        text: &str,
        timestamp: u64,
        tags: &HashMap<String, String>,
        msgid: &str,
        replaces_msgid: &str,
        sender_did: Option<&str>,
    ) -> SqlResult<()> {
        let tags_json = serde_json::to_string(tags).unwrap_or_else(|_| "{}".to_string());
        let stored_text = if let Some(ref key) = self.encryption_key {
            encrypt_at_rest(key, text)
        } else {
            text.to_string()
        };
        self.conn.execute(
            "INSERT INTO messages (channel, sender, text, timestamp, tags_json, msgid, replaces_msgid, sender_did)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![channel, sender, stored_text, timestamp as i64, tags_json, msgid, replaces_msgid, sender_did],
        )?;
        Ok(())
    }

    // ── Reactions ──────────────────────────────────────────────────────

    /// Store a reaction. Upsert — duplicate (msgid, nick, emoji) is ignored.
    pub fn store_reaction(
        &self,
        target_msgid: &str,
        channel: &str,
        reactor_nick: &str,
        reactor_did: Option<&str>,
        emoji: &str,
        timestamp: u64,
    ) -> SqlResult<()> {
        self.conn.execute(
            "INSERT OR IGNORE INTO reactions (target_msgid, channel, reactor_nick, reactor_did, emoji, timestamp)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![target_msgid, channel, reactor_nick, reactor_did, emoji, timestamp as i64],
        )?;
        Ok(())
    }

    /// Remove a reaction.
    pub fn remove_reaction(
        &self,
        target_msgid: &str,
        reactor_nick: &str,
        emoji: &str,
    ) -> SqlResult<usize> {
        let changed = self.conn.execute(
            "DELETE FROM reactions WHERE target_msgid = ?1 AND reactor_nick = ?2 AND emoji = ?3",
            params![target_msgid, reactor_nick, emoji],
        )?;
        Ok(changed)
    }

    /// Get reactions for a list of message IDs, grouped by msgid -> emoji -> nicks.
    pub fn get_reactions_for_messages(
        &self,
        msgids: &[&str],
    ) -> SqlResult<HashMap<String, Vec<ReactionRow>>> {
        if msgids.is_empty() {
            return Ok(HashMap::new());
        }
        let placeholders: Vec<String> = msgids.iter().enumerate().map(|(i, _)| format!("?{}", i + 1)).collect();
        let sql = format!(
            "SELECT target_msgid, channel, reactor_nick, reactor_did, emoji, timestamp
             FROM reactions WHERE target_msgid IN ({})
             ORDER BY timestamp ASC",
            placeholders.join(", ")
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let params: Vec<&dyn rusqlite::ToSql> = msgids.iter().map(|s| s as &dyn rusqlite::ToSql).collect();
        let rows = stmt.query_map(params.as_slice(), |row| {
            Ok(ReactionRow {
                target_msgid: row.get(0)?,
                channel: row.get(1)?,
                reactor_nick: row.get(2)?,
                reactor_did: row.get(3)?,
                emoji: row.get(4)?,
                timestamp: row.get::<_, i64>(5)? as u64,
            })
        })?;
        let mut result: HashMap<String, Vec<ReactionRow>> = HashMap::new();
        for row in rows {
            let row = row?;
            result.entry(row.target_msgid.clone()).or_default().push(row);
        }
        Ok(result)
    }

    // ── Pins ──────────────────────────────────────────────────────────

    /// Store a pin. Duplicate (channel, msgid) is ignored.
    pub fn store_pin(
        &self,
        channel: &str,
        msgid: &str,
        pinned_by: &str,
        pinned_at: u64,
    ) -> SqlResult<()> {
        self.conn.execute(
            "INSERT OR IGNORE INTO pins (channel, msgid, pinned_by, pinned_at)
             VALUES (?1, ?2, ?3, ?4)",
            params![channel, msgid, pinned_by, pinned_at as i64],
        )?;
        Ok(())
    }

    /// Remove a pin.
    pub fn remove_pin(&self, channel: &str, msgid: &str) -> SqlResult<usize> {
        let changed = self.conn.execute(
            "DELETE FROM pins WHERE channel = ?1 AND msgid = ?2",
            params![channel, msgid],
        )?;
        Ok(changed)
    }

    /// Get all pins for a channel, most recent first.
    pub fn get_pins(&self, channel: &str) -> SqlResult<Vec<crate::server::PinnedMessage>> {
        let mut stmt = self.conn.prepare(
            "SELECT msgid, pinned_by, pinned_at FROM pins
             WHERE channel = ?1
             ORDER BY pinned_at DESC"
        )?;
        let rows = stmt.query_map(params![channel], |row| {
            Ok(crate::server::PinnedMessage {
                msgid: row.get(0)?,
                pinned_by: row.get(1)?,
                pinned_at: row.get::<_, i64>(2)? as u64,
            })
        })?;
        rows.collect()
    }

    /// Get raw (potentially encrypted) message text for testing.
    /// Returns the stored text without decryption.
    pub fn get_raw_message_text(&self, channel: &str, timestamp: u64) -> SqlResult<String> {
        self.conn.query_row(
            "SELECT text FROM messages WHERE channel = ?1 AND timestamp = ?2",
            params![channel, timestamp as i64],
            |row| row.get(0),
        )
    }

    /// List DM conversations for a given DID, ordered by most recent message.
    /// Returns (canonical_dm_key, last_message_timestamp) pairs.
    pub fn dm_conversations(
        &self,
        did: &str,
        limit: usize,
    ) -> SqlResult<Vec<(String, u64)>> {
        let pattern = format!("%{did}%");
        let mut stmt = self.conn.prepare(
            "SELECT channel, MAX(timestamp) AS last_ts
             FROM messages
             WHERE channel LIKE 'dm:%' AND channel LIKE ?1
               AND deleted_at IS NULL
             GROUP BY channel
             ORDER BY last_ts DESC
             LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![pattern, limit as i64], |row| {
            let channel: String = row.get(0)?;
            let ts: i64 = row.get(1)?;
            Ok((channel, ts as u64))
        })?;
        rows.collect()
    }

    /// Edit a message (update text by msgid).
    pub fn edit_message(
        &self,
        msgid: &str,
        _sender: &str,
        new_text: &str,
        new_msgid: Option<&str>,
    ) -> SqlResult<()> {
        let stored_text = if let Some(ref key) = self.encryption_key {
            encrypt_at_rest(key, new_text)
        } else {
            new_text.to_string()
        };
        if let Some(new_id) = new_msgid {
            self.conn.execute(
                "UPDATE messages SET text = ?1, replaces_msgid = ?2 WHERE msgid = ?3",
                params![stored_text, new_id, msgid],
            )?;
        } else {
            self.conn.execute(
                "UPDATE messages SET text = ?1 WHERE msgid = ?2",
                params![stored_text, msgid],
            )?;
        }
        Ok(())
    }

    // ── Pre-key bundles (E2EE) ────────────────────────────────────────

    /// Store or update a pre-key bundle for a DID.
    pub fn save_prekey_bundle(&self, did: &str, bundle_json: &str) -> SqlResult<()> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        self.conn.execute(
            "INSERT INTO prekey_bundles (did, bundle_json, updated_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(did) DO UPDATE SET bundle_json=excluded.bundle_json, updated_at=excluded.updated_at",
            params![did, bundle_json, now as i64],
        )?;
        Ok(())
    }

    /// Load a pre-key bundle for a DID.
    pub fn get_prekey_bundle(&self, did: &str) -> SqlResult<Option<serde_json::Value>> {
        let mut stmt = self
            .conn
            .prepare("SELECT bundle_json FROM prekey_bundles WHERE did = ?1")?;
        let mut rows = stmt.query_map(params![did], |row| {
            let json_str: String = row.get(0)?;
            Ok(serde_json::from_str(&json_str).unwrap_or(serde_json::Value::Null))
        })?;
        match rows.next() {
            Some(row) => Ok(Some(row?)),
            None => Ok(None),
        }
    }

    /// Load all pre-key bundles (for populating in-memory cache on startup).
    pub fn load_all_prekey_bundles(&self) -> SqlResult<Vec<(String, serde_json::Value)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT did, bundle_json FROM prekey_bundles")?;
        let rows = stmt.query_map([], |row| {
            let did: String = row.get(0)?;
            let json_str: String = row.get(1)?;
            let bundle = serde_json::from_str(&json_str).unwrap_or(serde_json::Value::Null);
            Ok((did, bundle))
        })?;
        rows.collect()
    }

    // ── Per-DID signing keys (MSGSIG) ────────────────────────────────
    //
    // When a session sends `MSGSIG <pubkey>`, the connection layer mirrors it
    // here so we can verify signatures from that DID across server restarts
    // and even when the DID has no active session. Used by:
    //   • PROVENANCE FreeqBotDelegation/v1 cert verification
    //   • (future) cross-session signature checks for offline signers

    /// UPSERT the registered signing key for a DID. `pubkey` must be 32 bytes.
    pub fn save_signing_key(&self, did: &str, pubkey: &[u8]) -> SqlResult<()> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        self.conn.execute(
            "INSERT INTO signing_keys (did, pubkey, registered_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(did) DO UPDATE SET pubkey=excluded.pubkey, registered_at=excluded.registered_at",
            params![did, pubkey, now as i64],
        )?;
        Ok(())
    }

    /// Look up the registered signing key for a DID. Returns the raw 32-byte
    /// ed25519 public key, or None if the DID has never registered one.
    pub fn get_signing_key(&self, did: &str) -> SqlResult<Option<[u8; 32]>> {
        let mut stmt = self
            .conn
            .prepare("SELECT pubkey FROM signing_keys WHERE did = ?1")?;
        let mut rows = stmt.query_map(params![did], |row| row.get::<_, Vec<u8>>(0))?;
        match rows.next() {
            Some(row) => {
                let bytes = row?;
                if bytes.len() != 32 {
                    return Ok(None);
                }
                let mut out = [0u8; 32];
                out.copy_from_slice(&bytes);
                Ok(Some(out))
            }
            None => Ok(None),
        }
    }

    // ── User channel persistence (auto-rejoin) ────────────────────────

    /// Record that a DID-authenticated user has joined a channel.
    pub fn add_user_channel(&self, did: &str, channel: &str) -> SqlResult<()> {
        self.conn.execute(
            "INSERT OR IGNORE INTO user_channels (did, channel) VALUES (?1, ?2)",
            params![did, channel],
        )?;
        Ok(())
    }

    /// Record that a DID-authenticated user has left a channel.
    pub fn remove_user_channel(&self, did: &str, channel: &str) -> SqlResult<()> {
        self.conn.execute(
            "DELETE FROM user_channels WHERE did = ?1 AND channel = ?2",
            params![did, channel],
        )?;
        Ok(())
    }

    /// Get all channels a DID-authenticated user was last in.
    pub fn get_user_channels(&self, did: &str) -> SqlResult<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT channel FROM user_channels WHERE did = ?1")?;
        let rows = stmt.query_map(params![did], |row| row.get(0))?;
        rows.collect()
    }

    // ── Identities (DID-nick bindings) ─────────────────────────────────

    /// Bind a DID to a nick. Overwrites any previous binding for that DID.
    pub fn save_identity(&self, did: &str, nick: &str) -> SqlResult<()> {
        self.conn.execute(
            "INSERT INTO identities (did, nick) VALUES (?1, ?2)
             ON CONFLICT(did) DO UPDATE SET nick=excluded.nick",
            params![did, nick],
        )?;
        Ok(())
    }

    /// Load all DID-nick bindings.
    pub fn load_identities(&self) -> SqlResult<Vec<IdentityRow>> {
        let mut stmt = self.conn.prepare("SELECT did, nick FROM identities")?;
        let rows = stmt.query_map([], |row| {
            Ok(IdentityRow {
                did: row.get(0)?,
                nick: row.get(1)?,
            })
        })?;
        rows.collect()
    }

    /// Look up a DID by nick.
    pub fn get_identity_by_nick(&self, nick: &str) -> SqlResult<Option<IdentityRow>> {
        let mut stmt = self
            .conn
            .prepare("SELECT did, nick FROM identities WHERE nick = ?1")?;
        let mut rows = stmt.query_map(params![nick], |row| {
            Ok(IdentityRow {
                did: row.get(0)?,
                nick: row.get(1)?,
            })
        })?;
        match rows.next() {
            Some(row) => Ok(Some(row?)),
            None => Ok(None),
        }
    }

    /// Look up a nick by DID.
    pub fn get_identity_by_did(&self, did: &str) -> SqlResult<Option<IdentityRow>> {
        let mut stmt = self
            .conn
            .prepare("SELECT did, nick FROM identities WHERE did = ?1")?;
        let mut rows = stmt.query_map(params![did], |row| {
            Ok(IdentityRow {
                did: row.get(0)?,
                nick: row.get(1)?,
            })
        })?;
        match rows.next() {
            Some(row) => Ok(Some(row?)),
            None => Ok(None),
        }
    }
}

fn map_message_row(row: &rusqlite::Row) -> SqlResult<MessageRow> {
    let tags_json: String = row.get(5)?;
    let tags: HashMap<String, String> = serde_json::from_str(&tags_json).unwrap_or_default();
    // New columns may not exist in old schemas — handle gracefully
    let msgid: Option<String> = row.get(6).unwrap_or(None);
    let replaces_msgid: Option<String> = row.get(7).unwrap_or(None);
    let deleted_at: Option<u64> = row
        .get::<_, Option<i64>>(8)
        .unwrap_or(None)
        .map(|v| v as u64);
    let sender_did: Option<String> = row.get(9).unwrap_or(None);
    Ok(MessageRow {
        id: row.get(0)?,
        channel: row.get(1)?,
        sender: row.get(2)?,
        text: row.get(3)?,
        timestamp: row.get::<_, i64>(4)? as u64,
        tags,
        msgid,
        replaces_msgid,
        deleted_at,
        sender_did,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::BanEntry;

    #[test]
    fn roundtrip_channel_state() {
        let db = Db::open_memory().unwrap();

        let mut ch = ChannelState::default();
        ch.topic = Some(TopicInfo {
            text: "Hello world".to_string(),
            set_by: "alice!a@host".to_string(),
            set_at: 1700000000,
        });
        ch.topic_locked = true;
        ch.invite_only = false;
        ch.key = Some("secret".to_string());

        db.save_channel("#test", &ch).unwrap();

        let loaded = db.load_channels().unwrap();
        let loaded_ch = loaded.get("#test").unwrap();
        assert!(loaded_ch.topic.is_some());
        let t = loaded_ch.topic.as_ref().unwrap();
        assert_eq!(t.text, "Hello world");
        assert_eq!(t.set_by, "alice!a@host");
        assert_eq!(t.set_at, 1700000000);
        assert!(loaded_ch.topic_locked);
        assert!(!loaded_ch.invite_only);
        assert_eq!(loaded_ch.key.as_deref(), Some("secret"));
        // Runtime state should be empty
        assert!(loaded_ch.members.is_empty());
        assert!(loaded_ch.ops.is_empty());
    }

    #[test]
    fn roundtrip_bans() {
        let db = Db::open_memory().unwrap();

        // Must create the channel first
        let ch = ChannelState::default();
        db.save_channel("#test", &ch).unwrap();

        let ban = BanEntry {
            mask: "bad!*@*".to_string(),
            set_by: "op!o@host".to_string(),
            set_at: 1700000000,
        };
        db.add_ban("#test", &ban).unwrap();

        let ban2 = BanEntry {
            mask: "did:plc:abc".to_string(),
            set_by: "op!o@host".to_string(),
            set_at: 1700000001,
        };
        db.add_ban("#test", &ban2).unwrap();

        let loaded = db.load_channels().unwrap();
        let loaded_ch = loaded.get("#test").unwrap();
        assert_eq!(loaded_ch.bans.len(), 2);
        assert_eq!(loaded_ch.bans[0].mask, "bad!*@*");
        assert_eq!(loaded_ch.bans[1].mask, "did:plc:abc");

        // Remove one
        db.remove_ban("#test", "bad!*@*").unwrap();
        let loaded = db.load_channels().unwrap();
        let loaded_ch = loaded.get("#test").unwrap();
        assert_eq!(loaded_ch.bans.len(), 1);
        assert_eq!(loaded_ch.bans[0].mask, "did:plc:abc");
    }

    #[test]
    fn roundtrip_messages() {
        let db = Db::open_memory().unwrap();

        let mut tags = HashMap::new();
        tags.insert("content-type".to_string(), "image/jpeg".to_string());

        db.insert_message(
            "#test",
            "alice!a@host",
            "hello",
            1000,
            &HashMap::new(),
            Some("01TEST00000000000000000001"),
            None,
        )
        .unwrap();
        db.insert_message(
            "#test",
            "bob!b@host",
            "world",
            1001,
            &tags,
            Some("01TEST00000000000000000002"),
            None,
        )
        .unwrap();
        db.insert_message(
            "#test",
            "alice!a@host",
            "third",
            1002,
            &HashMap::new(),
            Some("01TEST00000000000000000003"),
            None,
        )
        .unwrap();

        // Get last 2
        let msgs = db.get_messages("#test", 2, None).unwrap();
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].text, "world");
        assert_eq!(msgs[0].tags.get("content-type").unwrap(), "image/jpeg");
        assert_eq!(msgs[1].text, "third");

        // Paginate: before timestamp 1002
        let msgs = db.get_messages("#test", 10, Some(1002)).unwrap();
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].text, "hello");
        assert_eq!(msgs[1].text, "world");
    }

    #[test]
    fn roundtrip_identities() {
        let db = Db::open_memory().unwrap();

        db.save_identity("did:plc:alice", "alice").unwrap();
        db.save_identity("did:plc:bob", "bob").unwrap();

        let all = db.load_identities().unwrap();
        assert_eq!(all.len(), 2);

        let by_nick = db.get_identity_by_nick("alice").unwrap().unwrap();
        assert_eq!(by_nick.did, "did:plc:alice");

        let by_did = db.get_identity_by_did("did:plc:bob").unwrap().unwrap();
        assert_eq!(by_did.nick, "bob");

        // Update nick
        db.save_identity("did:plc:alice", "alice2").unwrap();
        let updated = db.get_identity_by_did("did:plc:alice").unwrap().unwrap();
        assert_eq!(updated.nick, "alice2");

        // Old nick no longer resolves
        assert!(db.get_identity_by_nick("alice").unwrap().is_none());
    }

    #[test]
    fn channel_delete_cascades_bans() {
        let db = Db::open_memory().unwrap();
        let ch = ChannelState::default();
        db.save_channel("#test", &ch).unwrap();
        let ban = BanEntry {
            mask: "bad!*@*".to_string(),
            set_by: "op".to_string(),
            set_at: 0,
        };
        db.add_ban("#test", &ban).unwrap();

        db.delete_channel("#test").unwrap();

        let loaded = db.load_channels().unwrap();
        assert!(!loaded.contains_key("#test"));
    }

    #[test]
    fn messages_different_channels() {
        let db = Db::open_memory().unwrap();
        db.insert_message("#a", "u", "msg-a", 1000, &HashMap::new(), None, None)
            .unwrap();
        db.insert_message("#b", "u", "msg-b", 1001, &HashMap::new(), None, None)
            .unwrap();

        let a = db.get_messages("#a", 100, None).unwrap();
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].text, "msg-a");

        let b = db.get_messages("#b", 100, None).unwrap();
        assert_eq!(b.len(), 1);
        assert_eq!(b[0].text, "msg-b");
    }

    #[test]
    fn duplicate_ban_ignored() {
        let db = Db::open_memory().unwrap();
        let ch = ChannelState::default();
        db.save_channel("#test", &ch).unwrap();
        let ban = BanEntry {
            mask: "bad!*@*".to_string(),
            set_by: "op".to_string(),
            set_at: 0,
        };
        db.add_ban("#test", &ban).unwrap();
        db.add_ban("#test", &ban).unwrap(); // should not error

        let loaded = db.load_channels().unwrap();
        assert_eq!(loaded.get("#test").unwrap().bans.len(), 1);
    }

    #[test]
    fn store_and_get_reactions() {
        let db = Db::open_memory().unwrap();
        db.store_reaction("msg001", "#test", "alice", Some("did:plc:alice"), "👍", 1000).unwrap();
        db.store_reaction("msg001", "#test", "bob", None, "👍", 1001).unwrap();
        db.store_reaction("msg001", "#test", "alice", Some("did:plc:alice"), "❤️", 1002).unwrap();

        let reactions = db.get_reactions_for_messages(&["msg001"]).unwrap();
        let msg_reactions = reactions.get("msg001").unwrap();
        assert_eq!(msg_reactions.len(), 3);
        assert_eq!(msg_reactions[0].reactor_nick, "alice");
        assert_eq!(msg_reactions[0].emoji, "👍");
        assert_eq!(msg_reactions[1].reactor_nick, "bob");
        assert_eq!(msg_reactions[2].emoji, "❤️");
    }

    #[test]
    fn duplicate_reaction_ignored() {
        let db = Db::open_memory().unwrap();
        db.store_reaction("msg001", "#test", "alice", None, "👍", 1000).unwrap();
        db.store_reaction("msg001", "#test", "alice", None, "👍", 1001).unwrap(); // duplicate

        let reactions = db.get_reactions_for_messages(&["msg001"]).unwrap();
        assert_eq!(reactions.get("msg001").unwrap().len(), 1);
    }

    #[test]
    fn remove_reaction() {
        let db = Db::open_memory().unwrap();
        db.store_reaction("msg001", "#test", "alice", None, "👍", 1000).unwrap();
        db.store_reaction("msg001", "#test", "alice", None, "❤️", 1001).unwrap();

        let removed = db.remove_reaction("msg001", "alice", "👍").unwrap();
        assert_eq!(removed, 1);

        let reactions = db.get_reactions_for_messages(&["msg001"]).unwrap();
        let msg_reactions = reactions.get("msg001").unwrap();
        assert_eq!(msg_reactions.len(), 1);
        assert_eq!(msg_reactions[0].emoji, "❤️");
    }

    #[test]
    fn get_reactions_multiple_messages() {
        let db = Db::open_memory().unwrap();
        db.store_reaction("msg001", "#test", "alice", None, "👍", 1000).unwrap();
        db.store_reaction("msg002", "#test", "bob", None, "🎉", 1001).unwrap();
        db.store_reaction("msg003", "#test", "carol", None, "❤️", 1002).unwrap();

        let reactions = db.get_reactions_for_messages(&["msg001", "msg002", "msg003"]).unwrap();
        assert!(reactions.contains_key("msg001"));
        assert!(reactions.contains_key("msg002"));
        assert!(reactions.contains_key("msg003"));
    }

    #[test]
    fn get_reactions_empty_input() {
        let db = Db::open_memory().unwrap();
        let reactions = db.get_reactions_for_messages(&[]).unwrap();
        assert!(reactions.is_empty());
    }

    #[test]
    fn get_reactions_no_matches() {
        let db = Db::open_memory().unwrap();
        let reactions = db.get_reactions_for_messages(&["nonexistent"]).unwrap();
        assert!(reactions.is_empty());
    }

    // ── Pin persistence tests ──

    #[test]
    fn store_and_get_pins() {
        let db = Db::open_memory().unwrap();
        db.store_pin("#test", "msg001", "alice", 1000).unwrap();
        db.store_pin("#test", "msg002", "bob", 1001).unwrap();

        let pins = db.get_pins("#test").unwrap();
        assert_eq!(pins.len(), 2);
        // Most recent first
        assert_eq!(pins[0].msgid, "msg002");
        assert_eq!(pins[0].pinned_by, "bob");
        assert_eq!(pins[1].msgid, "msg001");
    }

    #[test]
    fn duplicate_pin_ignored() {
        let db = Db::open_memory().unwrap();
        db.store_pin("#test", "msg001", "alice", 1000).unwrap();
        db.store_pin("#test", "msg001", "bob", 1001).unwrap(); // same msgid

        let pins = db.get_pins("#test").unwrap();
        assert_eq!(pins.len(), 1);
        assert_eq!(pins[0].pinned_by, "alice"); // first pinner wins
    }

    #[test]
    fn remove_pin() {
        let db = Db::open_memory().unwrap();
        db.store_pin("#test", "msg001", "alice", 1000).unwrap();
        db.store_pin("#test", "msg002", "bob", 1001).unwrap();

        let removed = db.remove_pin("#test", "msg001").unwrap();
        assert_eq!(removed, 1);

        let pins = db.get_pins("#test").unwrap();
        assert_eq!(pins.len(), 1);
        assert_eq!(pins[0].msgid, "msg002");
    }

    #[test]
    fn remove_nonexistent_pin() {
        let db = Db::open_memory().unwrap();
        let removed = db.remove_pin("#test", "nonexistent").unwrap();
        assert_eq!(removed, 0);
    }

    #[test]
    fn pins_separate_per_channel() {
        let db = Db::open_memory().unwrap();
        db.store_pin("#chan1", "msg001", "alice", 1000).unwrap();
        db.store_pin("#chan2", "msg002", "bob", 1001).unwrap();

        let pins1 = db.get_pins("#chan1").unwrap();
        let pins2 = db.get_pins("#chan2").unwrap();
        assert_eq!(pins1.len(), 1);
        assert_eq!(pins2.len(), 1);
        assert_eq!(pins1[0].msgid, "msg001");
        assert_eq!(pins2[0].msgid, "msg002");
    }

    #[test]
    fn load_pins_on_channel_startup() {
        let db = Db::open_memory().unwrap();
        let ch = ChannelState::default();
        db.save_channel("#test", &ch).unwrap();
        db.store_pin("#test", "msg001", "alice", 1000).unwrap();
        db.store_pin("#test", "msg002", "bob", 1001).unwrap();

        let channels = db.load_channels().unwrap();
        let loaded = channels.get("#test").unwrap();
        assert_eq!(loaded.pins.len(), 2);
        assert_eq!(loaded.pins[0].msgid, "msg002"); // most recent first
    }

    #[test]
    fn signing_key_roundtrip_and_upsert() {
        let db = Db::open_memory().unwrap();
        let did = "did:plc:abc";
        assert!(db.get_signing_key(did).unwrap().is_none());

        let key1 = [1u8; 32];
        db.save_signing_key(did, &key1).unwrap();
        assert_eq!(db.get_signing_key(did).unwrap(), Some(key1));

        // UPSERT overwrites with the new key
        let key2 = [2u8; 32];
        db.save_signing_key(did, &key2).unwrap();
        assert_eq!(db.get_signing_key(did).unwrap(), Some(key2));

        // Different DID is independent
        db.save_signing_key("did:plc:xyz", &[9u8; 32]).unwrap();
        assert_eq!(db.get_signing_key(did).unwrap(), Some(key2));
        assert_eq!(db.get_signing_key("did:plc:xyz").unwrap(), Some([9u8; 32]));
    }

    #[test]
    fn signing_key_rejects_wrong_length_on_read() {
        // If somehow a non-32-byte blob ends up in the table, the read API
        // returns None rather than panicking. This guards against a future
        // schema change or a manual DB edit.
        let db = Db::open_memory().unwrap();
        db.conn
            .execute(
                "INSERT INTO signing_keys (did, pubkey, registered_at) VALUES (?1, ?2, ?3)",
                params!["did:plc:short", &[1u8; 16][..], 0i64],
            )
            .unwrap();
        assert!(db.get_signing_key("did:plc:short").unwrap().is_none());
    }
}

// ── Agent governance DB methods ────────────────────────────────────

/// A capability grant row.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CapabilityGrantRow {
    pub id: i64,
    pub channel: String,
    pub agent_did: String,
    pub capability: String,
    pub scope: Option<String>,
    pub ttl_seconds: u64,
    pub requires_approval: bool,
    pub rate_limit: u32,
    pub granted_by: String,
    pub granted_at: i64,
    pub expires_at: Option<i64>,
    pub revoked_at: Option<i64>,
}

/// A governance log entry.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GovernanceLogEntry {
    pub id: i64,
    pub channel: Option<String>,
    pub target_did: String,
    pub action: String,
    pub issued_by: String,
    pub reason: Option<String>,
    pub timestamp: i64,
}

/// A pending approval row.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PendingApprovalRow {
    pub id: String,
    pub channel: String,
    pub agent_did: String,
    pub capability: String,
    pub resource: Option<String>,
    pub requested_at: i64,
    pub granted_by: Option<String>,
    pub granted_at: Option<i64>,
    pub denied_by: Option<String>,
    pub denied_at: Option<i64>,
    pub deny_reason: Option<String>,
    pub expires_at: Option<i64>,
}

impl Db {
    // ── Capability grants ──────────────────────────────────────────

    pub fn grant_capability(
        &self,
        channel: &str,
        agent_did: &str,
        capability: &str,
        scope: Option<&str>,
        ttl_seconds: u64,
        requires_approval: bool,
        rate_limit: u32,
        granted_by: &str,
    ) -> SqlResult<i64> {
        let now = chrono::Utc::now().timestamp();
        let expires_at = if ttl_seconds > 0 {
            Some(now + ttl_seconds as i64)
        } else {
            None
        };
        self.conn.execute(
            "INSERT INTO agent_capability_grants
             (channel, agent_did, capability, scope, ttl_seconds, requires_approval, rate_limit, granted_by, granted_at, expires_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
             ON CONFLICT(channel, agent_did, capability, scope) DO UPDATE SET
                ttl_seconds=excluded.ttl_seconds,
                requires_approval=excluded.requires_approval,
                rate_limit=excluded.rate_limit,
                granted_by=excluded.granted_by,
                granted_at=excluded.granted_at,
                expires_at=excluded.expires_at,
                revoked_at=NULL",
            params![channel, agent_did, capability, scope, ttl_seconds as i64, requires_approval as i32, rate_limit as i32, granted_by, now, expires_at],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn get_capabilities(&self, channel: &str, agent_did: &str) -> Vec<CapabilityGrantRow> {
        let mut stmt = self.conn
            .prepare(
                "SELECT id, channel, agent_did, capability, scope, ttl_seconds, requires_approval, rate_limit, granted_by, granted_at, expires_at, revoked_at
                 FROM agent_capability_grants
                 WHERE channel = ?1 AND agent_did = ?2 AND revoked_at IS NULL
                   AND (expires_at IS NULL OR expires_at > ?3)"
            )
            .unwrap();
        let now = chrono::Utc::now().timestamp();
        stmt.query_map(params![channel, agent_did, now], |row| {
            Ok(CapabilityGrantRow {
                id: row.get(0)?,
                channel: row.get(1)?,
                agent_did: row.get(2)?,
                capability: row.get(3)?,
                scope: row.get(4)?,
                ttl_seconds: row.get::<_, i64>(5)? as u64,
                requires_approval: row.get::<_, i32>(6)? != 0,
                rate_limit: row.get::<_, i32>(7)? as u32,
                granted_by: row.get(8)?,
                granted_at: row.get(9)?,
                expires_at: row.get(10)?,
                revoked_at: row.get(11)?,
            })
        })
        .unwrap()
        .filter_map(|r| r.ok())
        .collect()
    }

    pub fn revoke_capability(&self, grant_id: i64) -> SqlResult<()> {
        let now = chrono::Utc::now().timestamp();
        self.conn.execute(
            "UPDATE agent_capability_grants SET revoked_at = ?1 WHERE id = ?2",
            params![now, grant_id],
        )?;
        Ok(())
    }

    pub fn revoke_all_capabilities(&self, channel: &str, agent_did: &str) -> SqlResult<usize> {
        let now = chrono::Utc::now().timestamp();
        let count = self.conn.execute(
            "UPDATE agent_capability_grants SET revoked_at = ?1
             WHERE channel = ?2 AND agent_did = ?3 AND revoked_at IS NULL",
            params![now, channel, agent_did],
        )?;
        Ok(count)
    }

    pub fn expire_capabilities(&self) -> SqlResult<usize> {
        let now = chrono::Utc::now().timestamp();
        let count = self.conn.execute(
            "UPDATE agent_capability_grants SET revoked_at = ?1
             WHERE expires_at IS NOT NULL AND expires_at < ?1 AND revoked_at IS NULL",
            params![now],
        )?;
        Ok(count)
    }

    // ── Governance log ─────────────────────────────────────────────

    pub fn log_governance(
        &self,
        channel: Option<&str>,
        target_did: &str,
        action: &str,
        issued_by: &str,
        reason: Option<&str>,
    ) -> SqlResult<()> {
        let now = chrono::Utc::now().timestamp();
        self.conn.execute(
            "INSERT INTO governance_log (channel, target_did, action, issued_by, reason, timestamp)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![channel, target_did, action, issued_by, reason, now],
        )?;
        Ok(())
    }

    // ── Pending approvals ──────────────────────────────────────────

    pub fn create_approval(
        &self,
        id: &str,
        channel: &str,
        agent_did: &str,
        capability: &str,
        resource: Option<&str>,
    ) -> SqlResult<()> {
        let now = chrono::Utc::now().timestamp();
        let expires_at = now + 3600; // 1 hour
        self.conn.execute(
            "INSERT INTO pending_approvals (id, channel, agent_did, capability, resource, requested_at, expires_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![id, channel, agent_did, capability, resource, now, expires_at],
        )?;
        Ok(())
    }

    pub fn grant_approval(&self, id: &str, granted_by: &str) -> SqlResult<bool> {
        let now = chrono::Utc::now().timestamp();
        let count = self.conn.execute(
            "UPDATE pending_approvals SET granted_by = ?1, granted_at = ?2
             WHERE id = ?3 AND granted_by IS NULL AND denied_by IS NULL
               AND (expires_at IS NULL OR expires_at > ?2)",
            params![granted_by, now, id],
        )?;
        Ok(count > 0)
    }

    pub fn deny_approval(&self, id: &str, denied_by: &str, reason: Option<&str>) -> SqlResult<bool> {
        let now = chrono::Utc::now().timestamp();
        let count = self.conn.execute(
            "UPDATE pending_approvals SET denied_by = ?1, denied_at = ?2, deny_reason = ?3
             WHERE id = ?4 AND granted_by IS NULL AND denied_by IS NULL",
            params![denied_by, now, reason, id],
        )?;
        Ok(count > 0)
    }

    pub fn get_pending_approvals(&self, channel: &str) -> Vec<PendingApprovalRow> {
        let mut stmt = self.conn
            .prepare(
                "SELECT id, channel, agent_did, capability, resource, requested_at,
                        granted_by, granted_at, denied_by, denied_at, deny_reason, expires_at
                 FROM pending_approvals
                 WHERE channel = ?1 AND granted_by IS NULL AND denied_by IS NULL
                   AND (expires_at IS NULL OR expires_at > ?2)
                 ORDER BY requested_at ASC"
            )
            .unwrap();
        let now = chrono::Utc::now().timestamp();
        stmt.query_map(params![channel, now], |row| {
            Ok(PendingApprovalRow {
                id: row.get(0)?,
                channel: row.get(1)?,
                agent_did: row.get(2)?,
                capability: row.get(3)?,
                resource: row.get(4)?,
                requested_at: row.get(5)?,
                granted_by: row.get(6)?,
                granted_at: row.get(7)?,
                denied_by: row.get(8)?,
                denied_at: row.get(9)?,
                deny_reason: row.get(10)?,
                expires_at: row.get(11)?,
            })
        })
        .unwrap()
        .filter_map(|r| r.ok())
        .collect()
    }

    pub fn find_pending_approval_for_agent(
        &self,
        channel: &str,
        agent_did: &str,
        capability: &str,
    ) -> Option<PendingApprovalRow> {
        self.get_pending_approvals(channel)
            .into_iter()
            .find(|a| a.agent_did == agent_did && a.capability == capability)
    }
}

/// A spend record.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SpendRecord {
    pub id: i64,
    pub channel: String,
    pub agent_did: String,
    pub amount: f64,
    pub unit: String,
    pub description: Option<String>,
    pub task_ref: Option<String>,
    pub timestamp: i64,
}

/// A spawned agent record.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SpawnedAgentRow {
    pub child_did: String,
    pub parent_did: String,
    pub parent_session: String,
    pub nick: String,
    pub channel: String,
    pub capabilities_json: String,
    pub ttl_seconds: Option<u64>,
    pub task_ref: Option<String>,
    pub spawned_at: i64,
}

// ── Coordination events DB methods ─────────────────────────────────

/// A coordination event row.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CoordinationEventRow {
    pub event_id: String,
    pub event_type: String,
    pub actor_did: String,
    pub channel: String,
    pub ref_id: Option<String>,
    pub payload_json: String,
    pub signature: Option<String>,
    pub timestamp: i64,
}

impl Db {
    pub fn store_coordination_event(&self, event: &CoordinationEventRow) -> SqlResult<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO coordination_events
             (event_id, event_type, actor_did, channel, ref_id, payload_json, signature, timestamp)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                event.event_id,
                event.event_type,
                event.actor_did,
                event.channel,
                event.ref_id,
                event.payload_json,
                event.signature,
                event.timestamp,
            ],
        )?;
        Ok(())
    }

    /// Query coordination events with optional filters.
    pub fn query_coordination_events(
        &self,
        channel: &str,
        event_type: Option<&str>,
        ref_id: Option<&str>,
        actor_did: Option<&str>,
        since: Option<i64>,
        limit: usize,
    ) -> Vec<CoordinationEventRow> {
        let mut sql = String::from(
            "SELECT event_id, event_type, actor_did, channel, ref_id, payload_json, signature, timestamp
             FROM coordination_events WHERE channel = ?1"
        );
        let mut param_idx = 2;
        let mut params_vec: Vec<Box<dyn rusqlite::types::ToSql>> = vec![Box::new(channel.to_string())];

        if let Some(et) = event_type {
            sql.push_str(&format!(" AND event_type = ?{param_idx}"));
            params_vec.push(Box::new(et.to_string()));
            param_idx += 1;
        }
        if let Some(ri) = ref_id {
            sql.push_str(&format!(" AND ref_id = ?{param_idx}"));
            params_vec.push(Box::new(ri.to_string()));
            param_idx += 1;
        }
        if let Some(ad) = actor_did {
            sql.push_str(&format!(" AND actor_did = ?{param_idx}"));
            params_vec.push(Box::new(ad.to_string()));
            param_idx += 1;
        }
        if let Some(s) = since {
            sql.push_str(&format!(" AND timestamp >= ?{param_idx}"));
            params_vec.push(Box::new(s));
            param_idx += 1;
        }
        let _ = param_idx; // suppress unused warning
        sql.push_str(&format!(" ORDER BY timestamp ASC LIMIT {limit}"));

        let params_refs: Vec<&dyn rusqlite::types::ToSql> = params_vec.iter().map(|b| b.as_ref()).collect();
        let mut stmt = match self.conn.prepare(&sql) {
            Ok(s) => s,
            Err(e) => {
                tracing::error!("Failed to prepare coordination query: {e}");
                return Vec::new();
            }
        };
        match stmt.query_map(params_refs.as_slice(), |row| {
            Ok(CoordinationEventRow {
                event_id: row.get(0)?,
                event_type: row.get(1)?,
                actor_did: row.get(2)?,
                channel: row.get(3)?,
                ref_id: row.get(4)?,
                payload_json: row.get(5)?,
                signature: row.get(6)?,
                timestamp: row.get(7)?,
            })
        }) {
            Ok(rows) => rows.filter_map(|r| r.ok()).collect(),
            Err(_) => Vec::new(),
        }
    }

    /// Get a task and all its related events.
    pub fn get_task(&self, task_id: &str) -> Option<CoordinationEventRow> {
        let mut stmt = self.conn.prepare(
            "SELECT event_id, event_type, actor_did, channel, ref_id, payload_json, signature, timestamp
             FROM coordination_events WHERE event_id = ?1 AND event_type = 'task_request'"
        ).ok()?;
        stmt.query_row(params![task_id], |row| {
            Ok(CoordinationEventRow {
                event_id: row.get(0)?,
                event_type: row.get(1)?,
                actor_did: row.get(2)?,
                channel: row.get(3)?,
                ref_id: row.get(4)?,
                payload_json: row.get(5)?,
                signature: row.get(6)?,
                timestamp: row.get(7)?,
            })
        }).ok()
    }

    /// Get all events referencing a task ID.
    pub fn get_task_events(&self, task_id: &str) -> Vec<CoordinationEventRow> {
        self.query_coordination_events("", None, Some(task_id), None, None, 1000)
            .into_iter()
            .collect()
    }

    /// Get task events regardless of channel (by ref_id).
    pub fn get_task_events_all_channels(&self, task_id: &str) -> Vec<CoordinationEventRow> {
        let mut stmt = match self.conn.prepare(
            "SELECT event_id, event_type, actor_did, channel, ref_id, payload_json, signature, timestamp
             FROM coordination_events WHERE ref_id = ?1 ORDER BY timestamp ASC"
        ) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        match stmt.query_map(params![task_id], |row| {
            Ok(CoordinationEventRow {
                event_id: row.get(0)?,
                event_type: row.get(1)?,
                actor_did: row.get(2)?,
                channel: row.get(3)?,
                ref_id: row.get(4)?,
                payload_json: row.get(5)?,
                signature: row.get(6)?,
                timestamp: row.get(7)?,
            })
        }) {
            Ok(rows) => rows.filter_map(|r| r.ok()).collect(),
            Err(_) => Vec::new(),
        }
    }

    // ── Agent manifests ──────────────────────────────────────────────

    pub fn save_manifest(
        &self,
        agent_did: &str,
        manifest_json: &str,
        manifest_url: Option<&str>,
        registered_by: &str,
    ) -> SqlResult<()> {
        let now = chrono::Utc::now().timestamp();
        self.conn.execute(
            "INSERT OR REPLACE INTO agent_manifests
             (agent_did, manifest_json, manifest_url, registered_by, registered_at, active)
             VALUES (?1, ?2, ?3, ?4, ?5, 1)",
            params![agent_did, manifest_json, manifest_url, registered_by, now],
        )?;
        Ok(())
    }

    pub fn get_manifest(&self, agent_did: &str) -> Option<String> {
        self.conn
            .query_row(
                "SELECT manifest_json FROM agent_manifests WHERE agent_did = ?1 AND active = 1",
                params![agent_did],
                |row| row.get(0),
            )
            .ok()
    }

    pub fn deactivate_manifest(&self, agent_did: &str) -> SqlResult<()> {
        self.conn.execute(
            "UPDATE agent_manifests SET active = 0 WHERE agent_did = ?1",
            params![agent_did],
        )?;
        Ok(())
    }

    pub fn list_manifests(&self) -> Vec<(String, String, i64)> {
        let mut stmt = match self.conn.prepare(
            "SELECT agent_did, manifest_json, registered_at FROM agent_manifests WHERE active = 1 ORDER BY registered_at DESC",
        ) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        match stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
            ))
        }) {
            Ok(rows) => rows.filter_map(|r| r.ok()).collect(),
            Err(_) => Vec::new(),
        }
    }

    // ── Spawned agents ─────────────────────────────────────────────

    pub fn record_spawn(
        &self,
        child_did: &str,
        parent_did: &str,
        parent_session: &str,
        nick: &str,
        channel: &str,
        capabilities: &[String],
        ttl_seconds: Option<u64>,
        task_ref: Option<&str>,
    ) -> SqlResult<()> {
        let now = chrono::Utc::now().timestamp();
        let caps_json = serde_json::to_string(capabilities).unwrap_or_else(|_| "[]".to_string());
        self.conn.execute(
            "INSERT OR REPLACE INTO spawned_agents
             (child_did, parent_did, parent_session, nick, channel, capabilities_json, ttl_seconds, task_ref, spawned_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![child_did, parent_did, parent_session, nick, channel, caps_json, ttl_seconds.map(|t| t as i64), task_ref, now],
        )?;
        Ok(())
    }

    pub fn record_despawn(&self, child_did: &str) -> SqlResult<()> {
        let now = chrono::Utc::now().timestamp();
        self.conn.execute(
            "UPDATE spawned_agents SET despawned_at = ?1 WHERE child_did = ?2",
            params![now, child_did],
        )?;
        Ok(())
    }

    pub fn get_active_spawns(&self, parent_did: &str) -> Vec<SpawnedAgentRow> {
        let mut stmt = match self.conn.prepare(
            "SELECT child_did, parent_did, parent_session, nick, channel, capabilities_json, ttl_seconds, task_ref, spawned_at
             FROM spawned_agents WHERE parent_did = ?1 AND despawned_at IS NULL",
        ) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        match stmt.query_map(params![parent_did], |row| {
            Ok(SpawnedAgentRow {
                child_did: row.get(0)?,
                parent_did: row.get(1)?,
                parent_session: row.get(2)?,
                nick: row.get(3)?,
                channel: row.get(4)?,
                capabilities_json: row.get(5)?,
                ttl_seconds: row.get::<_, Option<i64>>(6)?.map(|t| t as u64),
                task_ref: row.get(7)?,
                spawned_at: row.get(8)?,
            })
        }) {
            Ok(rows) => rows.filter_map(|r| r.ok()).collect(),
            Err(_) => Vec::new(),
        }
    }

    pub fn get_spawn_by_nick(&self, channel: &str, nick: &str) -> Option<SpawnedAgentRow> {
        self.conn.query_row(
            "SELECT child_did, parent_did, parent_session, nick, channel, capabilities_json, ttl_seconds, task_ref, spawned_at
             FROM spawned_agents WHERE channel = ?1 AND nick = ?2 AND despawned_at IS NULL",
            params![channel, nick],
            |row| Ok(SpawnedAgentRow {
                child_did: row.get(0)?,
                parent_did: row.get(1)?,
                parent_session: row.get(2)?,
                nick: row.get(3)?,
                channel: row.get(4)?,
                capabilities_json: row.get(5)?,
                ttl_seconds: row.get::<_, Option<i64>>(6)?.map(|t| t as u64),
                task_ref: row.get(7)?,
                spawned_at: row.get(8)?,
            }),
        ).ok()
    }

    // ── Agent spend tracking ──────────────────────────────────────────

    pub fn record_spend(
        &self,
        channel: &str,
        agent_did: &str,
        amount: f64,
        unit: &str,
        description: Option<&str>,
        task_ref: Option<&str>,
    ) -> SqlResult<()> {
        let now = chrono::Utc::now().timestamp();
        self.conn.execute(
            "INSERT INTO agent_spend (channel, agent_did, amount, unit, description, task_ref, timestamp)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![channel, agent_did, amount, unit, description, task_ref, now],
        )?;
        Ok(())
    }

    /// Sum spend for a channel/agent/unit since a given timestamp.
    pub fn sum_spend(&self, channel: &str, agent_did: Option<&str>, unit: &str, since: i64) -> f64 {
        let (sql, params_vec): (String, Vec<Box<dyn rusqlite::types::ToSql>>) = match agent_did {
            Some(did) => (
                "SELECT COALESCE(SUM(amount), 0.0) FROM agent_spend
                 WHERE channel = ?1 AND agent_did = ?2 AND unit = ?3 AND timestamp >= ?4".to_string(),
                vec![Box::new(channel.to_string()), Box::new(did.to_string()), Box::new(unit.to_string()), Box::new(since)],
            ),
            None => (
                "SELECT COALESCE(SUM(amount), 0.0) FROM agent_spend
                 WHERE channel = ?1 AND unit = ?2 AND timestamp >= ?3".to_string(),
                vec![Box::new(channel.to_string()), Box::new(unit.to_string()), Box::new(since)],
            ),
        };
        let refs: Vec<&dyn rusqlite::types::ToSql> = params_vec.iter().map(|b| b.as_ref()).collect();
        self.conn.query_row(&sql, refs.as_slice(), |row| row.get(0)).unwrap_or(0.0)
    }

    /// Query spend records with optional filters.
    pub fn query_spend(
        &self,
        channel: &str,
        agent_did: Option<&str>,
        since: Option<i64>,
        limit: usize,
    ) -> Vec<SpendRecord> {
        let mut sql = String::from(
            "SELECT id, channel, agent_did, amount, unit, description, task_ref, timestamp
             FROM agent_spend WHERE channel = ?1"
        );
        let mut params_vec: Vec<Box<dyn rusqlite::types::ToSql>> = vec![Box::new(channel.to_string())];
        let mut idx = 2;
        if let Some(did) = agent_did {
            sql.push_str(&format!(" AND agent_did = ?{idx}"));
            params_vec.push(Box::new(did.to_string()));
            idx += 1;
        }
        if let Some(s) = since {
            sql.push_str(&format!(" AND timestamp >= ?{idx}"));
            params_vec.push(Box::new(s));
            idx += 1;
        }
        let _ = idx;
        sql.push_str(&format!(" ORDER BY timestamp DESC LIMIT {limit}"));
        let refs: Vec<&dyn rusqlite::types::ToSql> = params_vec.iter().map(|b| b.as_ref()).collect();
        let mut stmt = match self.conn.prepare(&sql) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        match stmt.query_map(refs.as_slice(), |row| {
            Ok(SpendRecord {
                id: row.get(0)?,
                channel: row.get(1)?,
                agent_did: row.get(2)?,
                amount: row.get(3)?,
                unit: row.get(4)?,
                description: row.get(5)?,
                task_ref: row.get(6)?,
                timestamp: row.get(7)?,
            })
        }) {
            Ok(rows) => rows.filter_map(|r| r.ok()).collect(),
            Err(_) => Vec::new(),
        }
    }

    /// Spend by agent for a channel/unit/period.
    pub fn spend_by_agent(&self, channel: &str, unit: &str, since: i64) -> Vec<(String, f64, i64)> {
        let mut stmt = match self.conn.prepare(
            "SELECT agent_did, SUM(amount), COUNT(*) FROM agent_spend
             WHERE channel = ?1 AND unit = ?2 AND timestamp >= ?3
             GROUP BY agent_did ORDER BY SUM(amount) DESC"
        ) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        match stmt.query_map(params![channel, unit, since], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, f64>(1)?, row.get::<_, i64>(2)?))
        }) {
            Ok(rows) => rows.filter_map(|r| r.ok()).collect(),
            Err(_) => Vec::new(),
        }
    }

    // ── Channel budgets ──────────────────────────────────────────────

    pub fn set_budget(
        &self,
        channel: &str,
        agent_did: Option<&str>,
        budget_json: &str,
        set_by: &str,
    ) -> SqlResult<()> {
        let now = chrono::Utc::now().timestamp();
        let did_key = agent_did.unwrap_or("*");
        self.conn.execute(
            "INSERT OR REPLACE INTO channel_budgets (channel, agent_did, budget_json, set_by, set_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![channel, did_key, budget_json, set_by, now],
        )?;
        Ok(())
    }

    pub fn get_budget(&self, channel: &str, agent_did: Option<&str>) -> Option<String> {
        let did_key = agent_did.unwrap_or("*");
        // Try agent-specific first, then channel default
        self.conn.query_row(
            "SELECT budget_json FROM channel_budgets WHERE channel = ?1 AND agent_did = ?2",
            params![channel, did_key],
            |row| row.get(0),
        ).ok().or_else(|| {
            if agent_did.is_some() {
                self.conn.query_row(
                    "SELECT budget_json FROM channel_budgets WHERE channel = ?1 AND agent_did = '*'",
                    params![channel],
                    |row| row.get(0),
                ).ok()
            } else {
                None
            }
        })
    }

    /// Query governance log entries for a channel.
    pub fn query_governance_log(&self, channel: Option<&str>, limit: usize) -> Vec<GovernanceLogEntry> {
        let (sql, params_vec): (String, Vec<Box<dyn rusqlite::types::ToSql>>) = match channel {
            Some(ch) => (
                "SELECT id, channel, target_did, action, issued_by, reason, timestamp
                 FROM governance_log WHERE channel = ?1 ORDER BY timestamp ASC LIMIT ?2".to_string(),
                vec![Box::new(ch.to_string()), Box::new(limit as i64)],
            ),
            None => (
                "SELECT id, channel, target_did, action, issued_by, reason, timestamp
                 FROM governance_log ORDER BY timestamp ASC LIMIT ?1".to_string(),
                vec![Box::new(limit as i64)],
            ),
        };
        let params_refs: Vec<&dyn rusqlite::types::ToSql> = params_vec.iter().map(|b| b.as_ref()).collect();
        let mut stmt = match self.conn.prepare(&sql) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        match stmt.query_map(params_refs.as_slice(), |row| {
            Ok(GovernanceLogEntry {
                id: row.get(0)?,
                channel: row.get(1)?,
                target_did: row.get(2)?,
                action: row.get(3)?,
                issued_by: row.get(4)?,
                reason: row.get(5)?,
                timestamp: row.get(6)?,
            })
        }) {
            Ok(rows) => rows.filter_map(|r| r.ok()).collect(),
            Err(_) => Vec::new(),
        }
    }

    // ── AV sessions ────────────────────────────────────────────────────

    pub fn save_av_session(&self, session: &crate::av::AvSession) -> SqlResult<()> {
        use crate::av::AvSessionState;
        let (ended_at, ended_by) = match &session.state {
            AvSessionState::Active => (None, None),
            AvSessionState::Ended { ended_at, ended_by } => {
                (Some(*ended_at), ended_by.clone())
            }
        };
        self.conn.execute(
            "INSERT INTO av_sessions (id, channel, created_by, created_at, ended_at, ended_by, title, iroh_ticket, backend, recording, max_participants)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
             ON CONFLICT(id) DO UPDATE SET
                ended_at=excluded.ended_at,
                ended_by=excluded.ended_by",
            params![
                session.id,
                session.channel,
                session.created_by,
                session.created_at,
                ended_at,
                ended_by,
                session.title,
                session.iroh_ticket,
                serde_json::to_string(&session.media_backend).unwrap_or_default(),
                session.recording_enabled,
                session.max_participants,
            ],
        )?;
        for p in session.participants.values() {
            self.conn.execute(
                "INSERT INTO av_participants (session_id, did, nick, joined_at, left_at, role)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(session_id, did) DO UPDATE SET
                    left_at=excluded.left_at, role=excluded.role",
                params![
                    session.id, p.did, p.nick, p.joined_at, p.left_at,
                    serde_json::to_string(&p.role).unwrap_or_default(),
                ],
            )?;
        }
        Ok(())
    }

    pub fn save_av_artifact(&self, artifact: &crate::av::AvArtifact) -> SqlResult<()> {
        self.conn.execute(
            "INSERT INTO av_artifacts (id, session_id, kind, created_at, created_by, content_ref, content_type, visibility, title)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                artifact.id, artifact.session_id,
                serde_json::to_string(&artifact.kind).unwrap_or_default(),
                artifact.created_at, artifact.created_by,
                artifact.content_ref, artifact.content_type,
                serde_json::to_string(&artifact.visibility).unwrap_or_default(),
                artifact.title,
            ],
        )?;
        Ok(())
    }

    pub fn list_av_artifacts(&self, session_id: &str) -> SqlResult<Vec<crate::av::AvArtifact>> {
        use crate::av::{AvArtifact, ArtifactKind, ArtifactVisibility};
        let mut stmt = self.conn.prepare(
            "SELECT id, session_id, kind, created_at, created_by, content_ref, content_type, visibility, title
             FROM av_artifacts WHERE session_id = ?1 ORDER BY created_at",
        )?;
        let rows = stmt.query_map([session_id], |row: &rusqlite::Row| {
            let kind_str: String = row.get(2)?;
            let vis_str: String = row.get(7)?;
            Ok(AvArtifact {
                id: row.get(0)?,
                session_id: row.get(1)?,
                kind: serde_json::from_str(&kind_str).unwrap_or(ArtifactKind::Summary),
                created_at: row.get(3)?,
                created_by: row.get(4)?,
                content_ref: row.get(5)?,
                content_type: row.get(6)?,
                visibility: serde_json::from_str(&vis_str).unwrap_or(ArtifactVisibility::Participants),
                title: row.get(8)?,
            })
        })?;
        rows.collect()
    }

    /// Load all active (non-ended) AV sessions with their participants. Used on server restart.
    pub fn load_active_av_sessions(&self) -> SqlResult<Vec<crate::av::AvSession>> {
        use crate::av::{AvSession, AvSessionState, AvParticipant, ParticipantRole, MediaBackendType};
        use std::collections::HashMap;
        let mut stmt = self.conn.prepare(
            "SELECT id, channel, created_by, created_at, title, iroh_ticket, backend, recording, max_participants
             FROM av_sessions WHERE ended_at IS NULL",
        )?;
        let mut sessions: Vec<AvSession> = stmt.query_map([], |row: &rusqlite::Row| {
            let backend_str: String = row.get(6)?;
            Ok(AvSession {
                id: row.get(0)?,
                channel: row.get(1)?,
                created_by: row.get(2)?,
                created_by_nick: String::new(),
                created_at: row.get(3)?,
                state: AvSessionState::Active,
                participants: HashMap::new(),
                title: row.get(4)?,
                iroh_ticket: row.get(5)?,
                media_backend: serde_json::from_str(&backend_str).unwrap_or(MediaBackendType::IrohLive),
                recording_enabled: row.get(7)?,
                max_participants: row.get(8)?,
            })
        })?.filter_map(|r| r.ok()).collect();

        // Load participants for each session
        for session in &mut sessions {
            let mut pstmt = self.conn.prepare(
                "SELECT did, nick, joined_at, left_at, role FROM av_participants WHERE session_id = ?1",
            )?;
            let participants: Vec<AvParticipant> = pstmt.query_map([&session.id], |row: &rusqlite::Row| {
                let role_str: String = row.get(4)?;
                Ok(AvParticipant {
                    did: row.get(0)?,
                    nick: row.get(1)?,
                    joined_at: row.get(2)?,
                    left_at: row.get(3)?,
                    role: serde_json::from_str(&role_str).unwrap_or(ParticipantRole::Speaker),
                    tracks: vec![],
                })
            })?.filter_map(|r| r.ok()).collect();
            for p in participants {
                // Also recover created_by_nick from the host participant
                if p.did == session.created_by {
                    session.created_by_nick = p.nick.clone();
                }
                session.participants.insert(p.did.clone(), p);
            }
        }
        Ok(sessions)
    }

    pub fn list_channel_av_sessions(&self, channel: &str, limit: u32) -> SqlResult<Vec<crate::av::AvSession>> {
        use crate::av::{AvSession, AvSessionState, MediaBackendType};
        use std::collections::HashMap;
        let mut stmt = self.conn.prepare(
            "SELECT id, channel, created_by, created_at, ended_at, ended_by, title, iroh_ticket, backend, recording, max_participants
             FROM av_sessions WHERE channel = ?1 ORDER BY created_at DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![channel, limit], |row: &rusqlite::Row| {
            let ended_at: Option<i64> = row.get(4)?;
            let ended_by: Option<String> = row.get(5)?;
            let backend_str: String = row.get(8)?;
            let state = match ended_at {
                Some(ea) => AvSessionState::Ended { ended_at: ea, ended_by },
                None => AvSessionState::Active,
            };
            Ok(AvSession {
                id: row.get(0)?,
                channel: row.get(1)?,
                created_by: row.get(2)?,
                created_by_nick: String::new(),
                created_at: row.get(3)?,
                state,
                participants: HashMap::new(),
                title: row.get(6)?,
                iroh_ticket: row.get(7)?,
                media_backend: serde_json::from_str(&backend_str).unwrap_or(MediaBackendType::IrohLive),
                recording_enabled: row.get(9)?,
                max_participants: row.get(10)?,
            })
        })?;
        rows.collect()
    }
}
