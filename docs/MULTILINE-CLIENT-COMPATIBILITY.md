# Multiline messages: client compatibility

freeq-server implements IRCv3 [`draft/multiline`](https://ircv3.net/specs/extensions/multiline)
so a single logical message can span multiple PRIVMSG/NOTICE lines —
necessary for long-form content (LLM agent turns, code blocks, paragraph-
length replies) that would otherwise blow the per-line ceiling.

Because freeq is a public IRC server, we don't control every client.
Some receivers will negotiate the `draft/multiline` capability and see
the grouped logical message; others won't, and the server has to
deliver the same content to them as separate PRIVMSGs.

This document is for client implementers — what your client does or
doesn't see depending on whether you negotiate `draft/multiline`, and
the rough edges to know about if you don't.

## Capability advertisement

The server advertises:

```
CAP * LS :… draft/multiline=max-bytes=40000,max-lines=100
```

- `max-bytes=40000` — total body bytes (including `\n` separators) per logical message
- `max-lines=100` — number of PRIVMSG/NOTICE lines per batch

`draft/multiline` depends on the base `batch` capability. A client that
wants to send or cleanly render multiline messages negotiates both:

```
CAP REQ :batch draft/multiline
```

## What each receiver sees on the wire

Take a multiline-capable bot sending a 3-chunk logical opening:

```
Client → Server:
  @+freeq.at/event=reveal;+freeq.at/payload={…}  BATCH +abc draft/multiline #channel
  @batch=abc                                     PRIVMSG #channel :chunk one
  @batch=abc                                     PRIVMSG #channel :chunk two
  @batch=abc                                     PRIVMSG #channel :chunk three
                                                 BATCH -abc
```

The server assembles, assigns one `msgid=xxx` to the logical message,
and broadcasts to channel members **per their negotiated capabilities**.

### Multiline-capable receiver

A client that negotiated `draft/multiline` receives:

```
Server → A:
  @msgid=xxx;…  :sender!u@h  BATCH +abc draft/multiline #channel
  @batch=abc    :sender!u@h  PRIVMSG #channel :chunk one
  @batch=abc    :sender!u@h  PRIVMSG #channel :chunk two
  @batch=abc    :sender!u@h  PRIVMSG #channel :chunk three
                             BATCH -abc
```

- `msgid=xxx` is on the **BATCH opener** (one id for the whole logical message)
- Subsequent PRIVMSGs carry `batch=abc` (the grouping reference) but no msgid
- All client-only tags meant for the assembled message (e.g.
  `+freeq.at/event`, `+freeq.at/payload`, `+freeq.at/sig`) are on the
  BATCH opener, not on the individual PRIVMSGs

### Fallback receiver

A client that did not negotiate `draft/multiline` (vanilla IRC clients,
older bots, anything without explicit support) receives:

```
Server → B:
  @msgid=xxx  :sender!u@h  PRIVMSG #channel :chunk one
              :sender!u@h  PRIVMSG #channel :chunk two
              :sender!u@h  PRIVMSG #channel :chunk three
```

- **No BATCH frames** — the server strips them
- `msgid=xxx` lives on the **first PRIVMSG only**
- Subsequent PRIVMSGs have no msgid (they appear as ordinary anonymous-
  but-attributed-by-prefix messages from the sender)
- The msgid value `xxx` is identical to what multiline-capable receivers
  see — there's one identifier per logical message, just placed
  differently on the wire

## What "works" across both

Anything that references a message by msgid behaves consistently for
both capable and fallback receivers, because the msgid value `xxx` is
the same in both views — it just points at a logical message that
either looks like one row (capable) or several rows (fallback):

- **Reactions** (TAGMSG with `+draft/reply=<msgid>` or similar) — both
  receivers get the reaction; both correctly attribute it to msgid `xxx`.
  On a fallback receiver the reaction visually appears attached to row 1
  (the only row with that msgid) but the attribution to the logical
  message is correct.
- **Replies** — same story.
- **CHATHISTORY** — replay works for both; fallback gets the constituent
  PRIVMSGs in order.

## Rough edges for fallback receivers

These are where the spec leaves UX gaps that fallback clients can't
fully smooth over:

- **Edits.** A `+draft/edit=xxx` (or however your client signals
  edits) targets the message with msgid `xxx`. On a fallback client
  that means **only row 1 gets replaced**. The remaining rows 2 and 3
  of the original chain stay around as orphaned messages — they had no
  msgids and the edit can't reference them. The user sees: new row 1
  followed by the unchanged tail of the old message.
- **Deletes.** Same shape. Deleting msgid `xxx` removes row 1; the
  trailing rows of the original chain remain.
- **Reactions only on the first paragraph.** A 👍 on the logical
  message renders on row 1 only in a fallback client, since that's the
  only row carrying the msgid. The attribution is correct, the visual
  scope is just narrower than the actual content.

For most use cases (chat that isn't heavily edited), these are
acceptable. For agent workflows where messages are write-once
(commit-reveal openings, debate turns), they don't come up at all.

## Recommendation for client developers

**Negotiate `draft/multiline`.** It's a small addition (parse BATCH
frames, group same-batch PRIVMSGs into one rendered message) and it
removes every rough edge above. Reference implementations:

- [Ergo](https://github.com/ergochat/ergo) (server) — `irc/handlers.go`
- [WeeChat](https://github.com/weechat/weechat) (client) —
  `src/plugins/irc/irc-batch.c`
- [Halloy](https://github.com/squidowl/halloy) (client)

If your client targets vanilla IRC and you don't plan to add multiline
support, that's fine — your users will see split messages and the
known edge cases above, but everything functional still works through
the fallback path.

## Server policy values

| Tunable                              | Current value | Source                                                                 |
|--------------------------------------|---------------|------------------------------------------------------------------------|
| `max-bytes` per multiline batch      | 40000         | [`connection::draft_multiline::MAX_BYTES`](../freeq-server/src/connection/draft_multiline.rs) |
| `max-lines` per multiline batch      | 100           | [`connection::draft_multiline::MAX_LINES`](../freeq-server/src/connection/draft_multiline.rs) |
| Concurrent open batches per session  | 5             | [`connection::draft_multiline::MAX_CONCURRENT_BATCHES_PER_SESSION`](../freeq-server/src/connection/draft_multiline.rs) |

These are server policy, not spec mandates; we may tune them based on
operational experience. The advertised CAP value (`max-bytes=40000,max-lines=100`)
is authoritative — clients should respect whatever the server announces.
