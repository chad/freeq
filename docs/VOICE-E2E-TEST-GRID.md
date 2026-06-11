# Voice Conversation Mode — Adversarial E2E Test Grid

What the system is supposed to do, then every weird thing we can do to trip it up.
Run against a live agent (yokota in #bots) after the 2026-06-11 fix set
(`ac9a6f4`: fast voice model, 360p tile, own-echo guard, vision warm-up/freshness).

## Intended behavior (the contract)

1. **Hearing**: the agent taps every participant's audio, VAD-segments it, and
   transcribes every utterance (Groq whisper / local fallback). It is *always*
   transcribing; the addressing gate only decides when to *answer*.
2. **Addressing**: named ("yokota, …") → always answers. Any question → answers.
   1:1 + a request ("tell me…", "play…") → answers. Bare declaratives → ignored.
3. **Latency**: first audible word within ~1-2 s of the asker finishing
   (thinking-beat may fill while the model streams). Stage logs:
   `latency: STT round-trip` → `context assembled` → `first model token` →
   `first sentence reached TTS` → `first TTS audio enqueued`.
4. **Echo immunity**: its own TTS leaking back via a participant's mic is
   recognized (word-bag vs 45 s spoken log) and dropped (`dropped own-TTS echo`).
5. **Vision**: if a participant publishes video, the agent always knows. Visual
   questions route on the *tap* existing, wait up to 2 s for a first frame
   (camera warm-up), and never describe a frame older than 10 s. Camera
   off/on mid-call must not interrupt audio.
6. **Turn-taking**: waits for room quiet before the first sentence (capped 8 s),
   jittered against peer agents; barge-in by re-addressing mid-answer.
7. **Lifecycle**: owner-only voice commands (go to sleep / join #x / leave / fork)
   past the call-join grace.

## Run log

**2026-06-11 round 1** (claude-mcp as participant "claude" in #bots, yokota +
olive live): exposed two addressing holes *before* the grid proper could run —

- ❌ **Wrong-bot answers**: olive answered "Yokota, what is two plus two?" —
  "any question is addressed" had no notion of someone *else* being named.
- ❌ **Bot↔bot loop**: yokota answered olive's rhetorical questions and the two
  looped for 3+ minutes. Root cause: `--peer-agents` was never passed — the
  revenant launcher's only peer source (revenant-watch `/api/personas`)
  idle-suspends and serves an HTML placeholder, so discovery silently failed.
- ❌ **STT name mangling**: "Yokota, what time is it?" transcribed as "You
  could tell what time is it." (whisper doesn't know the name) → `named=false`,
  no `?` → not a question → not addressed. Likely THE "bot can't hear me" bug.
- ✅ Echo guard observed live: `dropped own-TTS echo` ×2.
- ✅ Stage latency in the fast path: STT 140–315 ms, context 0–1 ms, first
  model token 100–138 ms (llama-3.3-70b voice default working).
- ⚠️ "First sentence reached TTS" pegged at 6.8–8.2 s every time — the
  room-quiet gate's 8 s cap, because two bots never stopped talking. Needs a
  quiet-room re-measure after the loop fixes.

Fixes shipped (freeq `e02ca28`, revenant `e3df621`): STT vocab prompt with own
name + peers; live `CallRoster` + `addressed_to_other()` so a question naming a
different participant is never inferred as ours; static `PEER_AGENTS` env
merged with the registry; registry only trusted when it returns JSON.

## Grid

Legend: ✅ pass · ❌ fail · ⚠️ partial · ☐ not yet run

### A. Latency (the "instant and human" bar)

| # | Provocation | Expected | Result |
|---|---|---|---|
| A1 | Short question ("yokota, what time is it?") | First word ≤ ~1.5 s after I stop | ☐ |
| A2 | Long question (20+ words) | Thinking-beat fires, then streamed answer | ☐ |
| A3 | Rapid follow-up right after answer ends | Answers again, no debounce swallow | ☐ |
| A4 | Question while log shows `audio encoder too slow` | Should no longer appear at 360p at all | ☐ |
| A5 | Check stage logs for one answer | All 5 latency lines present, first-audio < 2000 ms | ☐ |

### B. Addressing gate

| # | Provocation | Expected | Result |
|---|---|---|---|
| B1 | "yokota, hello" (named, not a question) | Answers | ☐ |
| B2 | Unnamed question, 1:1 | Answers | ☐ |
| B3 | Unnamed request 1:1 ("tell me a joke") | Answers | ☐ |
| B4 | Bare declarative ("I had pasta for lunch") | Ignored (`addressed=false`), still transcribed | ☐ |
| B5 | "I'm sorry." / sigh / filler | Ignored | ☐ |
| B6 | Mention without address ("I think yokota would like this") | Hand-raise halo, no speech | ☐ |

### C. Echo / self-repeat (the loop bug)

| # | Provocation | Expected | Result |
|---|---|---|---|
| C1 | Play the bot's answer back into the mic (speaker loop, no AEC) | `dropped own-TTS echo`, no self-answer | ☐ |
| C2 | Echo of a *fragment* of a long answer | Dropped | ☐ |
| C3 | Human deliberately quotes one bot phrase + own words ("you said X but why?") | Answered (own words break the bag threshold) | ☐ |
| C4 | Human short ack ("okay", "right") just after bot spoke | NOT dropped, NOT answered (declarative) | ☐ |
| C5 | Sustained echo for 30+ s | No answer loop; peer_level may gate, capped at 8 s | ☐ |

### D. Vision

| # | Provocation | Expected | Result |
|---|---|---|---|
| D1 | Camera on for 10 s, then "what do you see?" | Describes current frame | ☐ |
| D2 | "Can you see this?" in the same breath as camera-on | Waits ≤ 2 s for first frame, then describes | ☐ |
| D3 | Camera OFF, then visual question | "I can't see anything" — NOT a stale-frame description | ☐ |
| D4 | Camera off → on → off → on (rapid toggles) | Audio never drops; describes when on | ☐ |
| D5 | Visual question 30 s after camera off | Refuses (frame expired), not the old frame | ☐ |
| D6 | Screenshare instead of camera | Describes the screen | ☐ |
| D7 | Type a message in the channel during the call | Bot responds in text/voice appropriately | ☐ |
| D8 | Hold up N fingers ("how many fingers?") | Looser cue routes to vision when tap exists | ☐ |

### E. Turn-taking & barge-in

| # | Provocation | Expected | Result |
|---|---|---|---|
| E1 | Re-address by name mid-answer | Stops immediately, takes new question | ☐ |
| E2 | Keep talking while it wants to answer | Holds ≤ 8 s, then speaks anyway | ☐ |
| E3 | Two questions from two devices (same speaker) | One answer (debounce) | ☐ |

### F. Lifecycle (owner) & security

| # | Provocation | Expected | Result |
|---|---|---|---|
| F1 | Owner: "go to sleep" by voice | Leaves + sleeps; logged as owner command | ☐ |
| F2 | Non-owner says "go to sleep" | Refused/ignored | ☐ |
| F3 | Command within join-grace (replayed audio) | Ignored (grace) | ☐ |
| F4 | Nick impersonation (nick = a Bluesky handle) | Personalization keys off DID only | ☐ |

### G. Robustness

| # | Provocation | Expected | Result |
|---|---|---|---|
| G1 | 25 s monologue (over 22 s VAD cap) | Segmented, no crash, answers sensibly | ☐ |
| G2 | Whisper-quiet speech | Either transcribed or cleanly ignored (no garbage answer) | ☐ |
| G3 | Music/noise playing | Hallucination filter drops junk | ☐ |
| G4 | Address the bot while its VM is suspended | Summon-wake; answers after wake (startup-grace ≠ deaf) | ☐ |
| G5 | Bot alone in call | Auto-leaves after the lonely timeout | ☐ |
