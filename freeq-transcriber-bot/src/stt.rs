//! Speech-to-text. Real implementation lives behind the `stt` feature
//! because whisper-rs pulls in whisper.cpp via cmake. Without the
//! feature we ship a no-op Whisper that returns empty transcriptions —
//! enough to exercise the full IRC + MoQ + transcript-relay pipeline
//! in tests without a model file or a C++ toolchain.

use std::path::Path;

use anyhow::Result;

#[cfg(feature = "stt")]
mod imp {
    use super::*;
    use std::sync::Mutex;
    use anyhow::Context;
    use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

    pub struct Whisper {
        ctx: Mutex<WhisperContext>,
    }

    impl Whisper {
        pub fn load(path: &Path) -> Result<Self> {
            let path_str = path
                .to_str()
                .context("whisper model path is not valid UTF-8")?;
            let ctx = WhisperContext::new_with_params(path_str, WhisperContextParameters::default())
                .context("WhisperContext::new failed; is the model path correct?")?;
            Ok(Self { ctx: Mutex::new(ctx) })
        }

        pub fn transcribe(&self, pcm_16k_mono: &[f32]) -> Result<String> {
            if pcm_16k_mono.len() < 16_000 {
                return Ok(String::new());
            }
            let mut ctx = self.ctx.lock().expect("whisper context poisoned");
            let mut state = ctx.create_state().context("whisper create_state failed")?;

            let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
            params.set_language(Some("en"));
            params.set_translate(false);
            params.set_no_context(true);
            params.set_print_special(false);
            params.set_print_progress(false);
            params.set_print_realtime(false);
            params.set_print_timestamps(false);
            params.set_suppress_blank(true);
            params.set_suppress_nst(true);

            state
                .full(params, pcm_16k_mono)
                .context("whisper inference failed")?;

            let segments = state.full_n_segments().unwrap_or(0);
            let mut out = String::new();
            for i in 0..segments {
                if let Ok(text) = state.full_get_segment_text(i) {
                    out.push_str(&text);
                }
            }
            Ok(out.trim().to_string())
        }
    }
}

#[cfg(not(feature = "stt"))]
mod imp {
    use super::*;

    pub struct Whisper;

    impl Whisper {
        pub fn load(_path: &Path) -> Result<Self> {
            tracing::warn!(
                "freeq-transcriber-bot built without the `stt` feature — STT is a no-op. \
                 Build with `--features stt` after installing cmake to enable whisper."
            );
            Ok(Self)
        }

        pub fn transcribe(&self, _pcm_16k_mono: &[f32]) -> Result<String> {
            Ok(String::new())
        }
    }
}

pub use imp::Whisper;

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// Without the `stt` feature, `Whisper::load` is a no-op: it must
    /// succeed for *any* path — even paths that don't exist, that
    /// aren't UTF-8 on disk, that point at a directory, or that contain
    /// embedded NULs we wouldn't otherwise tolerate. This is the
    /// contract the rest of the crate relies on so tests can run
    /// without a model file. If a future refactor accidentally re-adds
    /// a `path.exists()` check the bot's CI breaks.
    #[cfg(not(feature = "stt"))]
    #[test]
    fn load_no_op_for_nonexistent_paths() {
        for p in [
            "/nope/nope/nope/never.bin",
            "",
            "/dev/null",
            "../../etc/passwd",
            "/tmp/with spaces/and(parens)/file.bin",
        ] {
            Whisper::load(&PathBuf::from(p))
                .unwrap_or_else(|e| panic!("no-op Whisper rejected path {p:?}: {e:#}"));
        }
    }

    /// Without the feature, `transcribe` always returns `Ok("")`, no
    /// matter the input shape. The IRC orchestrator filters empty
    /// strings before posting, so this is what keeps the no-feature
    /// build from spamming `[transcript] nick:` lines.
    #[cfg(not(feature = "stt"))]
    #[test]
    fn transcribe_returns_empty_string_for_any_input() {
        let w = Whisper::load(&PathBuf::from("/dev/null")).unwrap();
        for input in [vec![], vec![0.0; 1], vec![0.5; 16_000], vec![f32::NAN; 32_000]] {
            let out = w.transcribe(&input).expect("transcribe errored");
            assert_eq!(out, "", "unexpected output for input len {}", input.len());
        }
    }

    /// The orchestrator stores `Whisper` inside `Arc<Whisper>` and
    /// hands it to `tokio::task::spawn_blocking`, which requires
    /// `Send + 'static`. Future fields (e.g. raw pointers, RefCell)
    /// would silently break that. Pin it with a compile-time check.
    #[test]
    fn whisper_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync + 'static>() {}
        assert_send_sync::<Whisper>();
    }
}
