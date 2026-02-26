//! Database storage for policy framework objects.
//!
//! Uses SQLite (via rusqlite) alongside the existing IRC database.

use super::canonical;
use super::types::*;
use rusqlite::{params, Connection, OptionalExtension};
use parking_lot::Mutex;

pub struct PolicyStore {
    db: Mutex<Connection>,
}

impl PolicyStore {
    /// Open or create the policy database.
    pub fn open(path: &str) -> Result<Self, rusqlite::Error> {
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL;")?;
        let store = PolicyStore {
            db: Mutex::new(conn),
        };
        store.migrate()?;
        Ok(store)
    }

    fn migrate(&self) -> Result<(), rusqlite::Error> {
        let db = self.db.lock();
        db.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS policies (
                policy_id TEXT PRIMARY KEY,
                channel_id TEXT NOT NULL,
                version INTEGER NOT NULL,
                effective_at TEXT NOT NULL,
                previous_policy_hash TEXT,
                authority_set_hash TEXT NOT NULL,
                document_json TEXT NOT NULL,
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                UNIQUE(channel_id, version)
            );

            CREATE INDEX IF NOT EXISTS idx_policies_channel ON policies(channel_id);

            CREATE TABLE IF NOT EXISTS authority_sets (
                authority_set_hash TEXT PRIMARY KEY,
                channel_id TEXT NOT NULL,
                document_json TEXT NOT NULL,
                previous_authority_set_hash TEXT,
                created_at TEXT NOT NULL DEFAULT (datetime('now'))
            );

            CREATE INDEX IF NOT EXISTS idx_authority_sets_channel ON authority_sets(channel_id);

            CREATE TABLE IF NOT EXISTS join_receipts (
                join_id TEXT PRIMARY KEY,
                channel_id TEXT NOT NULL,
                policy_id TEXT NOT NULL,
                subject_did TEXT NOT NULL,
                receipt_json TEXT NOT NULL,
                state TEXT NOT NULL DEFAULT 'JOIN_PENDING',
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at TEXT NOT NULL DEFAULT (datetime('now'))
            );

            CREATE INDEX IF NOT EXISTS idx_join_receipts_channel ON join_receipts(channel_id);
            CREATE INDEX IF NOT EXISTS idx_join_receipts_did ON join_receipts(subject_did);

            CREATE TABLE IF NOT EXISTS membership_attestations (
                attestation_id TEXT PRIMARY KEY,
                channel_id TEXT NOT NULL,
                policy_id TEXT NOT NULL,
                authority_set_hash TEXT NOT NULL,
                subject_did TEXT NOT NULL,
                role TEXT NOT NULL,
                issued_at TEXT NOT NULL,
                expires_at TEXT,
                join_id TEXT,
                issuer_did TEXT NOT NULL,
                attestation_json TEXT NOT NULL,
                attestation_hash TEXT NOT NULL,
                state TEXT NOT NULL DEFAULT 'VALID',
                created_at TEXT NOT NULL DEFAULT (datetime('now'))
            );

            CREATE INDEX IF NOT EXISTS idx_attestations_channel ON membership_attestations(channel_id);
            CREATE INDEX IF NOT EXISTS idx_attestations_did ON membership_attestations(subject_did);
            CREATE INDEX IF NOT EXISTS idx_attestations_channel_did ON membership_attestations(channel_id, subject_did);

            CREATE TABLE IF NOT EXISTS transparency_log (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                entry_version INTEGER NOT NULL DEFAULT 1,
                channel_id TEXT NOT NULL,
                policy_id TEXT NOT NULL,
                attestation_hash TEXT NOT NULL,
                issued_at TEXT NOT NULL,
                issuer_authority_id TEXT NOT NULL,
                created_at TEXT NOT NULL DEFAULT (datetime('now'))
            );

            CREATE INDEX IF NOT EXISTS idx_tlog_channel ON transparency_log(channel_id);

            CREATE TABLE IF NOT EXISTS credentials (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                subject_did TEXT NOT NULL,
                credential_type TEXT NOT NULL,
                issuer TEXT NOT NULL,
                metadata_json TEXT NOT NULL DEFAULT '{}',
                issued_at TEXT NOT NULL DEFAULT (datetime('now')),
                revoked INTEGER NOT NULL DEFAULT 0,
                UNIQUE(subject_did, credential_type, issuer)
            );

            CREATE INDEX IF NOT EXISTS idx_credentials_did ON credentials(subject_did);

            CREATE TABLE IF NOT EXISTS signed_tree_heads (
                log_id TEXT NOT NULL,
                tree_size INTEGER NOT NULL,
                root_hash TEXT NOT NULL,
                timestamp TEXT NOT NULL,
                authority_id TEXT NOT NULL,
                signature TEXT NOT NULL,
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                PRIMARY KEY (log_id, tree_size)
            );
            ",
        )?;
        Ok(())
    }

    // ─── Policy Documents ────────────────────────────────────────────────

    /// Store a policy document. Computes policy_id from JCS hash.
    pub fn store_policy(&self, mut policy: PolicyDocument) -> Result<PolicyDocument, PolicyError> {
        // Compute policy_id by hashing the document without the policy_id field
        policy.policy_id = None;
        let policy_id = canonical::hash_canonical(&policy)
            .map_err(|e| PolicyError::Serialization(e.to_string()))?;
        policy.policy_id = Some(policy_id.clone());

        let json = serde_json::to_string(&policy)
            .map_err(|e| PolicyError::Serialization(e.to_string()))?;

        let db = self.db.lock();
        db.execute(
            "INSERT INTO policies (policy_id, channel_id, version, effective_at, previous_policy_hash, authority_set_hash, document_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                policy_id,
                policy.channel_id,
                policy.version,
                policy.effective_at,
                policy.previous_policy_hash,
                policy.authority_set_hash,
                json,
            ],
        )
        .map_err(|e| PolicyError::Database(e.to_string()))?;

        Ok(policy)
    }

    /// Get the current (latest version) policy for a channel.
    pub fn get_current_policy(
        &self,
        channel_id: &str,
    ) -> Result<Option<PolicyDocument>, PolicyError> {
        let db = self.db.lock();
        let json: Option<String> = db
            .query_row(
                "SELECT document_json FROM policies WHERE channel_id = ?1 ORDER BY version DESC LIMIT 1",
                params![channel_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| PolicyError::Database(e.to_string()))?;

        match json {
            Some(j) => {
                let doc: PolicyDocument = serde_json::from_str(&j)
                    .map_err(|e| PolicyError::Serialization(e.to_string()))?;
                Ok(Some(doc))
            }
            None => Ok(None),
        }
    }

    /// Get a policy by its hash.
    pub fn get_policy_by_hash(
        &self,
        policy_id: &str,
    ) -> Result<Option<PolicyDocument>, PolicyError> {
        let db = self.db.lock();
        let json: Option<String> = db
            .query_row(
                "SELECT document_json FROM policies WHERE policy_id = ?1",
                params![policy_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| PolicyError::Database(e.to_string()))?;

        match json {
            Some(j) => {
                let doc: PolicyDocument = serde_json::from_str(&j)
                    .map_err(|e| PolicyError::Serialization(e.to_string()))?;
                Ok(Some(doc))
            }
            None => Ok(None),
        }
    }

    /// Get all policy versions for a channel, oldest first.
    pub fn get_policy_chain(
        &self,
        channel_id: &str,
    ) -> Result<Vec<PolicyDocument>, PolicyError> {
        let db = self.db.lock();
        let mut stmt = db
            .prepare("SELECT document_json FROM policies WHERE channel_id = ?1 ORDER BY version ASC")
            .map_err(|e| PolicyError::Database(e.to_string()))?;

        let docs = stmt
            .query_map(params![channel_id], |row| {
                let json: String = row.get(0)?;
                Ok(json)
            })
            .map_err(|e| PolicyError::Database(e.to_string()))?
            .filter_map(|r| r.ok())
            .filter_map(|j| serde_json::from_str::<PolicyDocument>(&j).ok())
            .collect();

        Ok(docs)
    }

    // ─── Authority Sets ──────────────────────────────────────────────────

    /// Store an authority set. Computes hash from JCS.
    pub fn store_authority_set(
        &self,
        mut auth_set: AuthoritySet,
    ) -> Result<AuthoritySet, PolicyError> {
        auth_set.authority_set_hash = None;
        let hash = canonical::hash_canonical(&auth_set)
            .map_err(|e| PolicyError::Serialization(e.to_string()))?;
        auth_set.authority_set_hash = Some(hash.clone());

        let json = serde_json::to_string(&auth_set)
            .map_err(|e| PolicyError::Serialization(e.to_string()))?;

        let db = self.db.lock();
        db.execute(
            "INSERT INTO authority_sets (authority_set_hash, channel_id, document_json, previous_authority_set_hash)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                hash,
                auth_set.channel_id,
                json,
                auth_set.previous_authority_set_hash,
            ],
        )
        .map_err(|e| PolicyError::Database(e.to_string()))?;

        Ok(auth_set)
    }

    /// Get an authority set by its hash.
    pub fn get_authority_set(
        &self,
        hash: &str,
    ) -> Result<Option<AuthoritySet>, PolicyError> {
        let db = self.db.lock();
        let json: Option<String> = db
            .query_row(
                "SELECT document_json FROM authority_sets WHERE authority_set_hash = ?1",
                params![hash],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| PolicyError::Database(e.to_string()))?;

        match json {
            Some(j) => {
                let doc: AuthoritySet = serde_json::from_str(&j)
                    .map_err(|e| PolicyError::Serialization(e.to_string()))?;
                Ok(Some(doc))
            }
            None => Ok(None),
        }
    }

    // ─── Join Receipts ───────────────────────────────────────────────────

    /// Store a join receipt.
    pub fn store_join_receipt(&self, receipt: &JoinReceipt) -> Result<(), PolicyError> {
        let json = serde_json::to_string(receipt)
            .map_err(|e| PolicyError::Serialization(e.to_string()))?;

        let db = self.db.lock();
        db.execute(
            "INSERT OR REPLACE INTO join_receipts (join_id, channel_id, policy_id, subject_did, receipt_json, state, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, 'JOIN_PENDING', datetime('now'))",
            params![
                receipt.join_id,
                receipt.channel_id,
                receipt.policy_id,
                receipt.subject_did,
                json,
            ],
        )
        .map_err(|e| PolicyError::Database(e.to_string()))?;

        Ok(())
    }

    /// Update join state.
    pub fn update_join_state(
        &self,
        join_id: &str,
        state: JoinState,
    ) -> Result<(), PolicyError> {
        let state_str = serde_json::to_value(state)
            .map_err(|e| PolicyError::Serialization(e.to_string()))?
            .as_str()
            .unwrap_or("JOIN_FAILED")
            .to_string();

        let db = self.db.lock();
        db.execute(
            "UPDATE join_receipts SET state = ?1, updated_at = datetime('now') WHERE join_id = ?2",
            params![state_str, join_id],
        )
        .map_err(|e| PolicyError::Database(e.to_string()))?;

        Ok(())
    }

    /// Get a join receipt by join_id.
    pub fn get_join_receipt(&self, join_id: &str) -> Result<Option<JoinReceipt>, PolicyError> {
        let db = self.db.lock();
        let json: Option<String> = db
            .query_row(
                "SELECT receipt_json FROM join_receipts WHERE join_id = ?1",
                params![join_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| PolicyError::Database(e.to_string()))?;

        match json {
            Some(j) => {
                let doc: JoinReceipt = serde_json::from_str(&j)
                    .map_err(|e| PolicyError::Serialization(e.to_string()))?;
                Ok(Some(doc))
            }
            None => Ok(None),
        }
    }

    // ─── Membership Attestations ─────────────────────────────────────────

    /// Store a membership attestation and add to transparency log.
    pub fn store_attestation(
        &self,
        attestation: &MembershipAttestation,
    ) -> Result<(), PolicyError> {
        let json = serde_json::to_string(attestation)
            .map_err(|e| PolicyError::Serialization(e.to_string()))?;
        let attestation_hash = canonical::hash_canonical(attestation)
            .map_err(|e| PolicyError::Serialization(e.to_string()))?;

        let db = self.db.lock();

        // Store attestation
        db.execute(
            "INSERT INTO membership_attestations
             (attestation_id, channel_id, policy_id, authority_set_hash, subject_did, role, issued_at, expires_at, join_id, issuer_did, attestation_json, attestation_hash)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                attestation.attestation_id,
                attestation.channel_id,
                attestation.policy_id,
                attestation.authority_set_hash,
                attestation.subject_did,
                attestation.role,
                attestation.issued_at,
                attestation.expires_at,
                attestation.join_id,
                attestation.issuer_did,
                json,
                attestation_hash,
            ],
        )
        .map_err(|e| PolicyError::Database(e.to_string()))?;

        // Add to transparency log
        db.execute(
            "INSERT INTO transparency_log (entry_version, channel_id, policy_id, attestation_hash, issued_at, issuer_authority_id)
             VALUES (1, ?1, ?2, ?3, ?4, ?5)",
            params![
                attestation.channel_id,
                attestation.policy_id,
                attestation_hash,
                attestation.issued_at,
                attestation.issuer_did,
            ],
        )
        .map_err(|e| PolicyError::Database(e.to_string()))?;

        Ok(())
    }

    /// Get the current valid attestation for a user in a channel.
    pub fn get_attestation(
        &self,
        channel_id: &str,
        subject_did: &str,
    ) -> Result<Option<MembershipAttestation>, PolicyError> {
        let db = self.db.lock();
        let json: Option<String> = db
            .query_row(
                "SELECT attestation_json FROM membership_attestations
                 WHERE channel_id = ?1 AND subject_did = ?2 AND state = 'VALID'
                 ORDER BY issued_at DESC LIMIT 1",
                params![channel_id, subject_did],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| PolicyError::Database(e.to_string()))?;

        match json {
            Some(j) => {
                let doc: MembershipAttestation = serde_json::from_str(&j)
                    .map_err(|e| PolicyError::Serialization(e.to_string()))?;
                Ok(Some(doc))
            }
            None => Ok(None),
        }
    }

    /// Get all valid members of a channel.
    pub fn get_channel_members(
        &self,
        channel_id: &str,
    ) -> Result<Vec<MembershipAttestation>, PolicyError> {
        let db = self.db.lock();
        let mut stmt = db
            .prepare(
                "SELECT attestation_json FROM membership_attestations
                 WHERE channel_id = ?1 AND state = 'VALID'
                 ORDER BY issued_at ASC",
            )
            .map_err(|e| PolicyError::Database(e.to_string()))?;

        let members = stmt
            .query_map(params![channel_id], |row| {
                let json: String = row.get(0)?;
                Ok(json)
            })
            .map_err(|e| PolicyError::Database(e.to_string()))?
            .filter_map(|r| r.ok())
            .filter_map(|j| serde_json::from_str::<MembershipAttestation>(&j).ok())
            .collect();

        Ok(members)
    }

    /// Get expired attestations (continuous validity model, past their expires_at).
    pub fn get_expired_attestations(&self) -> Result<Vec<MembershipAttestation>, PolicyError> {
        let db = self.db.lock();
        let now = chrono::Utc::now().to_rfc3339();
        let mut stmt = db
            .prepare(
                "SELECT attestation_json FROM membership_attestations
                 WHERE state = 'VALID' AND expires_at IS NOT NULL AND expires_at < ?1",
            )
            .map_err(|e| PolicyError::Database(e.to_string()))?;

        let expired = stmt
            .query_map(params![now], |row| {
                let json: String = row.get(0)?;
                Ok(json)
            })
            .map_err(|e| PolicyError::Database(e.to_string()))?
            .filter_map(|r| r.ok())
            .filter_map(|j| serde_json::from_str::<MembershipAttestation>(&j).ok())
            .collect();

        Ok(expired)
    }

    /// Mark an attestation as invalid (expired/revoked).
    pub fn invalidate_attestation(&self, attestation_id: &str) -> Result<(), PolicyError> {
        let db = self.db.lock();
        db.execute(
            "UPDATE membership_attestations SET state = 'INVALID' WHERE attestation_id = ?1",
            params![attestation_id],
        ).map_err(|e| PolicyError::Database(e.to_string()))?;
        Ok(())
    }

    // ─── Policy Removal ────────────────────────────────────────────────

    /// Remove all policy data for a channel.
    /// Returns true if anything was removed.
    pub fn remove_channel_policy(&self, channel_id: &str) -> Result<bool, PolicyError> {
        let db = self.db.lock();
        let total: usize = ["policies", "membership_attestations", "join_receipts", "transparency_log"]
            .iter()
            .map(|table| {
                db.execute(
                    &format!("DELETE FROM {} WHERE channel_id = ?1", table),
                    params![channel_id],
                ).unwrap_or(0)
            })
            .sum();
        Ok(total > 0)
    }

    // ─── Transparency Log ────────────────────────────────────────────────

    /// Get transparency log entries for a channel.
    pub fn get_log_entries(
        &self,
        channel_id: &str,
        since: Option<i64>,
    ) -> Result<Vec<TransparencyLogEntry>, PolicyError> {
        let db = self.db.lock();
        let mut stmt = db
            .prepare(
                "SELECT entry_version, channel_id, policy_id, attestation_hash, issued_at, issuer_authority_id
                 FROM transparency_log
                 WHERE channel_id = ?1 AND id > ?2
                 ORDER BY id ASC",
            )
            .map_err(|e| PolicyError::Database(e.to_string()))?;

        let entries = stmt
            .query_map(params![channel_id, since.unwrap_or(0)], |row| {
                Ok(TransparencyLogEntry {
                    entry_version: row.get(0)?,
                    channel_id: row.get(1)?,
                    policy_id: row.get(2)?,
                    attestation_hash: row.get(3)?,
                    issued_at: row.get(4)?,
                    issuer_authority_id: row.get(5)?,
                })
            })
            .map_err(|e| PolicyError::Database(e.to_string()))?
            .filter_map(|r| r.ok())
            .collect();

        Ok(entries)
    }
    // ─── Credentials ───────────────────────────────────────────────────

    /// Store a verified credential for a user.
    /// Upserts (replaces if same did+type+issuer exists).
    pub fn store_credential(
        &self,
        subject_did: &str,
        credential_type: &str,
        issuer: &str,
        metadata: &serde_json::Value,
    ) -> Result<(), PolicyError> {
        let db = self.db.lock();
        db.execute(
            "INSERT INTO credentials (subject_did, credential_type, issuer, metadata_json, issued_at)
             VALUES (?1, ?2, ?3, ?4, datetime('now'))
             ON CONFLICT(subject_did, credential_type, issuer)
             DO UPDATE SET metadata_json = ?4, issued_at = datetime('now'), revoked = 0",
            params![
                subject_did,
                credential_type,
                issuer,
                serde_json::to_string(metadata).unwrap_or_default(),
            ],
        )
        .map_err(|e| PolicyError::Database(e.to_string()))?;
        Ok(())
    }

    /// Get all valid (non-revoked) credentials for a user.
    pub fn get_credentials(
        &self,
        subject_did: &str,
    ) -> Result<Vec<StoredCredential>, PolicyError> {
        let db = self.db.lock();
        let mut stmt = db
            .prepare(
                "SELECT credential_type, issuer, metadata_json, issued_at
                 FROM credentials
                 WHERE subject_did = ?1 AND revoked = 0",
            )
            .map_err(|e| PolicyError::Database(e.to_string()))?;

        let creds = stmt
            .query_map(params![subject_did], |row| {
                Ok(StoredCredential {
                    credential_type: row.get(0)?,
                    issuer: row.get(1)?,
                    metadata_json: row.get(2)?,
                    issued_at: row.get(3)?,
                })
            })
            .map_err(|e| PolicyError::Database(e.to_string()))?
            .filter_map(|r| r.ok())
            .collect();

        Ok(creds)
    }

    /// Revoke a credential.
    pub fn revoke_credential(
        &self,
        subject_did: &str,
        credential_type: &str,
        issuer: &str,
    ) -> Result<bool, PolicyError> {
        let db = self.db.lock();
        let n = db.execute(
            "UPDATE credentials SET revoked = 1 WHERE subject_did = ?1 AND credential_type = ?2 AND issuer = ?3",
            params![subject_did, credential_type, issuer],
        ).map_err(|e| PolicyError::Database(e.to_string()))?;
        Ok(n > 0)
    }
}

/// A stored credential from the database.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct StoredCredential {
    pub credential_type: String,
    pub issuer: String,
    pub metadata_json: String,
    pub issued_at: String,
}

#[derive(Debug, thiserror::Error)]
pub enum PolicyError {
    #[error("Database error: {0}")]
    Database(String),
    #[error("Serialization error: {0}")]
    Serialization(String),
    #[error("Validation error: {0}")]
    Validation(String),
}
