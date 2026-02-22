//! Policy engine — orchestrates the join flow, requirement evaluation,
//! and attestation issuance.
//!
//! This is the "authority server" logic that runs inside freeq-server.

use super::canonical;
use super::eval::{self, Credential, EvalResult, UserEvidence};
use super::store::{PolicyError, PolicyStore};
use super::types::*;
use chrono::Utc;
use std::collections::HashSet;

/// The policy engine — evaluates requirements and issues attestations.
pub struct PolicyEngine {
    store: PolicyStore,
    /// DID of this server (as an authority).
    authority_did: String,
}

/// Result of a join attempt.
#[derive(Debug)]
pub enum JoinResult {
    /// Join succeeded — attestation issued.
    Confirmed {
        attestation: MembershipAttestation,
        join_id: String,
    },
    /// Channel has no policy — open join (backwards compatible).
    NoPolicy,
    /// Join pending — additional requirements needed.
    Pending {
        join_id: String,
        missing: Vec<String>,
    },
    /// Join failed.
    Failed(String),
}

impl PolicyEngine {
    pub fn new(store: PolicyStore, authority_did: String) -> Self {
        PolicyEngine {
            store,
            authority_did,
        }
    }

    /// Access the underlying store.
    pub fn store(&self) -> &PolicyStore {
        &self.store
    }

    // ─── Channel Setup ───────────────────────────────────────────────────

    /// Create an initial policy and authority set for a channel.
    /// Returns (policy, authority_set).
    pub fn create_channel_policy(
        &self,
        channel_id: &str,
        requirements: Requirement,
        role_requirements: std::collections::BTreeMap<String, Requirement>,
    ) -> Result<(PolicyDocument, AuthoritySet), PolicyError> {
        // Validate requirements
        eval::validate_structure(&requirements)
            .map_err(|e| PolicyError::Validation(e))?;
        for (role, req) in &role_requirements {
            eval::validate_structure(req)
                .map_err(|e| PolicyError::Validation(format!("Role {role}: {e}")))?;
        }

        // Create authority set first (policy needs the hash)
        let auth_set = AuthoritySet {
            authority_set_hash: None,
            channel_id: channel_id.to_string(),
            signers: vec![AuthoritySigner {
                did: self.authority_did.clone(),
                public_key: String::new(), // TODO: actual key
                label: Some("Primary authority".into()),
                endpoint: None,
            }],
            policy_threshold: 1,
            authority_refresh_ttl_seconds: 3600,
            transparency: Some(TransparencyConfig {
                visibility: "public".into(),
                mmd_seconds: 86400,
            }),
            previous_authority_set_hash: None,
        };
        let auth_set = self.store.store_authority_set(auth_set)?;
        let auth_hash = auth_set.authority_set_hash.clone().unwrap();

        // Create policy
        let policy = PolicyDocument {
            channel_id: channel_id.to_string(),
            policy_id: None,
            version: 1,
            effective_at: Utc::now().to_rfc3339(),
            previous_policy_hash: None,
            authority_set_hash: auth_hash,
            requirements,
            role_requirements,
            validity_model: ValidityModel::JoinTime,
            receipt_embedding: ReceiptEmbedding::Require,
            policy_locations: vec![],
            limits: None,
            transparency: None,
        };
        let policy = self.store.store_policy(policy)?;

        Ok((policy, auth_set))
    }

    /// Update a channel's policy (creates a new version, chained to previous).
    pub fn update_channel_policy(
        &self,
        channel_id: &str,
        requirements: Requirement,
        role_requirements: std::collections::BTreeMap<String, Requirement>,
    ) -> Result<PolicyDocument, PolicyError> {
        // Validate
        eval::validate_structure(&requirements)
            .map_err(|e| PolicyError::Validation(e))?;

        let current = self.store.get_current_policy(channel_id)?
            .ok_or_else(|| PolicyError::Validation("No existing policy to update".into()))?;

        let policy = PolicyDocument {
            channel_id: channel_id.to_string(),
            policy_id: None,
            version: current.version + 1,
            effective_at: Utc::now().to_rfc3339(),
            previous_policy_hash: current.policy_id.clone(),
            authority_set_hash: current.authority_set_hash.clone(),
            requirements,
            role_requirements,
            validity_model: current.validity_model.clone(),
            receipt_embedding: current.receipt_embedding.clone(),
            policy_locations: current.policy_locations.clone(),
            limits: current.limits.clone(),
            transparency: current.transparency.clone(),
        };
        self.store.store_policy(policy)
    }

    // ─── Join Flow ───────────────────────────────────────────────────────

    /// Process a join request.
    ///
    /// For ACCEPT-only policies, the user provides `accepted_hashes` with
    /// the rules hash. For more complex requirements, additional evidence
    /// is needed.
    pub fn process_join(
        &self,
        channel_id: &str,
        subject_did: &str,
        evidence: &UserEvidence,
    ) -> Result<JoinResult, PolicyError> {
        // Get current policy
        let policy = match self.store.get_current_policy(channel_id)? {
            Some(p) => p,
            None => return Ok(JoinResult::NoPolicy),
        };

        let policy_id = policy.policy_id.clone().unwrap_or_default();

        // Check if user already has a valid attestation
        if let Some(existing) = self.store.get_attestation(channel_id, subject_did)? {
            // Check if attestation is for current policy
            if existing.policy_id == policy_id {
                // Check expiry for continuous validity
                if let Some(ref expires_at) = existing.expires_at {
                    if let Ok(exp) = chrono::DateTime::parse_from_rfc3339(expires_at) {
                        if exp > Utc::now() {
                            let jid = existing.join_id.clone().unwrap_or_default();
                            return Ok(JoinResult::Confirmed {
                                attestation: existing,
                                join_id: jid,
                            });
                        }
                        // Expired — fall through to re-evaluate
                    }
                } else {
                    // No expiry (join_time model) — still valid
                    let jid = existing.join_id.clone().unwrap_or_default();
                    return Ok(JoinResult::Confirmed {
                        attestation: existing,
                        join_id: jid,
                    });
                }
            }
            // Policy changed — need to re-evaluate
        }

        // Evaluate requirements
        let result = eval::evaluate(&policy.requirements, evidence);
        match result {
            EvalResult::Satisfied => {
                // Generate join receipt
                let join_id = generate_join_id();
                let nonce = generate_nonce();
                let now = Utc::now().to_rfc3339();

                let receipt = JoinReceipt {
                    channel_id: channel_id.to_string(),
                    policy_id: policy_id.clone(),
                    join_id: join_id.clone(),
                    subject_did: subject_did.to_string(),
                    timestamp: now.clone(),
                    nonce,
                    embedded_policy: match policy.receipt_embedding {
                        ReceiptEmbedding::Require => Some(policy.clone()),
                        _ => None,
                    },
                    signature: String::new(), // Server-side receipt doesn't need user sig for MVP
                };
                self.store.store_join_receipt(&receipt)?;

                // Determine role
                let role = self.evaluate_role(subject_did, &policy, evidence);

                // Issue attestation
                let attestation = self.issue_attestation(
                    channel_id,
                    &policy_id,
                    &policy.authority_set_hash,
                    subject_did,
                    &role,
                    Some(&join_id),
                    &policy.validity_model,
                )?;

                // Confirm join
                self.store.update_join_state(&join_id, JoinState::JoinConfirmed)?;

                Ok(JoinResult::Confirmed {
                    attestation,
                    join_id,
                })
            }
            EvalResult::Failed(reason) => {
                Ok(JoinResult::Failed(reason))
            }
            EvalResult::Error(err) => {
                Ok(JoinResult::Failed(format!("Evaluation error: {err}")))
            }
        }
    }

    /// Evaluate which role a user qualifies for.
    fn evaluate_role(
        &self,
        _subject_did: &str,
        policy: &PolicyDocument,
        evidence: &UserEvidence,
    ) -> String {
        // Check role requirements from highest to lowest priority
        // (order determined by BTreeMap key ordering)
        for (role_name, requirement) in policy.role_requirements.iter().rev() {
            if eval::evaluate(requirement, evidence).is_satisfied() {
                return role_name.clone();
            }
        }
        "member".to_string()
    }

    /// Issue a membership attestation.
    fn issue_attestation(
        &self,
        channel_id: &str,
        policy_id: &str,
        authority_set_hash: &str,
        subject_did: &str,
        role: &str,
        join_id: Option<&str>,
        validity_model: &ValidityModel,
    ) -> Result<MembershipAttestation, PolicyError> {
        let now = Utc::now();
        let expires_at = match validity_model {
            ValidityModel::Continuous => {
                Some((now + chrono::Duration::hours(1)).to_rfc3339())
            }
            ValidityModel::JoinTime => None,
        };

        let attestation = MembershipAttestation {
            attestation_id: generate_attestation_id(),
            channel_id: channel_id.to_string(),
            policy_id: policy_id.to_string(),
            authority_set_hash: authority_set_hash.to_string(),
            subject_did: subject_did.to_string(),
            role: role.to_string(),
            issued_at: now.to_rfc3339(),
            expires_at,
            join_id: join_id.map(String::from),
            signature: String::new(), // TODO: actual signing
            issuer_did: self.authority_did.clone(),
        };

        self.store.store_attestation(&attestation)?;

        Ok(attestation)
    }

    // ─── Query ───────────────────────────────────────────────────────────

    /// Check if a user has a valid attestation for a channel.
    pub fn check_membership(
        &self,
        channel_id: &str,
        subject_did: &str,
    ) -> Result<Option<MembershipAttestation>, PolicyError> {
        self.store.get_attestation(channel_id, subject_did)
    }

    /// Get the current policy for a channel.
    pub fn get_policy(&self, channel_id: &str) -> Result<Option<PolicyDocument>, PolicyError> {
        self.store.get_current_policy(channel_id)
    }
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

fn generate_join_id() -> String {
    let bytes: [u8; 16] = rand::random();
    hex::encode(bytes)
}

fn generate_nonce() -> String {
    let bytes: [u8; 16] = rand::random();
    hex::encode(bytes)
}

fn generate_attestation_id() -> String {
    let bytes: [u8; 16] = rand::random();
    hex::encode(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_engine() -> PolicyEngine {
        let store = PolicyStore::open(":memory:").unwrap();
        PolicyEngine::new(store, "did:plc:testauthority".into())
    }

    #[test]
    fn test_create_and_join_accept_only() {
        let engine = test_engine();

        // Create channel with ACCEPT-only policy
        let rules_hash = canonical::sha256_hex(b"Be nice. No spam.");
        let (policy, _auth) = engine
            .create_channel_policy(
                "#test",
                Requirement::Accept {
                    hash: rules_hash.clone(),
                },
                std::collections::BTreeMap::new(),
            )
            .unwrap();

        assert_eq!(policy.version, 1);
        assert!(policy.policy_id.is_some());

        // Try to join without accepting rules
        let mut evidence = UserEvidence {
            accepted_hashes: HashSet::new(),
            credentials: vec![],
            proofs: HashSet::new(),
        };
        let result = engine.process_join("#test", "did:plc:user1", &evidence).unwrap();
        assert!(matches!(result, JoinResult::Failed(_)));

        // Accept rules and join
        evidence.accepted_hashes.insert(rules_hash.clone());
        let result = engine.process_join("#test", "did:plc:user1", &evidence).unwrap();
        match result {
            JoinResult::Confirmed { attestation, .. } => {
                assert_eq!(attestation.subject_did, "did:plc:user1");
                assert_eq!(attestation.role, "member");
                assert_eq!(attestation.channel_id, "#test");
            }
            other => panic!("Expected Confirmed, got {:?}", other),
        }
    }

    #[test]
    fn test_no_policy_allows_join() {
        let engine = test_engine();
        let evidence = UserEvidence {
            accepted_hashes: HashSet::new(),
            credentials: vec![],
            proofs: HashSet::new(),
        };
        let result = engine.process_join("#open", "did:plc:user1", &evidence).unwrap();
        assert!(matches!(result, JoinResult::NoPolicy));
    }

    #[test]
    fn test_role_escalation() {
        let engine = test_engine();
        let rules_hash = canonical::sha256_hex(b"Project rules");

        let mut role_reqs = std::collections::BTreeMap::new();
        role_reqs.insert(
            "op".to_string(),
            Requirement::All {
                requirements: vec![
                    Requirement::Accept {
                        hash: rules_hash.clone(),
                    },
                    Requirement::Present {
                        credential_type: "github_membership".into(),
                        issuer: Some("github".into()),
                    },
                ],
            },
        );

        engine
            .create_channel_policy(
                "#project",
                Requirement::Accept {
                    hash: rules_hash.clone(),
                },
                role_reqs,
            )
            .unwrap();

        // Regular user
        let mut evidence = UserEvidence {
            accepted_hashes: HashSet::from([rules_hash.clone()]),
            credentials: vec![],
            proofs: HashSet::new(),
        };
        let result = engine.process_join("#project", "did:plc:regular", &evidence).unwrap();
        match result {
            JoinResult::Confirmed { attestation, .. } => {
                assert_eq!(attestation.role, "member");
            }
            other => panic!("Expected Confirmed, got {:?}", other),
        }

        // GitHub committer
        evidence.credentials.push(Credential {
            credential_type: "github_membership".into(),
            issuer: "github".into(),
        });
        let result = engine.process_join("#project", "did:plc:committer", &evidence).unwrap();
        match result {
            JoinResult::Confirmed { attestation, .. } => {
                assert_eq!(attestation.role, "op");
            }
            other => panic!("Expected Confirmed, got {:?}", other),
        }
    }

    #[test]
    fn test_policy_update_chains() {
        let engine = test_engine();
        let hash1 = canonical::sha256_hex(b"rules v1");

        let (p1, _) = engine
            .create_channel_policy(
                "#versioned",
                Requirement::Accept { hash: hash1.clone() },
                std::collections::BTreeMap::new(),
            )
            .unwrap();

        let hash2 = canonical::sha256_hex(b"rules v2");
        let p2 = engine
            .update_channel_policy(
                "#versioned",
                Requirement::Accept { hash: hash2.clone() },
                std::collections::BTreeMap::new(),
            )
            .unwrap();

        assert_eq!(p2.version, 2);
        assert_eq!(p2.previous_policy_hash, p1.policy_id);

        // Policy chain
        let chain = engine.store().get_policy_chain("#versioned").unwrap();
        assert_eq!(chain.len(), 2);
        assert_eq!(chain[0].version, 1);
        assert_eq!(chain[1].version, 2);
    }

    #[test]
    fn test_idempotent_join() {
        let engine = test_engine();
        let hash = canonical::sha256_hex(b"rules");

        engine
            .create_channel_policy(
                "#idem",
                Requirement::Accept { hash: hash.clone() },
                std::collections::BTreeMap::new(),
            )
            .unwrap();

        let evidence = UserEvidence {
            accepted_hashes: HashSet::from([hash]),
            credentials: vec![],
            proofs: HashSet::new(),
        };

        // Join twice — second should return existing attestation
        let r1 = engine.process_join("#idem", "did:plc:user", &evidence).unwrap();
        let r2 = engine.process_join("#idem", "did:plc:user", &evidence).unwrap();

        match (r1, r2) {
            (JoinResult::Confirmed { attestation: a1, .. }, JoinResult::Confirmed { attestation: a2, .. }) => {
                assert_eq!(a1.attestation_id, a2.attestation_id);
            }
            _ => panic!("Expected both to be Confirmed"),
        }
    }

    #[test]
    fn test_transparency_log() {
        let engine = test_engine();
        let hash = canonical::sha256_hex(b"rules");

        engine
            .create_channel_policy(
                "#logged",
                Requirement::Accept { hash: hash.clone() },
                std::collections::BTreeMap::new(),
            )
            .unwrap();

        let evidence = UserEvidence {
            accepted_hashes: HashSet::from([hash]),
            credentials: vec![],
            proofs: HashSet::new(),
        };

        engine.process_join("#logged", "did:plc:user1", &evidence).unwrap();
        engine.process_join("#logged", "did:plc:user2", &evidence).unwrap();

        let entries = engine.store().get_log_entries("#logged", None).unwrap();
        assert_eq!(entries.len(), 2);
        // Entries don't contain user DIDs (privacy)
        assert!(entries.iter().all(|e| !e.attestation_hash.is_empty()));
    }
}
