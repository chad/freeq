//! `freeq-av` — the reusable voice/video layer for freeq agents.
//!
//! freeq AV calls ride MoQ (Media over QUIC). This crate packages the
//! media side an agent needs, so building an AV agent doesn't mean
//! re-deriving the transport plumbing by hand:
//!
//! - [`audio`] — audio primitives: a [`Speaker`] for publishing the
//!   agent's own audio, the matching [`PushAudioSource`] for the
//!   encoder, a participant [`TapBackend`] that surfaces decoded remote
//!   PCM, and a band-limited [`resample_mono`].
//! - [`session`] — [`AvSession`]: connect to the MoQ SFU, publish the
//!   agent's broadcast, and tap every participant's audio, with
//!   automatic reconnect.
//!
//! The IRC-side call *signaling* (av-start / av-join / av-leave and the
//! `av-state` broadcasts) lives separately in `freeq_sdk::av`.

pub mod audio;
pub mod session;

pub use audio::{
    resample_mono, PcmFrame, PushAudioSource, Speaker, TapBackend, SPEAK_RATE,
};
pub use session::{
    broadcast_path, path_nick, AvConfig, AvParticipant, AvSession, VideoHandle,
};
