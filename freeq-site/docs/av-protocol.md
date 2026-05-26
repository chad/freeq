# AV Call Protocol

freeq voice and video calls are built from two independent layers:

- **Signaling** rides ordinary IRC. Participants announce calls with
  `TAGMSG`s; the server echoes call state back to the channel. This is
  the *control plane* — who is in the call, and what its id is.
- **Media** rides a separate transport: MoQ (Media over QUIC) through an
  SFU. This is the *data plane* — the actual Opus audio and H.264 video.

Keeping them apart means a call is just metadata on a normal channel:
any IRC client sees the `TAGMSG`s, and a client with no AV support is
unaffected. This page is the reference for both layers. To *build* an
agent on top of them, see [Build a Voice & Video Agent](/docs/av-agents/).

---

## 1. Signaling: the `+freeq.at/av-*` tags

All call signaling is IRCv3 `TAGMSG`s sent to a channel. A client opens,
joins, or leaves a call by sending message tags; the server validates the
action and broadcasts an **`av-state`** `TAGMSG` to every channel member.

### Client → server

| Tag | Value | Meaning |
|-----|-------|---------|
| `+freeq.at/av-start` | *(empty)* | Open a new call in this channel. |
| `+freeq.at/av-join` | *(empty)* | Join the call named by `av-id`. |
| `+freeq.at/av-leave` | *(empty)* | Leave the call named by `av-id`. |
| `+freeq.at/av-instance` | instance id | This client's per-device id (see §3). |
| `+freeq.at/av-id` | session id | The call to join/leave (not used on `av-start`). |
| `+freeq.at/av-title` | text | Optional human-readable call title (on `av-start`). |

### Server → channel

The server answers every accepted action with one `av-state` `TAGMSG`:

| Tag | Value | Meaning |
|-----|-------|---------|
| `+freeq.at/av-state` | `started` / `joined` / `left` / `ended` | What changed. |
| `+freeq.at/av-id` | session id | The call. The MoQ broadcast-path prefix (§3). |
| `+freeq.at/av-actor` | nick | Who did it, when the server includes it. |
| `+freeq.at/av-participants` | integer | Live participant count after the change. |
| `+freeq.at/av-title` | text | The call title, when set. |

`av-state=ended` fires when the last participant leaves.

### In the Rust SDK

`freeq_sdk::av` builds these tag maps and parses the replies, so an agent
never hand-assembles a `HashMap`:

```rust
use freeq_sdk::av::{self, AvAction, parse_av_state};

// Sending: ClientHandle has the three convenience methods.
handle.av_start("#standup", &av::new_av_instance(), Some("Daily standup")).await?;
handle.av_join("#standup", &session_id, &instance).await?;
handle.av_leave("#standup", &session_id, &instance).await?;

// Receiving: apply parse_av_state to every TAGMSG you get.
if let Some(state) = parse_av_state(&tags) {
    match state.action {
        AvAction::Started => { /* a call opened — decide whether to join */ }
        AvAction::Joined  => { /* someone joined state.session_id */ }
        AvAction::Left    => {}
        AvAction::Ended   => { /* tear down */ }
    }
}
```

`parse_av_state` returns `None` for any `TAGMSG` that isn't an
`av-state` broadcast, so it is safe to call on every incoming tag event.

---

## 2. Session lifecycle

```
  Alice                     Server                      Bob
    │  TAGMSG av-start ───────▶│                          │
    │                          │── av-state=started ─────▶│   (broadcast
    │◀──── av-state=started ───│                          │    to channel)
    │                          │                          │
    │                          │◀──── TAGMSG av-join ─────│
    │◀──── av-state=joined ────│──── av-state=joined ────▶│
    │                          │                          │
    │   ══ media flows over MoQ between Alice and Bob ══   │
    │                          │                          │
    │                          │◀──── TAGMSG av-leave ────│
    │◀──── av-state=left ──────│──── av-state=left ──────▶│
    │  TAGMSG av-leave ───────▶│                          │
    │◀──── av-state=ended ─────│── av-state=ended ───────▶│
```

1. **Start.** A client sends `av-start` with a fresh instance id. The
   server creates a session, assigns it a **session id**, and echoes
   `av-state=started` carrying `av-id=<session id>`. The starter is the
   call's first participant — it does **not** also send `av-join`.
2. **Join.** Other clients send `av-join` with that `av-id` and their own
   instance id. The server echoes `av-state=joined`.
3. **Media.** Each participant publishes a MoQ broadcast and subscribes
   to the others (§3, §4).
4. **Leave.** `av-leave` produces `av-state=left`; the last leave
   produces `av-state=ended`.

### Discover-or-start

A blind `av-start` is rejected when the channel already has a live call.
An agent that wants a call running should **probe first**:

```
GET /api/v1/channels/{channel}/sessions
→ { "active": { "id": "<session id>", "state": "Active", ... } }
```

If `active` is non-null and `Active`, `av-join` it; otherwise `av-start`.
This avoids the race where two clients both try to open the same call.

---

## 3. Broadcast addressing

Every participant publishes exactly one MoQ broadcast. Its path is:

```
{session_id}/{nick}~{instance}
```

- **`session_id`** — the call, from `av-id`. Shared by every broadcast in
  the call, so a subscriber filters the SFU's announce stream by the
  `"{session_id}/"` prefix and ignores stale broadcasts from other calls.
- **`nick`** — the participant's display name.
- **`instance`** — a per-device id: 8 lowercase hex characters
  (`freeq_sdk::av::new_av_instance()`). The same identity joining from
  two devices gets two instance ids, so the two broadcast paths don't
  collide. An agent skipping its *own* broadcast (to avoid transcribing
  its own voice) matches on `nick`, not the full path.

`freeq-av` has helpers for both directions:

```rust
use freeq_av::{broadcast_path, path_nick};

let path = broadcast_path("01HXYZ", "eliza", "0a1b2c3d"); // "01HXYZ/eliza~0a1b2c3d"
let nick = path_nick(&path);                              // "eliza"
```

---

## 4. Media transport

Media rides **MoQ — Media over QUIC** — through an SFU (selective
forwarding unit). The SFU endpoint is `/av/moq` on the freeq server host;
QUIC is the low-latency path, with a WebSocket fallback for environments
where QUIC can't establish.

- **Audio** — Opus, 48 kHz mono.
- **Video** — H.264.
- Each participant publishes its own broadcast and subscribes to every
  other broadcast in the same session. The SFU forwards; it does not mix.

A subscriber decodes each remote broadcast to PCM locally. Publishing a
*continuous* audio stream (silence included) keeps subscribers attached,
so there is no join latency when a participant actually starts talking.

The `freeq-av` crate packages this whole plane — connecting, publishing,
watching the announce stream, and decoding every participant — behind one
`AvSession`. See the [agent tutorial](/docs/av-agents/).

---

## 5. Compatibility

- A client that ignores `+freeq.at/av-*` tags sees a normal channel.
- A call adds no channel modes and no special channel state beyond the
  `TAGMSG` history.
- The signaling and media layers are independent: signaling works with
  no media support, and the media transport carries no identity of its
  own — it trusts the IRC-side `session_id`.

---

## See also

- [Build a Voice & Video Agent](/docs/av-agents/) — the tutorial.
- [Bot Quickstart](/docs/bot-quickstart/) — a text-only bot in 10 minutes.
- [Protocol Notes](/docs/protocol/) — the SASL `ATPROTO-CHALLENGE`
  mechanism and the rest of the IRC extensions.
