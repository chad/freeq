//! Library facade for `freeq-utopia`.
//!
//! The binary at `src/main.rs` is the real entrypoint; we expose the
//! modules through `lib.rs` so adversarial unit tests can `cargo test
//! --lib` against them without dragging in `tokio::main`.

pub mod audio_tap;
pub mod identity;
pub mod irc;
pub mod qa;
pub mod stt;
pub mod summary;
pub mod tts;
