//! HTTP API endpoints for the Policy & Authority Framework.
//!
//! These endpoints enable:
//! - Policy discovery (clients fetch channel policies)
//! - Join flow (clients submit evidence, receive attestations)
//! - Transparency log queries

use super::engine::PolicyEngine;
use super::eval::{Credential, UserEvidence};
use super::types::*;
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::Arc;

/// Build the policy API router.
pub fn router(engine: Arc<PolicyEngine>) -> Router {
    Router::new()
        .route("/api/v1/policy/:channel", get(get_policy))
        .route("/api/v1/policy/:channel/history", get(get_policy_chain))
        .route("/api/v1/policy/:channel/join", post(join_channel))
        .route(
            "/api/v1/policy/:channel/membership/:did",
            get(check_membership),
        )
        .route(
            "/api/v1/policy/:channel/transparency",
            get(get_transparency_log),
        )
        .route("/api/v1/authority/:hash", get(get_authority_set))
        .with_state(engine)
}

// ─── Request/Response Types ──────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct JoinRequest {
    /// DID of the joining user.
    subject_did: String,
    /// Hashes of accepted rules documents.
    #[serde(default)]
    accepted_hashes: Vec<String>,
    /// Credentials presented.
    #[serde(default)]
    credentials: Vec<CredentialInput>,
    /// Proofs provided.
    #[serde(default)]
    proofs: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct CredentialInput {
    credential_type: String,
    issuer: String,
}

#[derive(Debug, Serialize)]
struct JoinResponse {
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    join_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    attestation: Option<MembershipAttestation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    missing: Option<Vec<String>>,
}

#[derive(Debug, Serialize)]
struct PolicyResponse {
    policy: PolicyDocument,
    #[serde(skip_serializing_if = "Option::is_none")]
    authority_set: Option<AuthoritySet>,
}

#[derive(Debug, Deserialize)]
struct LogQuery {
    since: Option<i64>,
}

// ─── Handlers ────────────────────────────────────────────────────────────────

/// GET /api/v1/policy/:channel — Get current policy for a channel.
async fn get_policy(
    State(engine): State<Arc<PolicyEngine>>,
    Path(channel): Path<String>,
) -> impl IntoResponse {
    let channel_id = normalize_channel(&channel);

    match engine.get_policy(&channel_id) {
        Ok(Some(policy)) => {
            // Also fetch the authority set
            let auth_set = engine
                .store()
                .get_authority_set(&policy.authority_set_hash)
                .ok()
                .flatten();

            Json(PolicyResponse {
                policy,
                authority_set: auth_set,
            })
            .into_response()
        }
        Ok(None) => (StatusCode::NOT_FOUND, "No policy for this channel").into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// GET /api/v1/policy/:channel/history — Get full policy chain.
async fn get_policy_chain(
    State(engine): State<Arc<PolicyEngine>>,
    Path(channel): Path<String>,
) -> impl IntoResponse {
    let channel_id = normalize_channel(&channel);

    match engine.store().get_policy_chain(&channel_id) {
        Ok(chain) => Json(chain).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// POST /api/v1/policy/:channel/join — Submit evidence to join a channel.
async fn join_channel(
    State(engine): State<Arc<PolicyEngine>>,
    Path(channel): Path<String>,
    Json(req): Json<JoinRequest>,
) -> impl IntoResponse {
    let channel_id = normalize_channel(&channel);

    let evidence = UserEvidence {
        accepted_hashes: req.accepted_hashes.into_iter().collect::<HashSet<_>>(),
        credentials: req
            .credentials
            .into_iter()
            .map(|c| Credential {
                credential_type: c.credential_type,
                issuer: c.issuer,
            })
            .collect(),
        proofs: req.proofs.into_iter().collect::<HashSet<_>>(),
    };

    match engine.process_join(&channel_id, &req.subject_did, &evidence) {
        Ok(result) => match result {
            super::JoinResult::Confirmed {
                attestation,
                join_id,
            } => Json(JoinResponse {
                status: "confirmed".into(),
                join_id: Some(join_id),
                attestation: Some(attestation),
                error: None,
                missing: None,
            })
            .into_response(),

            super::JoinResult::NoPolicy => Json(JoinResponse {
                status: "open".into(),
                join_id: None,
                attestation: None,
                error: None,
                missing: None,
            })
            .into_response(),

            super::JoinResult::Pending { join_id, missing } => (
                StatusCode::ACCEPTED,
                Json(JoinResponse {
                    status: "pending".into(),
                    join_id: Some(join_id),
                    attestation: None,
                    error: None,
                    missing: Some(missing),
                }),
            )
                .into_response(),

            super::JoinResult::Failed(reason) => (
                StatusCode::FORBIDDEN,
                Json(JoinResponse {
                    status: "failed".into(),
                    join_id: None,
                    attestation: None,
                    error: Some(reason),
                    missing: None,
                }),
            )
                .into_response(),
        },
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// GET /api/v1/policy/:channel/membership/:did — Check membership.
async fn check_membership(
    State(engine): State<Arc<PolicyEngine>>,
    Path((channel, did)): Path<(String, String)>,
) -> impl IntoResponse {
    let channel_id = normalize_channel(&channel);

    match engine.check_membership(&channel_id, &did) {
        Ok(Some(attestation)) => Json(attestation).into_response(),
        Ok(None) => (StatusCode::NOT_FOUND, "No valid membership").into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// GET /api/v1/policy/:channel/transparency — Get transparency log.
async fn get_transparency_log(
    State(engine): State<Arc<PolicyEngine>>,
    Path(channel): Path<String>,
    Query(query): Query<LogQuery>,
) -> impl IntoResponse {
    let channel_id = normalize_channel(&channel);

    match engine.store().get_log_entries(&channel_id, query.since) {
        Ok(entries) => Json(entries).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// GET /api/v1/authority/:hash — Get authority set by hash.
async fn get_authority_set(
    State(engine): State<Arc<PolicyEngine>>,
    Path(hash): Path<String>,
) -> impl IntoResponse {
    match engine.store().get_authority_set(&hash) {
        Ok(Some(auth_set)) => Json(auth_set).into_response(),
        Ok(None) => (StatusCode::NOT_FOUND, "Authority set not found").into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// Normalize channel name (ensure # prefix, lowercase).
fn normalize_channel(channel: &str) -> String {
    let ch = if channel.starts_with('#') {
        channel.to_string()
    } else {
        format!("#{channel}")
    };
    ch.to_lowercase()
}
