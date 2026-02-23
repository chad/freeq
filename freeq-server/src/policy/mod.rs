//! Freeq Policy & Authority Framework
//!
//! Cryptographically versioned channel governance with auditable membership.
//!
//! # Architecture
//!
//! - `types` — Core data structures (PolicyDocument, AuthoritySet, etc.)
//! - `canonical` — JCS (RFC 8785) canonicalization and SHA-256 hashing
//! - `eval` — Requirement DSL evaluator
//! - `store` — SQLite storage for all policy objects
//! - `engine` — Join flow orchestration and attestation issuance
//! - `api` — HTTP API endpoints for policy discovery and join flow

pub mod api;
pub mod canonical;
pub mod credentials;
pub mod engine;
pub mod eval;
pub mod store;
pub mod types;

pub use engine::{JoinResult, PolicyEngine};
pub use store::{PolicyError, PolicyStore};
// Re-export key types
pub use types::{Requirement, PolicyDocument, AuthoritySet, MembershipAttestation, VerifiableCredential};
