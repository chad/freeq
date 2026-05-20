# freeq-transcriber-bot

A sample agent that joins freeq AV (voice/video) sessions, transcribes the
audio with [whisper.cpp](https://github.com/ggerganov/whisper.cpp), and posts
the running transcript — plus an optional end-of-call summary + action items —
back into the channel.

It's a reference implementation for "an agent that participates in a call":
IRC identity + SASL, MoQ media subscription, a decoded-PCM tap, STT, and an
LLM post-processing step, wired end to end.

## What it does

1. Loads (or mints, on first run) a `did:key` identity at
   `~/.freeq/bots/<name>/`. Layout is interchangeable with `freeq-bot-id`
   and `@freeq/bot-kit`.
2. Connects to a freeq IRC server and authenticates via SASL
   `ATPROTO-CHALLENGE` with that key.
3. Joins the channels you give it and watches for
   `+freeq.at/av-state=started` TAGMSGs.
4. When a call starts in one of those channels: sends `+freeq.at/av-join`,
   opens a MoQ subscriber against the server's SFU, and subscribes to every
   participant's broadcast.
5. Taps decoded PCM out of each remote audio track (a custom
   `AudioStreamFactory` that captures samples instead of playing them),
   resamples to 16 kHz mono, and runs whisper over rolling windows.
6. Posts each utterance as `[transcript] <nick>: <text>`.
7. On `av-state=ended`, optionally sends the full transcript to the
   Anthropic API for a **Summary** + **Action items** block and posts that.

One active call at a time. A call starting in a second channel while the bot
is busy is logged and skipped.

## Build

The default build has **no STT** — `stt::Whisper` is a no-op that returns
empty transcriptions. That's deliberate: the full IRC + MoQ + relay path is
exercised (and unit/e2e tested) without a C++ toolchain or a model file.

```bash
# control-plane only — joins calls, relays the "listening"/"ended" lines,
# but every audio window transcribes to "" (no [transcript] <nick>: lines)
cargo build --release -p freeq-transcriber-bot
```

For **real transcription**, enable the `stt` feature. It builds whisper.cpp
from source, which needs `cmake` and a C++ toolchain:

```bash
brew install cmake          # macOS;  apt install cmake  on Debian/Ubuntu
cargo build --release -p freeq-transcriber-bot --features stt
```

## Model

With `--features stt` you need a ggml whisper model. `ggml-small.en.bin`
(~466 MB) is a good latency/accuracy balance for CPU:

```bash
mkdir -p models
curl -L -o models/ggml-small.en.bin \
  https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.en.bin
```

`ggml-base.en.bin` (~142 MB) is faster and lighter if the box is small;
`ggml-medium.en.bin` is more accurate but slower.

## Run

```bash
# Transcript-only (no summary)
cargo run --release -p freeq-transcriber-bot --features stt -- \
  --server wss://irc.freeq.at/irc \
  --channel '#avtest' \
  --model-path ./models/ggml-small.en.bin

# With end-of-call summary + action items
ANTHROPIC_API_KEY=sk-ant-... \
cargo run --release -p freeq-transcriber-bot --features stt -- \
  --server wss://irc.freeq.at/irc \
  --channel '#avtest' \
  --model-path ./models/ggml-small.en.bin
```

First run mints `~/.freeq/bots/transcriber/`. Subsequent runs reuse the DID.

### CLI flags

| Flag | Default | Notes |
|---|---|---|
| `--server` | `wss://irc.freeq.at/irc` | `wss://`/`https://` → WebSocket; `host:port` → raw TCP |
| `--channel` | `#avtest` | Repeatable. Bot only transcribes calls in channels it's in |
| `--name` | `transcriber` | Identity dir: `~/.freeq/bots/<name>/` |
| `--nick` | = `--name` | IRC nick |
| `--model-path` | `./models/ggml-small.en.bin` | ggml whisper model (used only with `--features stt`) |
| `--window-secs` | `10` | Audio accumulated before each whisper pass — lower = snappier, more CPU |
| `--summary-model` | `claude-sonnet-4-5` | Anthropic model for the end-of-call summary |
| `--no-summary` | off | Skip the summary even if `ANTHROPIC_API_KEY` is set |

`ANTHROPIC_API_KEY` (env): when set and `--no-summary` is absent, the bot
generates an end-of-call summary. When unset, it's transcript-only.

## A/B testing it during development

For now this bot is meant to be run by hand for an A/B test, not deployed as
a service. Start it pointed at a single channel, join a call from the web or
iOS client, and watch the `[transcript]` lines land. Stop it with Ctrl-C.

## Tests

```bash
# Unit + integration — runs without cmake or a model file (stt feature off)
cargo test -p freeq-transcriber-bot
```

The suite covers identity minting + path-traversal hardening, the PCM
resampler's adversarial inputs (NaN/∞, extreme rates), the summary API
client's error paths, SFU-URL derivation, and end-to-end scenarios against
in-process freeq-server instances.

## Architecture

```
freeq IRC  ──TAGMSG av-state=started──►  irc.rs : watch loop
   ▲                                         │
   │ PRIVMSG [transcript] ...                 │ send av-join
   │                                         ▼
   │                              moq subscriber (iroh-live)
   │                                         │
   │                          per-participant audio track
   │                                         ▼
   │                          audio_tap.rs : TapBackend
   │                            (AudioStreamFactory that
   │                             captures PCM, drops on
   │                             backpressure)
   │                                         ▼
   │                          to_whisper_pcm: downmix + 16 kHz
   │                                         ▼
   └────────── post utterance ◄──── stt.rs : whisper.cpp
                                              │
                          av-state=ended ─────┤
                                              ▼
                                  summary.rs : Anthropic API
```

| File | Responsibility |
|---|---|
| `main.rs` | CLI parsing, startup |
| `identity.rs` | did:key load/mint, name sanitization |
| `irc.rs` | IRC connect, av-state watch, MoQ subscribe, orchestration |
| `audio_tap.rs` | `AudioStreamFactory` PCM capture + resampler |
| `stt.rs` | whisper.cpp wrapper (feature-gated) / no-op fallback |
| `summary.rs` | Anthropic Messages API client |

## Known limitations

- One concurrent call. A second simultaneous call is skipped, not queued.
- The naïve linear resampler in `audio_tap.rs` is tuned for speech, not music.
- The transcript is sent verbatim to the summary model — a very long call
  could exceed the context window and surface as an API error rather than a
  graceful truncation.
- whisper runs in fixed windows, so utterances can be split at window
  boundaries.
