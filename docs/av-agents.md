# Build a Voice & Video Agent

A freeq **AV agent** joins a live voice/video call, hears every
participant, and can talk and show video back. [Eliza](#reference-eliza)
— the agent that transcribes calls, answers spoken questions, and renders
a live video tile — is one. This guide shows how to build your own.

An AV agent is an ordinary freeq bot plus three crates that handle the
call:

| Crate | Role |
|-------|------|
| `freeq-sdk` (`freeq_sdk::av`) | Call **signaling** — start/join/leave over IRC. |
| `freeq-av` | The **media session** — connect to the SFU, publish the agent's audio + video, decode every participant. |
| `freeq-agent-kit` | **Voice-agent helpers** — turn a PCM stream into utterances, detect when the agent is addressed. |

The split mirrors the protocol: signaling is IRC, media is MoQ. If you
want the wire-level detail first, read the
[AV Call Protocol](/docs/av-protocol/). Otherwise, start here.

---

## The shape of an AV agent

```
   IRC connection  (freeq-sdk)
        │
        ├─ watch TAGMSGs ─▶ parse_av_state ─▶ "a call started"
        │                                          │
        │                                  AvSession::connect   (freeq-av)
        │                                          │
        │                          ┌───────────────┴───────────────┐
        │                   publish agent's              recv() one
        │                   audio + video                AvParticipant
        │                          │                     per speaker
        │                     Speaker                          │
        │                   (enqueue to talk)        VadSegmenter (agent-kit)
        │                                                       │
        │                                            utterance ─┴─▶ do something
        └────────────────────────────────────────────  (transcribe, answer, …)
```

Every agent has the same skeleton:

1. **Connect to IRC** and join its channels (a normal bot).
2. **Watch for `av-state`** TAGMSGs. When a call starts, decide whether
   to join.
3. **Open an `AvSession`** — it publishes the agent's broadcast and hands
   you one decoded-PCM stream per participant.
4. **Segment each PCM stream** into utterances with a `VadSegmenter`.
5. **Act** — transcribe, answer, record, whatever the agent is for. To
   talk back, enqueue audio on the `Speaker`.

---

## 1. Signaling — `freeq_sdk::av`

The agent connects to IRC like any bot (see the
[Bot Quickstart](/docs/bot-quickstart/)) and watches every `TAGMSG` for
call state:

```rust
use freeq_sdk::av::{parse_av_state, AvAction};

while let Some(event) = events.recv().await {
    if let Event::TagMsg { tags, .. } = &event {
        if let Some(state) = parse_av_state(tags) {
            match state.action {
                AvAction::Started => {
                    // A call opened. Join it.
                    let instance = freeq_sdk::av::new_av_instance();
                    handle.av_join(&channel, &state.session_id, &instance).await?;
                    // …then open an AvSession (step 2).
                }
                AvAction::Ended => { /* tear the call down */ }
                _ => {}
            }
        }
    }
}
```

`new_av_instance()` mints the per-device id; `handle.av_start` /
`av_join` / `av_leave` send the signaling TAGMSGs. To *initiate* a call
rather than wait for one, probe `GET /api/v1/channels/{channel}/sessions`
first and `av_start` only if there's no active session — see
[discover-or-start](/docs/av-protocol/#2-session-lifecycle).

---

## 2. The media session — `freeq-av`

`AvSession` is the whole media plane behind one handle. You give it where
to connect, an audio source for the agent's own voice, and a video
source; it connects to the SFU, publishes the agent's broadcast, watches
every other participant, decodes their audio, and reconnects on its own
if the transport drops.

```rust
use std::sync::Arc;
use std::sync::atomic::AtomicU32;
use freeq_av::{AvConfig, AvSession, Speaker, broadcast_path};

// A Speaker is how the agent talks; the paired source is what the
// session publishes. (The AtomicU32 is a loudness meter — wire it to a
// video tile, or ignore it.)
let (speaker, audio_source) = Speaker::new(Arc::new(AtomicU32::new(0)));

let config = AvConfig {
    sfu_url: "https://irc.freeq.at:8080/av/moq".parse()?,
    session_id: session_id.clone(),
    our_broadcast: broadcast_path(&session_id, "myagent", &instance),
    my_nick: "myagent".to_string(),
};

// `make_video` is called once per (re)connect — return a fresh
// iroh-live VideoSource each time. For a video-light agent, a static
// test pattern works; Eliza renders an animated tile (step 5).
let mut session = AvSession::connect(config, audio_source, move || {
    iroh_live::media::test_sources::TestPatternSource::new(640, 360)
});
```

`AvSession::recv()` then yields one `AvParticipant` per remote speaker
the session starts tapping. Each carries a `tokio::sync::mpsc::Receiver`
of decoded PCM frames:

```rust
while let Some(mut participant) = session.recv().await {
    println!("now hearing {}", participant.nick);
    tokio::spawn(async move {
        while let Some(frame) = participant.audio.recv().await {
            // `frame.samples` is f32 PCM at `frame.format` — feed it on.
        }
        // The receiver closes when the participant leaves or the
        // session reconnects. A reconnect re-announces everyone, so
        // `recv()` will yield them again.
    });
}
```

Dropping the `AvSession` ends the call: it stops publishing and closes
every participant stream.

### Talking back

Enqueue PCM on the `Speaker` and it goes out on the agent's broadcast:

```rust
speaker.enqueue(&tts_pcm, 24_000);  // resampled to the broadcast rate
if speaker.is_speaking() { /* still draining the queue */ }
speaker.clear();                    // stop immediately (barge-in)
```

---

## 3. Hearing words — `freeq-agent-kit`

A participant's audio arrives as a stream of tiny PCM frames. A
transcriber wants whole **utterances**. `VadSegmenter` does the cut: it
accumulates while someone is talking and flushes at a natural pause.

```rust
use freeq_agent_kit::{VadConfig, VadSegmenter};

let mut segmenter = VadSegmenter::new(VadConfig::default());

while let Some(frame) = participant.audio.recv().await {
    // Resample to 16 kHz mono for the recognizer first (see Eliza's
    // `to_whisper_pcm`), then:
    if let Some(utterance) = segmenter.push(&pcm_16k_mono) {
        // `utterance` is one complete spoken turn — send it to STT.
    }
}
```

`push` returns `Some` only on the frame that completes an utterance;
pre-speech silence and noise-only blips are dropped inside the segmenter,
so they never reach — and never hallucinate words out of — the recognizer.

The kit has the rest of the voice-agent glue too:

- **`extract_addressed(text, "myagent")`** — was the agent addressed by
  name? Returns the question if so. Tolerant of speech-to-text
  mishearings ("in miza" → "eliza") and one filler word ("hey eliza").
- **`is_hallucination(text)`** — drop the canonical Whisper silence
  phantoms ("Thank you.", "Bye.") before acting on a transcription.
- **`split_speech_and_links(reply)`** — split a reply into speakable text
  plus the URLs it mentioned, so the agent can read the answer aloud and
  post links as text.

`freeq-agent-kit` is dependency-free — pull it in without dragging the
media stack along.

---

## 4. Putting it together

The full skeleton, with the call lifecycle wired up:

```rust
// On AvAction::Started (or after a discover-or-start probe):
let instance = freeq_sdk::av::new_av_instance();
handle.av_join(&channel, &session_id, &instance).await?;

let (speaker, audio_source) = Speaker::new(Arc::new(AtomicU32::new(0)));
let config = AvConfig {
    sfu_url: sfu_url.clone(),
    session_id: session_id.clone(),
    our_broadcast: broadcast_path(&session_id, &nick, &instance),
    my_nick: nick.clone(),
};
let mut session = AvSession::connect(config, audio_source, make_video);

// Keep `speaker` and `session` alive for the call. Dropping `session`
// ends it. One task per participant:
tokio::spawn(async move {
    while let Some(mut p) = session.recv().await {
        let speaker = speaker.clone();
        tokio::spawn(async move {
            let mut seg = VadSegmenter::new(VadConfig::default());
            while let Some(frame) = p.audio.recv().await {
                let pcm = to_whisper_pcm(&frame.samples, frame.format);
                if let Some(utterance) = seg.push(&pcm) {
                    // transcribe → maybe answer → speaker.enqueue(...)
                }
            }
        });
    }
});
```

That is the entire agent loop. Everything past `seg.push` — the STT call,
the LLM, the text-to-speech, the video tile — is what makes *your* agent
different from the next one.

---

## Reference: Eliza

`freeq-eliza` is the worked example. On top of this skeleton she adds:

- **Speech-to-text** per utterance (Groq Whisper, or local whisper.cpp).
- **Spoken Q&A** — when `extract_addressed` fires, an LLM answers and
  ElevenLabs speaks the reply back through the `Speaker`.
- **Barge-in** — addressed again mid-answer, she calls `speaker.clear()`
  and takes the new question.
- **A live video tile** — an `iroh-live` `VideoSource` that renders an
  audio-reactive presence and LLM-designed answer cards.

Her code is the place to see every piece of this guide in production:
the av-state handler, the `AvSession` wiring, and the per-participant
VAD loop are all in `freeq-eliza/src/irc.rs`.

---

## See also

- [AV Call Protocol](/docs/av-protocol/) — the wire-level reference.
- [Bot Quickstart](/docs/bot-quickstart/) — the text-bot foundation an
  AV agent builds on.
- [Building Agents](/docs/agents/) — agent identity, governance, and
  coordination primitives.
