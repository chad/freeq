//! `freeq-claude-mcp` — bridges a Claude Code agent into a freeq AV
//! call.
//!
//! The library half exposes [`Orchestrator`]: a lean bot that joins
//! IRC + SASL, joins (or starts) the channel's AV session, taps every
//! participant's decoded audio through Whisper, and exposes:
//!
//! - a `Receiver<Transcript>` of voice utterances (with an `addressed`
//!   bool that's true when the line addresses the bot by name), and
//! - a `say(text)` call that synthesizes ElevenLabs TTS and broadcasts
//!   it into the call.
//!
//! Nothing here calls an LLM — the brain is Claude Code, sitting on the
//! other side of the MCP transport. See `bin/mcp_server.rs` for the
//! wrapper that exposes these as MCP tools, and `bin/stdio_driver.rs`
//! for a JSON-lines driver useful for manual testing.

pub mod discover;
pub mod orchestrator;
pub mod streaming_stt;
pub mod tile_overlay;
pub mod video_face;

pub use orchestrator::{OrcConfig, Orchestrator, SayPriority, SayResult, Transcript};
pub use tile_overlay::TileOverlay;
pub use video_face::ParticleControl;
