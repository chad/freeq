//! SQLite persistence layer.
//!
//! Stores message history, channel state, bans, and DID-nick identity bindings.
//! Uses WAL mode for concurrent reads during writes.

use std::collections::HashMap;
use std::path::Path;

use rusqlite::{params, Connection, Result as SqlResult};

use crate::server::{BanEntry, ChannelState, TopicInfo};

/// Prefix for encrypted-at-rest message content.
const EAR_PREFIX: &str = "EAR1:";

/// Encrypt text with AES-256-GCM for storage at rest.
fn encrypt_at_rest(key: &[u8; 32], plaintext: &str) -> String {
    use aes_gcm::{Aes256Gcm, KeyInit, aead::Aead, Nonce};
    let cipher = Aes256Gcm::new(key.into());
    let nonce_bytes: [u8; 12] = rand::random();
    let nonce = Nonce::from_slice(&nonce_bytes);
    match cipher.encrypt(nonce, plaintext.as_bytes()) {
        Ok(ct) => {
            use base64::Engine;
            let mut combined = Vec::with_capacity(12 + ct.len());
            combined.extend_from_slice(&nonce_bytes);
            combined.extend_from_slice(&ct);
            format!("{EAR_PREFIX}{}", base64::engine::general_purpose::STANDARD.encode(&combined))
        }
        Err(_) => plaintext.to_string(), // fallback: store plaintext
    }
}

/// Decrypt text from at-rest storage. Returns plaintext if not encrypted.
fn decrypt_at_rest(key: &[u8; 32], stored: &str) -> String {
    if !stored.starts_with(EAR_PREFIX) {
        return stored.to_string(); // not encrypted — return as-is (legacy data)
    }
    use aes_gcm::{Aes256Gcm, KeyInit, aead::Aead, Nonce};
    use base64::Engine;
    let b64 = &stored[EAR_PREFIX.len()..];
    match base64::engine::general_purpose::STANDARD.decode(b64) {
        Ok(combined) if combined.len() > 12 => {
            let nonce = Nonce::from_slice(&combined[..12]);
            let ct = &combined[12..];
            let cipher = Aes256Gcm::new(key.into());
            match cipher.decrypt(nonce, ct) {
                Ok(pt) => String::from_utf8_lossy(&pt).to_string(),
                Err(_) => stored.to_string(), // can't decrypt — return raw
            }
        }
        _ => stored.to_string(),
    }
}

/// Database handle wrapping a SQLite connection.
pub struct Db {
    conn: Connection,
    /// AES-256-GCM key for encrypting message content at rest.
    /// Derived from the server's signing key. If None, messages stored as plaintext.
    encryption_key: Option<[u8; 32]>,
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
        let db = Self { conn, encryption_key: None };
        db.init()?;
        Ok(db)
    }

    /// Open a database with encryption at rest for message content.
    pub fn open_encrypted<P: AsRef<Path>>(path: P, key: [u8; 32]) -> SqlResult<Self> {
        let conn = Connection::open(path)?;
        let db = Self { conn, encryption_key: Some(key) };
        db.init()?;
        Ok(db)
    }

    /// Open an in-memory database (for testing).
    pub fn open_memory() -> SqlResult<Self> {
        let conn = Connection::open_in_memory()?;
        let db = Self { conn, encryption_key: None };
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
        ];
        for sql in &migrations {
            // Ignore "duplicate column name" errors — means column already exists
            let _ = self.conn.execute(sql, []);
        }

        Ok(())
    }

    // ── Channel state ──────────────────────────────────────────────────

    /// Save or update a channel's metadata (topic, modes, key).
    pub fn save_channel(&self, name: &str, ch: &ChannelState) -> SqlResult<()> {
        let did_ops_json = serde_json::to_string(
            &ch.did_ops.iter().collect::<Vec<_>>()
        ).unwrap_or_else(|_| "[]".to_string());
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
        self.conn.execute("DELETE FROM channels WHERE name = ?1", params![name])?;
        self.conn.execute("DELETE FROM bans WHERE channel = ?1", params![name])?;
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
            let did_ops_json: String = row.get::<_, Option<String>>(10)?.unwrap_or_else(|| "[]".to_string());

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

            let mut ch = ChannelState::default();
            ch.topic = topic;
            ch.topic_locked = topic_locked;
            ch.invite_only = invite_only;
            ch.no_ext_msg = no_ext_msg;
            ch.moderated = moderated;
            ch.key = key;
            ch.founder_did = founder_did;
            ch.did_ops = did_ops;
            Ok((name, ch))
        })?;

        for row in rows {
            let (name, ch) = row?;
            channels.insert(name, ch);
        }

        // Load bans
        let mut stmt = self.conn.prepare("SELECT channel, mask, set_by, set_at FROM bans")?;
        let ban_rows = stmt.query_map([], |row| {
            let channel: String = row.get(0)?;
            let mask: String = row.get(1)?;
            let set_by: String = row.get(2)?;
            let set_at: i64 = row.get(3)?;
            Ok((channel, BanEntry { mask, set_by, set_at: set_at as u64 }))
        })?;

        for row in ban_rows {
            let (channel, ban) = row?;
            if let Some(ch) = channels.get_mut(&channel) {
                ch.bans.push(ban);
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
    ) -> SqlResult<()> {
        let tags_json = serde_json::to_string(tags).unwrap_or_else(|_| "{}".to_string());
        let stored_text = if let Some(ref key) = self.encryption_key {
            encrypt_at_rest(key, text)
        } else {
            text.to_string()
        };
        self.conn.execute(
            "INSERT INTO messages (channel, sender, text, timestamp, tags_json, msgid)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![channel, sender, stored_text, timestamp as i64, tags_json, msgid],
        )?;
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
                "SELECT id, channel, sender, text, timestamp, tags_json, msgid, replaces_msgid, deleted_at
                 FROM messages
                 WHERE channel = ?1 AND deleted_at IS NULL AND timestamp < ?2
                 ORDER BY timestamp DESC, id DESC
                 LIMIT ?3"
            )?;
            let rows = stmt.query_map(params![channel, before_ts as i64, limit as i64], map_message_row)?;
            rows.collect::<SqlResult<Vec<_>>>()?
        } else {
            let mut stmt = self.conn.prepare(
                "SELECT id, channel, sender, text, timestamp, tags_json, msgid, replaces_msgid, deleted_at
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
            "SELECT id, channel, sender, text, timestamp, tags_json, msgid, replaces_msgid, deleted_at
             FROM messages
             WHERE channel = ?1 AND deleted_at IS NULL AND timestamp > ?2
             ORDER BY timestamp ASC, id ASC
             LIMIT ?3"
        )?;
        let rows = stmt.query_map(params![channel, after as i64, limit as i64], map_message_row)?;
        let mut result = rows.collect::<SqlResult<Vec<_>>>()?;
        if let Some(ref key) = self.encryption_key {
            for row in &mut result { row.text = decrypt_at_rest(key, &row.text); }
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
            "SELECT id, channel, sender, text, timestamp, tags_json, msgid, replaces_msgid, deleted_at
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
            for row in &mut result { row.text = decrypt_at_rest(key, &row.text); }
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
    pub fn get_message_by_msgid(&self, channel: &str, msgid: &str) -> SqlResult<Option<MessageRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, channel, sender, text, timestamp, tags_json, msgid, replaces_msgid, deleted_at
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
    ) -> SqlResult<()> {
        let tags_json = serde_json::to_string(tags).unwrap_or_else(|_| "{}".to_string());
        let stored_text = if let Some(ref key) = self.encryption_key {
            encrypt_at_rest(key, text)
        } else {
            text.to_string()
        };
        self.conn.execute(
            "INSERT INTO messages (channel, sender, text, timestamp, tags_json, msgid, replaces_msgid)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![channel, sender, stored_text, timestamp as i64, tags_json, msgid, replaces_msgid],
        )?;
        Ok(())
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
        let mut stmt = self.conn.prepare("SELECT did, nick FROM identities WHERE nick = ?1")?;
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
        let mut stmt = self.conn.prepare("SELECT did, nick FROM identities WHERE did = ?1")?;
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
    let tags: HashMap<String, String> =
        serde_json::from_str(&tags_json).unwrap_or_default();
    // New columns may not exist in old schemas — handle gracefully
    let msgid: Option<String> = row.get(6).unwrap_or(None);
    let replaces_msgid: Option<String> = row.get(7).unwrap_or(None);
    let deleted_at: Option<u64> = row.get::<_, Option<i64>>(8)
        .unwrap_or(None)
        .map(|v| v as u64);
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

        let ban = BanEntry { mask: "bad!*@*".to_string(), set_by: "op!o@host".to_string(), set_at: 1700000000 };
        db.add_ban("#test", &ban).unwrap();

        let ban2 = BanEntry { mask: "did:plc:abc".to_string(), set_by: "op!o@host".to_string(), set_at: 1700000001 };
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

        db.insert_message("#test", "alice!a@host", "hello", 1000, &HashMap::new(), Some("01TEST00000000000000000001")).unwrap();
        db.insert_message("#test", "bob!b@host", "world", 1001, &tags, Some("01TEST00000000000000000002")).unwrap();
        db.insert_message("#test", "alice!a@host", "third", 1002, &HashMap::new(), Some("01TEST00000000000000000003")).unwrap();

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
        let ban = BanEntry { mask: "bad!*@*".to_string(), set_by: "op".to_string(), set_at: 0 };
        db.add_ban("#test", &ban).unwrap();

        db.delete_channel("#test").unwrap();

        let loaded = db.load_channels().unwrap();
        assert!(!loaded.contains_key("#test"));
    }

    #[test]
    fn messages_different_channels() {
        let db = Db::open_memory().unwrap();
        db.insert_message("#a", "u", "msg-a", 1000, &HashMap::new(), None).unwrap();
        db.insert_message("#b", "u", "msg-b", 1001, &HashMap::new(), None).unwrap();

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
        let ban = BanEntry { mask: "bad!*@*".to_string(), set_by: "op".to_string(), set_at: 0 };
        db.add_ban("#test", &ban).unwrap();
        db.add_ban("#test", &ban).unwrap(); // should not error

        let loaded = db.load_channels().unwrap();
        assert_eq!(loaded.get("#test").unwrap().bans.len(), 1);
    }
}
