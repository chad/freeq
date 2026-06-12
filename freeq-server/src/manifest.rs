//! Agent manifests — declarative agent configuration.
//!
//! An agent manifest describes an agent's identity, provenance, and default
//! capabilities. Manifests can be submitted inline (TOML/JSON) or fetched
//! from a URL.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentManifest {
    pub agent: AgentInfo,
    pub provenance: ManifestProvenance,
    #[serde(default)]
    pub capabilities: ManifestCapabilities,
    #[serde(default)]
    pub presence: ManifestPresence,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentInfo {
    pub display_name: String,
    #[serde(default = "default_actor_class")]
    pub actor_class: String,
    pub description: Option<String>,
    pub source_repo: Option<String>,
    pub image_digest: Option<String>,
    pub version: Option<String>,
    pub documentation_url: Option<String>,
}

fn default_actor_class() -> String {
    "agent".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestProvenance {
    pub origin_type: String,
    pub creator_did: String,
    pub revocation_authority: String,
    pub authority_basis: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ManifestCapabilities {
    #[serde(default)]
    pub default: Vec<String>,
    #[serde(default)]
    pub channels: BTreeMap<String, Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestPresence {
    #[serde(default = "default_heartbeat")]
    pub heartbeat_interval_seconds: u64,
}

fn default_heartbeat() -> u64 {
    30
}

impl Default for ManifestPresence {
    fn default() -> Self {
        Self {
            heartbeat_interval_seconds: 30,
        }
    }
}

impl AgentManifest {
    /// Parse from TOML string.
    pub fn from_toml(s: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(s)
    }

    /// Parse from JSON string.
    pub fn from_json(s: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(s)
    }

    /// Serialize to JSON.
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    /// Validate the manifest for basic correctness.
    pub fn validate(&self) -> Result<(), String> {
        if self.agent.display_name.is_empty() {
            return Err("display_name is required".into());
        }
        if self.provenance.creator_did.is_empty() {
            return Err("creator_did is required".into());
        }
        if self.provenance.revocation_authority.is_empty() {
            return Err("revocation_authority is required".into());
        }
        Ok(())
    }
}

/// Wrapper trust profile for external agent bridges.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WrapperRecord {
    pub wrapper_did: String,
    pub wrapper_name: String,
    pub description: Option<String>,
    pub source_repo: Option<String>,
    pub image_digest: Option<String>,
    pub audit_status: WrapperAuditStatus,
    pub supported_protocols: Vec<String>,
    pub registered_by: String,
    pub registered_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum WrapperAuditStatus {
    Unaudited,
    CommunityReviewed,
    FormallyAudited,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_toml_manifest() {
        let toml = r##"
[agent]
display_name = "test-bot"
description = "A test agent"
source_repo = "https://github.com/example/bot"
version = "0.1.0"

[provenance]
origin_type = "template"
creator_did = "did:plc:abc123"
revocation_authority = "did:plc:abc123"
authority_basis = "Test"

[capabilities]
default = ["post_message", "read_channel"]

[capabilities.channels]
"#factory" = ["post_message", "call_tool"]

[presence]
heartbeat_interval_seconds = 15
"##;
        let manifest = AgentManifest::from_toml(toml).unwrap();
        assert_eq!(manifest.agent.display_name, "test-bot");
        assert_eq!(manifest.capabilities.default.len(), 2);
        assert_eq!(
            manifest
                .capabilities
                .channels
                .get("#factory")
                .unwrap()
                .len(),
            2
        );
        assert_eq!(manifest.presence.heartbeat_interval_seconds, 15);
        assert!(manifest.validate().is_ok());
    }

    #[test]
    fn validate_empty_name() {
        let toml = r#"
[agent]
display_name = ""
[provenance]
origin_type = "custom"
creator_did = "did:plc:abc"
revocation_authority = "did:plc:abc"
"#;
        let manifest = AgentManifest::from_toml(toml).unwrap();
        assert!(manifest.validate().is_err());
    }
}
