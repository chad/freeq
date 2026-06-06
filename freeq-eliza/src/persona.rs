//! Persona pack — the forkable, on-disk definition of an Eliza-class
//! agent's *brain*: who it is (system prompt), how it sounds (the
//! ElevenLabs TTS voice), what it says on arrival (greeting), and which
//! ghostly *character* (face + voice DSP) it wears.
//!
//! ## Decoupling
//!
//! The visual + audio *character* lives entirely in the `ghostly` crate
//! and is referenced here only by **name** (`ghostly_character`). freeq
//! never reaches into ghostly's character internals — it hands ghostly
//! a name and gets back a face ([`ghostly::characters::by_name`]) and a
//! voice-DSP profile ([`ghostly::audio::profile::for_character`]). So a
//! persona is the *brain* (this crate) wearing a *body* (ghostly),
//! linked by a single string. That's the clean seam between the two
//! projects.
//!
//! A built-in persona ([`PersonaPack::builtin`]) is just a
//! [`crate::character_profile`] constant lifted into owned data; a
//! custom persona is loaded from JSON with [`PersonaPack::from_file`].

use serde::{Deserialize, Serialize};

/// A complete agent persona — loadable from disk, forkable, and
/// independent of how the character it wears is rendered.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PersonaPack {
    /// The agent's own name/identity (what it calls itself and is
    /// addressed as). Independent of the character it wears.
    pub name: String,
    /// Which built-in ghostly character (face + voice DSP) this persona
    /// wears, by name (e.g. `"oblivion"`). Resolved by the `ghostly`
    /// crate. Defaults to [`name`](Self::name) when absent — i.e. a
    /// persona named after a built-in character wears that character.
    /// Ignored when [`ghostly_pack`](Self::ghostly_pack) is set.
    #[serde(default)]
    pub ghostly_character: Option<String>,
    /// Path to a custom ghostly `CharacterPack` JSON (a forkable face +
    /// voice-DSP definition). When set, the persona wears this fully
    /// custom character instead of a built-in archetype — this is what
    /// lets a forked persona carry its own visuals + audio end-to-end.
    #[serde(default)]
    pub ghostly_pack: Option<String>,
    /// ElevenLabs voice ID for TTS. This is the *base* voice; ghostly's
    /// per-character DSP chain colours it further.
    pub voice_id: String,
    /// TTS speed multiplier (`>1.0` = faster). Carried for a future
    /// tuning pass; not yet surfaced to the ElevenLabs call.
    #[serde(default = "default_speed")]
    pub speed_multiplier: f32,
    /// System prompt — the personality. Used verbatim (the author owns
    /// the self-identity wording).
    pub system_prompt: String,
    /// Resting emotion bias for the idle/ambient path.
    #[serde(default)]
    pub default_emotion: Option<String>,
    /// One-liner spoken aloud on joining a call. `None` = stay silent
    /// on arrival.
    #[serde(default)]
    pub hello_line: Option<String>,
    /// Lineage: the persona this was forked from — a name today, an
    /// `at://` URI once personas are AT-Protocol records. Carried so a
    /// fork graph can be reconstructed; freeq-eliza only records it.
    #[serde(default)]
    pub forked_from: Option<String>,
    /// Creator identity (DID/handle) — attribution that survives forks.
    #[serde(default)]
    pub author: Option<String>,
}

fn default_speed() -> f32 {
    1.0
}

impl PersonaPack {
    /// The ghostly character (face + voice-DSP archetype) this persona
    /// wears. Falls back to the persona's own name.
    pub fn character(&self) -> &str {
        self.ghostly_character.as_deref().unwrap_or(&self.name)
    }

    /// Lift a built-in [`crate::character_profile`] (Oblivion, Narrator,
    /// Utopia) into an owned persona. `None` for names without a
    /// built-in profile (e.g. plain `"eliza"`).
    pub fn builtin(name: &str) -> Option<Self> {
        let p = crate::character_profile::by_name(name)?;
        Some(Self {
            name: name.to_string(),
            ghostly_character: Some(name.to_string()),
            ghostly_pack: None,
            voice_id: p.voice_id.to_string(),
            speed_multiplier: p.speed_multiplier,
            system_prompt: p.system_prompt.to_string(),
            default_emotion: Some(p.default_emotion.to_string()),
            hello_line: Some(p.hello_line.to_string()),
            forked_from: None,
            author: None,
        })
    }

    /// Parse a persona from a JSON string.
    pub fn from_json_str(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }

    /// Serialize to pretty JSON (for `export`/forking).
    pub fn to_json_string(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }

    /// Load a persona from a JSON file on disk.
    pub fn from_file(path: impl AsRef<std::path::Path>) -> std::io::Result<Self> {
        let s = std::fs::read_to_string(path)?;
        Self::from_json_str(&s)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    }
}

/// Resolve the ghostly *face* an agent wears: a custom `CharacterPack`
/// file (`ghostly_pack`) when given and loadable, otherwise the built-in
/// character by name. `None` only when neither resolves.
///
/// This (and [`resolve_voice_profile`]) is the single place that knows
/// "custom pack or built-in archetype" — the only seam where freeq
/// reaches into ghostly to materialize a character.
pub fn resolve_character(
    ghostly_character: &str,
    ghostly_pack: Option<&str>,
) -> Option<ghostly::Character> {
    if let Some(path) = ghostly_pack {
        match ghostly::CharacterPack::from_file(path) {
            Ok(pack) => match pack.to_character() {
                Some(c) => return Some(c),
                None => tracing::warn!(
                    pack = %path,
                    base = %pack.base,
                    "persona: ghostly pack has unknown base archetype; falling back to built-in"
                ),
            },
            Err(e) => tracing::warn!(
                pack = %path,
                error = %e,
                "persona: failed to load ghostly pack; falling back to built-in"
            ),
        }
    }
    ghostly::characters::by_name(ghostly_character)
}

/// Resolve the ghostly *voice-DSP* profile an agent uses: from a custom
/// `CharacterPack` file when given, otherwise the built-in character's
/// default profile.
pub fn resolve_voice_profile(
    ghostly_character: &str,
    ghostly_pack: Option<&str>,
) -> ghostly::audio::profile::VoiceProfile {
    if let Some(path) = ghostly_pack {
        match ghostly::CharacterPack::from_file(path) {
            Ok(pack) => return pack.voice_profile(),
            Err(e) => tracing::warn!(
                pack = %path,
                error = %e,
                "persona: failed to load ghostly pack voice; falling back to built-in"
            ),
        }
    }
    ghostly::audio::profile::for_character(ghostly_character)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtins_lift_to_personas() {
        for name in ["oblivion", "narrator", "utopia"] {
            let p = PersonaPack::builtin(name).expect("built-in persona");
            assert_eq!(p.name, name);
            assert_eq!(p.character(), name);
            assert!(!p.voice_id.is_empty());
            assert!(p.hello_line.is_some());
            // Round-trips through JSON.
            let json = p.to_json_string().unwrap();
            let back = PersonaPack::from_json_str(&json).unwrap();
            assert_eq!(back.system_prompt, p.system_prompt);
        }
        // Plain eliza has no built-in profile.
        assert!(PersonaPack::builtin("eliza").is_none());
    }

    #[test]
    fn custom_persona_can_wear_any_character() {
        // A brand-new agent that wears Oblivion's face/voice but is its
        // own identity with its own brain and lineage.
        let json = r#"{
            "name": "cassandra",
            "ghostly_character": "oblivion",
            "voice_id": "abc123",
            "system_prompt": "You are Cassandra. You foresee and you warn.",
            "hello_line": "Cassandra. You won't listen, but I'll speak anyway.",
            "forked_from": "oblivion",
            "author": "did:plc:example"
        }"#;
        let p = PersonaPack::from_json_str(json).unwrap();
        assert_eq!(p.name, "cassandra");
        assert_eq!(p.character(), "oblivion"); // wears Oblivion's body
        assert_eq!(p.forked_from.as_deref(), Some("oblivion"));
        assert_eq!(p.speed_multiplier, 1.0); // defaulted
    }

    #[test]
    fn character_defaults_to_name() {
        let json = r#"{ "name": "eliza", "voice_id": "v", "system_prompt": "hi" }"#;
        let p = PersonaPack::from_json_str(json).unwrap();
        assert_eq!(p.character(), "eliza");
    }

    #[test]
    fn resolves_face_and_voice_from_a_ghostly_pack() {
        // The cross-repo loop end to end: a custom ghostly CharacterPack
        // file (Oblivion's body, renamed) drives both the face and the
        // voice DSP a persona uses.
        let mut pack = ghostly::CharacterPack::from_character("oblivion").unwrap();
        pack.name = "azure-oblivion".to_string();
        let path = std::env::temp_dir()
            .join(format!("freeq-persona-pack-{}.json", std::process::id()));
        std::fs::write(&path, pack.to_json_string().unwrap()).unwrap();
        let p = path.to_str().unwrap();

        // Face resolves from the pack — note the custom name is carried.
        let c = resolve_character("eliza", Some(p)).expect("pack character");
        assert_eq!(c.name, "azure-oblivion");

        // Voice resolves from the pack — it inherited Oblivion's profile.
        let v = resolve_voice_profile("eliza", Some(p));
        assert_eq!(
            v.formant_shift,
            ghostly::audio::profile::for_character("oblivion").formant_shift
        );

        // A bad pack path falls back to the named built-in.
        let fb = resolve_character("oblivion", Some("/no/such/pack.json")).expect("fallback");
        assert_eq!(fb.name, "oblivion");

        let _ = std::fs::remove_file(&path);
    }
}
