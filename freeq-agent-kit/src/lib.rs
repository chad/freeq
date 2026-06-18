//! `freeq-agent-kit` — voice-agent building blocks for freeq.
//!
//! An agent that joins a freeq AV call (see `freeq-av` for the media
//! transport and `freeq_sdk::av` for call signaling) has the same handful
//! of text/audio chores no matter what it does with the conversation.
//! This crate packages those, as small pure helpers with no dependencies:
//!
//! - [`vad`] — [`VadSegmenter`] turns a stream of PCM frames into
//!   discrete utterances, cutting at natural pauses.
//! - [`addressing`] — [`extract_addressed`] decides whether a line
//!   addresses the agent by name (tolerant of speech-to-text mishearings)
//!   and returns the question.
//! - [`speech`] — [`is_hallucination`] drops the canonical Whisper
//!   silence phantoms; [`split_speech_and_links`] separates a spoken
//!   reply from the URLs it mentioned.
//!
//! None of this is freeq-specific beyond the conventions it encodes — it
//! is the reusable half of an agent like `freeq-eliza`.

pub mod addressing;
pub mod speech;
pub mod vad;

pub use addressing::extract_addressed;
pub use speech::{is_hallucination, split_speech_and_links};
pub use vad::{Utterance, VadConfig, VadSegmenter};
