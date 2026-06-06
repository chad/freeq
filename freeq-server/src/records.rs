//! AT Protocol records for forkable personas (and characters).
//!
//! These mirror the lexicons in `/lexicons/at.freeq.*.json`. The point:
//! a persona is a public, signed record in its author's PDS repo,
//! addressed by an `at://` URI. Lineage is intrinsic — each record pins
//! the `forkedFrom` URI of its parent — so fork counts and ancestry are
//! verifiable without trusting any one server. The server's job shrinks
//! to *aggregating* the fork graph (see [`crate::db`] fork methods);
//! anyone can rebuild it from the firehose.
//!
//! Live PDS writes (OAuth) and a firehose indexer are out of scope here;
//! this module provides the record types, AT-URI handling, and the
//! fork-edge derivation that an ingest endpoint (or a future indexer)
//! uses to fold a record into the graph.

use serde::{Deserialize, Serialize};

/// Collection / NSID for persona records.
pub const PERSONA_NSID: &str = "at.freeq.persona";
/// Collection / NSID for character records.
pub const CHARACTER_NSID: &str = "at.freeq.character";

fn persona_type() -> String {
    PERSONA_NSID.to_string()
}

/// `at.freeq.persona#voice` — the base TTS voice.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PersonaVoice {
    pub provider: String,
    pub voice_id: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub speed_milli: Option<i64>,
}

/// `at.freeq.persona#face` — the ghostly character the persona wears.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct PersonaFace {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub character: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub pack: Option<String>,
}

/// An `at.freeq.persona` record. Unknown fields are ignored, so a record
/// straight from a PDS (carrying `$type`, etc.) deserializes cleanly.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PersonaRecord {
    #[serde(rename = "$type", default = "persona_type")]
    pub record_type: String,
    pub name: String,
    pub system_prompt: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub greeting: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub voice: Option<PersonaVoice>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub face: Option<PersonaFace>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub forked_from: Option<String>,
    pub created_at: String,
}

/// A parsed `at://authority/collection/rkey` URI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AtUri {
    /// The repo authority — a DID (or handle).
    pub authority: String,
    pub collection: String,
    pub rkey: String,
}

impl AtUri {
    /// Parse `at://did/collection/rkey`. `None` if it isn't a
    /// fully-qualified record URI.
    pub fn parse(uri: &str) -> Option<Self> {
        let rest = uri.strip_prefix("at://")?;
        let mut parts = rest.splitn(3, '/');
        let authority = parts.next()?.to_string();
        let collection = parts.next()?.to_string();
        let rkey = parts.next()?.to_string();
        if authority.is_empty() || collection.is_empty() || rkey.is_empty() {
            return None;
        }
        Some(Self { authority, collection, rkey })
    }
}

impl std::fmt::Display for AtUri {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "at://{}/{}/{}", self.authority, self.collection, self.rkey)
    }
}

/// The fork-graph edge a persona record implies, given its own at:// URI.
/// Returns `(parent_uri, child_uri, forked_by_did)` when the record is a
/// fork (`forkedFrom` set), else `None`. `forked_by` is the child URI's
/// authority — the DID that signed the fork, not something the submitter
/// can spoof.
pub fn persona_fork_edge(
    record_uri: &str,
    rec: &PersonaRecord,
) -> Option<(String, String, Option<String>)> {
    let parent = rec.forked_from.clone()?;
    let forked_by = AtUri::parse(record_uri).map(|u| u.authority);
    Some((parent, record_uri.to_string(), forked_by))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn at_uri_round_trips() {
        let uri = "at://did:plc:abc123/at.freeq.persona/3kxyz";
        let parsed = AtUri::parse(uri).unwrap();
        assert_eq!(parsed.authority, "did:plc:abc123");
        assert_eq!(parsed.collection, "at.freeq.persona");
        assert_eq!(parsed.rkey, "3kxyz");
        assert_eq!(parsed.to_string(), uri);
        // Malformed URIs are rejected.
        assert!(AtUri::parse("https://example.com/x").is_none());
        assert!(AtUri::parse("at://did:plc:abc/onlytwo").is_none());
    }

    #[test]
    fn persona_record_serde_is_lexicon_shaped() {
        // A record as it would appear in a PDS (camelCase + $type).
        let json = r#"{
            "$type": "at.freeq.persona",
            "name": "Cassandra",
            "systemPrompt": "You foresee and you warn.",
            "greeting": "You won't listen.",
            "voice": { "provider": "elevenlabs", "voiceId": "abc", "speedMilli": 1180 },
            "face": { "character": "oblivion" },
            "forkedFrom": "at://did:plc:orig/at.freeq.persona/parent1",
            "createdAt": "2026-06-06T00:00:00Z"
        }"#;
        let rec: PersonaRecord = serde_json::from_str(json).unwrap();
        assert_eq!(rec.name, "Cassandra");
        assert_eq!(rec.voice.as_ref().unwrap().speed_milli, Some(1180));
        assert_eq!(rec.face.as_ref().unwrap().character.as_deref(), Some("oblivion"));
        // Re-serializes back to camelCase keys.
        let out = serde_json::to_string(&rec).unwrap();
        assert!(out.contains("\"systemPrompt\""));
        assert!(out.contains("\"forkedFrom\""));
    }

    #[test]
    fn fork_edge_derives_from_forked_from() {
        let mut rec = PersonaRecord {
            record_type: persona_type(),
            name: "c".into(),
            system_prompt: "p".into(),
            greeting: None,
            voice: None,
            face: None,
            forked_from: Some("at://did:plc:orig/at.freeq.persona/parent1".into()),
            created_at: "2026-06-06T00:00:00Z".into(),
        };
        let child = "at://did:plc:me/at.freeq.persona/child1";
        let (parent, c, by) = persona_fork_edge(child, &rec).unwrap();
        assert_eq!(parent, "at://did:plc:orig/at.freeq.persona/parent1");
        assert_eq!(c, child);
        assert_eq!(by.as_deref(), Some("did:plc:me")); // the signer, from the child URI

        // An original (no forkedFrom) implies no edge.
        rec.forked_from = None;
        assert!(persona_fork_edge(child, &rec).is_none());
    }
}
