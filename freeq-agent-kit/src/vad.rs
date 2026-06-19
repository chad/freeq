//! Voice-activity segmentation: turning a PCM stream into utterances.
//!
//! A participant's microphone arrives as a continuous run of small PCM
//! frames. A transcriber wants whole *utterances* — a sentence or two,
//! cut at a natural pause. [`VadSegmenter`] does that cut:
//!
//! - it accumulates audio while the speaker is talking,
//! - it flushes the buffer when a long-enough silence ends the utterance
//!   (or a hard length cap is hit, so a monologue doesn't grow unbounded),
//! - it drops pre-speech silence and noise-only flushes, so silent
//!   stretches never reach the recognizer (where they become "Thank you."
//!   hallucinations — see [`crate::speech::is_hallucination`]).
//!
//! Feed it 16 kHz mono PCM (resample first if your source differs); the
//! [`VadConfig`] thresholds are expressed in samples at that rate.

/// The rate the default thresholds assume — 16 kHz, what Whisper-family
/// recognizers want.
const SAMPLE_RATE: usize = 16_000;

/// Tuning for [`VadSegmenter`]. All sample counts are at 16 kHz mono.
#[derive(Clone, Copy, Debug)]
pub struct VadConfig {
    /// Peak amplitude at or above which a frame counts as speech. Mic
    /// silence / room noise sits well under this; conversational speech
    /// peaks far above it. Deliberately low so quiet talkers aren't cut.
    pub voice_peak_threshold: f32,
    /// A trailing pause at least this long ends an utterance and triggers
    /// a flush. Long enough to ride over the gaps between words, short
    /// enough that the result still feels prompt.
    pub silence_gap_samples: usize,
    /// Hard cap on one utterance — flush even mid-speech so a monologue
    /// doesn't accumulate unbounded latency.
    pub max_utterance_samples: usize,
    /// Minimum voiced audio for a flush to be worth emitting. Below this
    /// the "utterance" is a cough / click / room noise — a prime
    /// hallucination source — and is dropped.
    pub min_voiced_samples: usize,
}

impl Default for VadConfig {
    /// Defaults tuned for conversational speech at 16 kHz: a 0.6 s pause
    /// ends an utterance, utterances are capped at 22 s, and a flush
    /// needs at least 0.45 s of actual voice to be emitted.
    fn default() -> Self {
        Self {
            // Raised from 0.018: room tone / background noise sat just above the
            // old floor and Whisper hallucinated phrases from it ("A live voice
            // call with the assistant…"), which the bot then answered. Real
            // conversational speech peaks well above 0.045.
            voice_peak_threshold: 0.045,
            // 0.45 s (was 0.6): the trailing pause before we decide the
            // turn ended is pure pre-answer latency. 0.45 s still rides
            // over inter-word gaps but shaves ~150 ms off perceived rhythm.
            silence_gap_samples: SAMPLE_RATE * 45 / 100, // 0.45 s
            max_utterance_samples: SAMPLE_RATE * 22,     // 22 s
            min_voiced_samples: SAMPLE_RATE * 45 / 100, // 0.45 s (was 0.35) — drop brief noise bursts
        }
    }
}

/// A completed utterance plus the segmentation stats that produced it.
/// Returned by [`VadSegmenter::push_stats`] for latency instrumentation.
pub struct Utterance {
    /// The utterance PCM (voiced audio plus mid-utterance + trailing pauses).
    pub pcm: Vec<f32>,
    /// Samples that crossed the voice threshold (actual speech).
    pub voiced_samples: usize,
    /// Trailing silence at the moment of flush — the end-of-turn dead air
    /// the VAD waited through before emitting. Pure pre-answer latency.
    pub trailing_silence_samples: usize,
}

/// Segments a PCM stream into utterances by voice activity.
///
/// Drive it by calling [`push`](VadSegmenter::push) with each frame as it
/// arrives. `push` returns `Some(utterance)` on the frame that completes
/// one, `None` otherwise.
pub struct VadSegmenter {
    config: VadConfig,
    /// The current utterance: voiced audio plus any mid-utterance pauses.
    buf: Vec<f32>,
    /// Voiced samples accumulated into `buf` (pauses excluded).
    voiced_samples: usize,
    /// Trailing silence since the last voiced frame.
    trailing_silence: usize,
}

impl VadSegmenter {
    /// A segmenter with the given thresholds.
    pub fn new(config: VadConfig) -> Self {
        Self {
            config,
            buf: Vec::new(),
            voiced_samples: 0,
            trailing_silence: 0,
        }
    }

    /// Feed one frame of 16 kHz mono PCM.
    ///
    /// Returns `Some(utterance)` when this frame completes one — a pause
    /// long enough to end it, or the hard length cap. Returns `None`
    /// while still accumulating, for pre-speech silence, and for a flush
    /// that turned out to be noise (less than `min_voiced_samples` of
    /// actual voice — silently dropped).
    pub fn push(&mut self, pcm: &[f32]) -> Option<Vec<f32>> {
        self.push_stats(pcm).map(|u| u.pcm)
    }

    /// Like [`push`](VadSegmenter::push) but returns the utterance plus the
    /// voiced/trailing-silence breakdown that drove the flush. Used by the
    /// voice-latency instrumentation: the trailing-silence gap is pure
    /// pre-answer latency (the VAD waits `silence_gap_samples` of quiet
    /// before it even knows the turn ended), and voiced vs total tells us
    /// how much of the STT audio is real speech.
    pub fn push_stats(&mut self, pcm: &[f32]) -> Option<Utterance> {
        if pcm.is_empty() {
            return None;
        }
        let peak = pcm.iter().fold(0.0f32, |m, &s| m.max(s.abs()));
        let voiced = peak >= self.config.voice_peak_threshold;

        if voiced {
            self.buf.extend_from_slice(pcm);
            self.voiced_samples += pcm.len();
            self.trailing_silence = 0;
        } else if !self.buf.is_empty() {
            // Mid-utterance silence: keep it (the pause is part of natural
            // speech and helps the recognizer) and count it toward the
            // end-of-utterance gap.
            self.buf.extend_from_slice(pcm);
            self.trailing_silence += pcm.len();
        }
        // else: pre-speech silence — drop it, never accumulate.

        let pause_flush =
            self.trailing_silence >= self.config.silence_gap_samples && !self.buf.is_empty();
        let cap_flush = self.buf.len() >= self.config.max_utterance_samples;
        if !pause_flush && !cap_flush {
            return None;
        }

        let chunk = std::mem::take(&mut self.buf);
        let chunk_voiced = self.voiced_samples;
        // Capture trailing silence at the moment of flush (before reset) —
        // this is the dead-air tax that delayed the turn end.
        let chunk_trailing = self.trailing_silence;
        self.voiced_samples = 0;
        self.trailing_silence = 0;

        // Too little actual speech to be worth a transcription round-trip.
        if chunk_voiced < self.config.min_voiced_samples {
            return None;
        }
        Some(Utterance {
            pcm: chunk,
            voiced_samples: chunk_voiced,
            trailing_silence_samples: chunk_trailing,
        })
    }

    /// Samples currently buffered in the in-progress utterance. Useful
    /// for a progress log; not needed to drive segmentation.
    pub fn buffered(&self) -> usize {
        self.buf.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Small, fast thresholds for tests — same shape as the real config.
    fn test_config() -> VadConfig {
        VadConfig {
            voice_peak_threshold: 0.5,
            silence_gap_samples: 10,
            max_utterance_samples: 100,
            min_voiced_samples: 4,
        }
    }

    #[test]
    fn default_config_matches_the_documented_16k_tuning() {
        let c = VadConfig::default();
        assert_eq!(c.silence_gap_samples, 7_200); // 0.45 s
        assert_eq!(c.max_utterance_samples, 352_000); // 22 s
        assert_eq!(c.min_voiced_samples, 7_200); // 0.45 s
    }

    #[test]
    fn pure_silence_never_flushes() {
        let mut seg = VadSegmenter::new(test_config());
        for _ in 0..50 {
            assert!(seg.push(&[0.0; 20]).is_none());
        }
        assert_eq!(seg.buffered(), 0, "pre-speech silence must not accumulate");
    }

    #[test]
    fn voiced_burst_then_pause_flushes_the_utterance() {
        let mut seg = VadSegmenter::new(test_config());
        // 8 voiced samples — not yet a flush.
        assert!(seg.push(&[1.0; 8]).is_none());
        assert_eq!(seg.buffered(), 8);
        // 12 silent samples → trailing silence (12) crosses the 10-sample
        // gap → flush.
        let utterance = seg.push(&[0.0; 12]).expect("should flush");
        // The utterance keeps the voiced audio *and* the trailing pause.
        assert_eq!(utterance.len(), 20);
        assert_eq!(seg.buffered(), 0, "buffer resets after a flush");
    }

    #[test]
    fn pre_speech_silence_is_not_part_of_the_utterance() {
        let mut seg = VadSegmenter::new(test_config());
        // Leading silence — dropped.
        assert!(seg.push(&[0.0; 30]).is_none());
        // Then speech.
        assert!(seg.push(&[1.0; 8]).is_none());
        let utterance = seg.push(&[0.0; 12]).expect("flush");
        assert_eq!(utterance.len(), 20, "leading silence must not be prepended");
    }

    #[test]
    fn noise_only_flush_is_dropped() {
        let mut seg = VadSegmenter::new(test_config());
        // 3 voiced samples — below min_voiced_samples (4).
        assert!(seg.push(&[1.0; 3]).is_none());
        // Trailing silence triggers a flush, but the chunk is noise.
        assert!(
            seg.push(&[0.0; 12]).is_none(),
            "a flush below min_voiced_samples must be dropped",
        );
        // State still resets so the next utterance segments cleanly.
        assert_eq!(seg.buffered(), 0);
    }

    #[test]
    fn length_cap_flushes_mid_speech() {
        let mut seg = VadSegmenter::new(test_config());
        // 120 voiced samples in one frame — over the 100-sample cap.
        let utterance = seg.push(&[1.0; 120]).expect("cap flush");
        assert_eq!(utterance.len(), 120);
    }

    #[test]
    fn segments_two_utterances_back_to_back() {
        let mut seg = VadSegmenter::new(test_config());
        seg.push(&[1.0; 8]);
        let first = seg.push(&[0.0; 12]).expect("first flush");
        assert_eq!(first.len(), 20);
        // Second utterance — the segmenter is reusable.
        seg.push(&[1.0; 6]);
        let second = seg.push(&[0.0; 10]).expect("second flush");
        assert_eq!(second.len(), 16);
    }

    #[test]
    fn empty_frame_is_a_no_op() {
        let mut seg = VadSegmenter::new(test_config());
        seg.push(&[1.0; 8]);
        let before = seg.buffered();
        assert!(seg.push(&[]).is_none());
        assert_eq!(seg.buffered(), before, "empty frame must not change state");
    }

    #[test]
    fn mid_utterance_pause_is_kept_and_does_not_split() {
        let mut seg = VadSegmenter::new(test_config());
        seg.push(&[1.0; 8]); // speech
        // A short pause (under the 10-sample gap) — kept, no flush.
        assert!(seg.push(&[0.0; 6]).is_none());
        // More speech — resets the trailing-silence counter.
        assert!(seg.push(&[1.0; 8]).is_none());
        // Now a long pause ends it; the utterance spans both bursts and
        // the gap between them.
        let utterance = seg.push(&[0.0; 12]).expect("flush");
        assert_eq!(utterance.len(), 8 + 6 + 8 + 12);
    }
}
