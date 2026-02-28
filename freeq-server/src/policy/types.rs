//! Core types for the Freeq Policy & Authority Framework.
//!
//! All objects are designed to be:
//! - Serializable via serde_json
//! - Canonicalized via JCS (RFC 8785)
//! - Hashed via SHA-256
//! - Immutable once created (updates create new versions)

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

// ─── Policy Document ─────────────────────────────────────────────────────────

/// Immutable, versioned channel rules document.
/// Updates create new versions chained via `previous_policy_hash`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PolicyDocument {
    /// Channel this policy applies to.
    pub channel_id: String,

    /// SHA-256 of the JCS-canonicalized document (computed, not stored in signed payload).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub policy_id: Option<String>,

    /// Monotonically increasing version number.
    pub version: i64,

    /// When this policy becomes effective (RFC 3339 timestamp).
    pub effective_at: String,

    /// Hash of the previous policy version (None for version 1).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_policy_hash: Option<String>,

    /// Hash of the authority set that governs this policy.
    pub authority_set_hash: String,

    /// Requirements for joining the channel.
    pub requirements: Requirement,

    /// Requirements for role escalation (role_name → requirement).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub role_requirements: BTreeMap<String, Requirement>,

    /// "join_time" (evaluate once) or "continuous" (re-check with expiry).
    #[serde(default = "default_validity_model")]
    pub validity_model: ValidityModel,

    /// Whether join receipts must embed the full policy.
    #[serde(default = "default_receipt_embedding")]
    pub receipt_embedding: ReceiptEmbedding,

    /// URLs where this policy can be fetched.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub policy_locations: Vec<String>,

    /// Channel limits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limits: Option<ChannelLimits>,

    /// Transparency configuration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transparency: Option<TransparencyConfig>,

    /// Credential endpoints — tells clients where to obtain each credential type.
    /// Keyed by credential_type (e.g. "github_membership").
    /// This is UX metadata, not part of the security model.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub credential_endpoints: BTreeMap<String, CredentialEndpoint>,
}

/// Metadata about where/how to obtain a specific credential type.
/// Stored in the policy document so clients can build guided join flows.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CredentialEndpoint {
    /// DID of the credential issuer.
    pub issuer: String,
    /// URL to start the verification flow.
    /// Client appends `?subject_did=...&callback=...` query params.
    pub url: String,
    /// Human-readable label for the button (e.g. "Verify with GitHub").
    pub label: String,
    /// Optional description shown to users.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ValidityModel {
    JoinTime,
    Continuous,
}

fn default_validity_model() -> ValidityModel {
    ValidityModel::JoinTime
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ReceiptEmbedding {
    Require,
    Allow,
    Forbid,
}

fn default_receipt_embedding() -> ReceiptEmbedding {
    ReceiptEmbedding::Require
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChannelLimits {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_members: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_bots: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TransparencyConfig {
    /// "public", "members_only", "authority_only"
    pub visibility: String,
    /// Maximum merge delay in seconds.
    #[serde(default = "default_mmd")]
    pub mmd_seconds: i64,
}

fn default_mmd() -> i64 {
    86400 // 24 hours
}

// ─── Requirement DSL ─────────────────────────────────────────────────────────

/// The requirement evaluation language.
/// Max depth: 8, max nodes: 64. Deterministic, fail-closed.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Requirement {
    /// User must accept a rules document (identified by hash).
    Accept {
        hash: String,
    },

    /// User must present a credential of the given type.
    Present {
        credential_type: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        issuer: Option<String>,
    },

    /// User must prove a capability.
    Prove {
        proof_type: String,
    },

    /// All sub-requirements must be satisfied.
    All {
        requirements: Vec<Requirement>,
    },

    /// At least one sub-requirement must be satisfied.
    Any {
        requirements: Vec<Requirement>,
    },

    /// The sub-requirement must NOT be satisfied.
    Not {
        requirement: Box<Requirement>,
    },
}

// ─── Authority Set ───────────────────────────────────────────────────────────

/// Authority configuration — separate from policy to allow key rotation
/// without requiring policy re-acceptance.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AuthoritySet {
    /// SHA-256 of the JCS-canonicalized object.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub authority_set_hash: Option<String>,

    /// Channel this authority set governs.
    pub channel_id: String,

    /// Signing authorities.
    pub signers: Vec<AuthoritySigner>,

    /// Number of signers required for policy updates.
    pub policy_threshold: i32,

    /// How often federated nodes should refresh (seconds).
    #[serde(default = "default_authority_ttl")]
    pub authority_refresh_ttl_seconds: i64,

    /// Transparency configuration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transparency: Option<TransparencyConfig>,

    /// Previous authority set hash (for chaining).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_authority_set_hash: Option<String>,
}

fn default_authority_ttl() -> i64 {
    3600 // 1 hour
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AuthoritySigner {
    /// DID of the signing authority.
    pub did: String,
    /// Public key (multibase or JWK).
    pub public_key: String,
    /// Human-readable label.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Endpoint for this authority.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
}

// ─── Join Receipt ────────────────────────────────────────────────────────────

/// Signed by the user at join time — proof they accepted the policy.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct JoinReceipt {
    /// Channel being joined.
    pub channel_id: String,
    /// Policy being accepted.
    pub policy_id: String,
    /// Unique join attempt ID (128-bit random, hex-encoded).
    pub join_id: String,
    /// DID of the joining user.
    pub subject_did: String,
    /// RFC 3339 timestamp.
    pub timestamp: String,
    /// Cryptographic nonce (hex-encoded).
    pub nonce: String,
    /// Full policy document (if receipt_embedding = "require").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub embedded_policy: Option<PolicyDocument>,
    /// User's signature over the JCS-canonicalized receipt (without this field).
    pub signature: String,
}

/// Join flow states.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum JoinState {
    JoinPending,
    JoinConfirmed,
    JoinFailed,
    JoinStale,
}

// ─── Membership Attestation ──────────────────────────────────────────────────

/// Issued by an authority server — proof of channel membership and role.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MembershipAttestation {
    /// Unique attestation ID.
    pub attestation_id: String,
    /// Channel.
    pub channel_id: String,
    /// Policy version this attestation was issued under.
    pub policy_id: String,
    /// Authority set that was active.
    pub authority_set_hash: String,
    /// DID of the member.
    pub subject_did: String,
    /// Assigned role.
    pub role: String,
    /// When issued (RFC 3339).
    pub issued_at: String,
    /// Expiry (for continuous validity model).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
    /// Linked join receipt ID.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub join_id: Option<String>,
    /// Authority's signature.
    pub signature: String,
    /// DID of the issuing authority.
    pub issuer_did: String,
}

/// Attestation validity state (based on transparency log inclusion).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum AttestationState {
    Valid,
    Suspended,
    Invalid,
}

// ─── Transparency Log ────────────────────────────────────────────────────────

/// A single entry in the transparency log.
/// Does NOT contain user DID (privacy-preserving).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TransparencyLogEntry {
    pub entry_version: i32,
    pub channel_id: String,
    pub policy_id: String,
    /// SHA-256 of the attestation.
    pub attestation_hash: String,
    pub issued_at: String,
    pub issuer_authority_id: String,
}

/// Signed Tree Head — published periodically by authorities.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SignedTreeHead {
    pub log_id: String,
    pub tree_size: i64,
    pub root_hash: String,
    pub timestamp: String,
    pub authority_id: String,
    pub signature: String,
}

// ─── Authority Revocation ────────────────────────────────────────────────────

/// Published when an authority key is compromised.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AuthorityRevocation {
    pub channel_id: String,
    /// DIDs of compromised signers.
    pub compromised_signers: Vec<String>,
    /// New authority set hash to transition to.
    pub new_authority_set_hash: String,
    /// Signatures meeting policy_threshold.
    pub signatures: Vec<RevocationSignature>,
    pub timestamp: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RevocationSignature {
    pub signer_did: String,
    pub signature: String,
}

// ─── Verifiable Credential ────────────────────────────────────────────────────

/// A portable, signed credential issued by an external verifier.
///
/// The issuer is identified by DID. The signature is Ed25519 over the
/// JCS-canonical form (with signature field empty). Anyone can verify
/// by resolving the issuer's DID document and extracting the public key.
///
/// This decouples credential verification from the freeq server:
/// - A standalone service does GitHub OAuth, email verification, etc.
/// - It issues a VerifiableCredential signed with its key
/// - The user presents it to any freeq server
/// - The server verifies the signature, never talks to GitHub
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VerifiableCredential {
    /// Always "FreeqCredential/v1".
    #[serde(rename = "type")]
    pub credential_type_tag: String,

    /// DID of the issuer (e.g. "did:web:verify.freeq.at").
    pub issuer: String,

    /// DID of the credential subject (the user).
    pub subject: String,

    /// Credential type (e.g. "github_membership").
    pub credential_type: String,

    /// Credential claims (issuer-defined).
    pub claims: serde_json::Value,

    /// When issued (RFC 3339).
    pub issued_at: String,

    /// When it expires (RFC 3339). None = no expiry.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,

    /// Ed25519 signature over the JCS-canonical form (base64url, unpadded).
    /// Computed with this field set to empty string.
    pub signature: String,
}

impl VerifiableCredential {
    /// Check if the credential has expired.
    pub fn is_expired(&self) -> bool {
        if let Some(ref exp) = self.expires_at
            && let Ok(dt) = chrono::DateTime::parse_from_rfc3339(exp) {
                return dt < chrono::Utc::now();
            }
        false
    }
}

// ─── Role & Permission ───────────────────────────────────────────────────────

/// Protocol-native permissions.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Permission {
    Post,
    Delete,
    Invite,
    Moderate,
    AddBot,
    ConfigureChannel,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RoleDefinition {
    pub name: String,
    pub permissions: Vec<Permission>,
}
