# Voice pipeline latency instrumentation + optimization

Goal: get Alexandria back to a **natural rhythm of speech**. Make the whole voice
round-trip measurable so we can see where the time goes, then optimize in a loop.

## The metric that matters
**speech_end → first audio out** (the gap a human perceives as "how long before she
starts answering"). Everything else is a breakdown of that number.

## Pipeline stages (inbound → outbound)
1. **VAD** (`freeq-agent-kit/src/vad.rs`): PCM → utterance. Trailing-silence tax
   (`silence_to_end`) is pure latency before we even start.
2. **STT** (`stt.rs::transcribe`): Groq whisper round-trip. `irc.rs:3385`.
3. **Addressing decision** (`irc.rs:3470+`): cheap, but logged.
4. **Answer** (`answer_and_speak` `irc.rs:1564`): route (`qa::route_question`),
   context assembly (memory + feed), model first-token, full generation
   (`qa::answer_streaming` / `anthropic_answer_streaming`).
5. **TTS** (`tts::synthesize_streaming` via `synth_and_enqueue` `irc.rs:1483`):
   request → first PCM byte → enqueue.
6. **Speaker/playout** (`freeq-av/src/audio.rs:244` `enqueue`): buffer → MoQ.

## Existing logs (off `t0` = answer entry, uncorrelated)
- `latency: STT round-trip`
- `question routed`, `latency: context assembled`, `latency: first model token`
- `latency: first sentence reached TTS`, `latency: first TTS audio enqueued`

## Instrumentation to add
- [ ] **Turn id** (atomic counter) minted at VAD flush; threaded STT → answer → TTS.
      Every latency line carries `turn=`.
- [ ] **speech_end anchor** (`t_flush` at VAD flush) threaded into `answer_and_speak`.
- [ ] **VAD breakdown** log: voiced_ms, trailing_silence_ms, utterance_ms.
- [ ] **STT** log: + turn, + audio_sec, + chars, + realtime-factor.
- [ ] **answer entry** offset from speech_end (STT+decide gap).
- [ ] **TTS net** (request → first byte) per first sentence, in `synth_and_enqueue`.
- [ ] **answer total** (first token → last token), tokens/chars, char/s.
- [ ] **per-turn SUMMARY line**: one line, all stages, anchored at speech_end →
      first audio. This is the at-a-glance "where did the time go".

## Test harness
- [ ] `voice_probe` example: publishes a spoken-audio WAV (generated via macOS
      `say` or ElevenLabs) as the probe's audio track into #chadtest, so the
      worker's STT fires and the full pipeline runs. Capture the SUMMARY from the
      worker log.

## Loop
- [ ] Baseline: several runs, record SUMMARY numbers, find the dominant stage.
- [ ] Optimize the dominant stage, re-test, repeat until rhythm is natural.

## Candidate optimizations (fill in after baseline)
- VAD `silence_to_end` shorter (less trailing dead air) vs over-cutting.
- TTS: `eleven_flash_v2_5` vs current model; `optimize_streaming_latency`.
- Model: voice_answer_model first-token latency (sonnet vs haiku for snap).
- Drop/!timebox feed cold-open + memory recall on voice path.
- Thinking-beat tuning (filler while composing).

## Status log
- (start) pipeline mapped; instrumenting.
- Instrumentation DONE + compiles: turn ids + speech_end anchor + VAD breakdown
  (voiced/trailing) + STT (audio_sec/rtf) + route/ctx/first-token/answer-complete
  + TTS first-byte + first-sentence + first-audio + per-turn SUMMARY
  `latency: SUMMARY speech_end→first_audio (rhythm)`. Files: vad.rs (push_stats +
  Utterance), agent-kit lib export, irc.rs (TurnClock, transcribe_participant,
  answer_and_speak, synth_and_enqueue).
- voice_probe harness built (publishes spoken WAV). BASELINE (turn=1):
  speech→first_audio = **1617ms** (+ 600ms VAD before clock = ~2.2s real).
  Breakdown: STT 569 (rtf .05), route 136, first-sentence-to-TTS 841 (← gate!),
  TTS first-byte 206, first-audio 1048 (compose-rel), first token 1188, answer
  done 2267. STT/TTS cheap; the **quiet-gate jitter (250–1000ms, solo-irrelevant)**
  + 600ms VAD dominate.
- OPTIMIZE round 1: (a) skip anti-collision jitter in wait_for_room_quiet when
  solo (no peers); (b) VAD silence_gap 0.6→0.45s; (c) gate-duration log.
  RESULT: speech→first_audio 1617 → ~835ms (3 runs 824/834/846). −48%.
- OPTIMIZE round 2: quiet HOLD 250→150ms (VAD already confirms silence first).
  RESULT (steady-state, 8 runs): gate 289→~167ms, compose→first_audio ~480→~390ms.
  speech→first_audio steady avg **~938ms** vs 1617 baseline = **−42%**.
  STT (~550ms for the 8.3s test question; ~300ms for a normal 3-4s turn) is now
  the dominant, network-bound component. Compose path is tight + consistent.

## RESULT / where the time goes now
- VAD trailing (pre-clock): 460ms — tunable, left at 0.45s for cutoff safety.
- STT (Groq whisper): ~300–600ms, network-bound, the remaining floor.
- quiet gate: ~167ms. route: parallel/free. TTS first-byte: ~190ms.
- thinking beat ("Hey Chad!") starts ~speech_end+938ms, masking model first-token
  (~1.2s) so the answer flows in naturally behind it.

## Remaining knobs (opt-in — quality/robustness tradeoffs, not applied)
- `eleven_flash_v2_5` TTS (−~100ms first-byte) — touches voice quality.
- VAD silence_gap <0.45s — snappier but risks cutting people off.
- Local whisper — removes STT network variance but needs model + CPU.

## STATUS: satisfied. Instrumentation complete; 42% rhythm improvement via two
## safe changes; remaining latency is network-bound STT.
