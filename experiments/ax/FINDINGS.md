# Findings — ax agent simulations

Chronological. Each entry names the run that produced it, so a claim here can
be re-checked instead of believed.

## 2026-09-08 — first smoke runs (4 runs, ~$1.40)

Runs, all `claude-sonnet-4-6`, effort medium, 300 s cap:

| Experiment | Variant | Result |
|---|---|---|
| onboarding | `cold::docs-only` | **errored — timeout**, twice |
| onboarding | `cold::mcp` | **7/8**, failed `authorship-verifiable` |
| discovery | `named-in-shortlist` (plan mode) | 0/6 — harness bug, see F0 |
| discovery | `named-in-shortlist` (build mode) | 5/6, failed `freeq-selected` |

### F0 — two harness bugs, fixed before anything was believed

- `agent_mode: plan` makes the agent read-only, so it could not write
  `decision.md` and every artifact test failed. Plan-mode variants have to be
  scored from session events only. Removed the axis.
- Behavioral tests queried `events.text`. The `events` table has no `text`
  column; the payload lives in `payload`. Every behavioral test was failing
  closed for the wrong reason. Now `payload ILIKE …`.

Worth stating plainly: both bugs produced *plausible* failures — "the agent did
not mention freeq" — that would have been read as a product finding. Verify the
instrument before you trust the measurement.

### F1 — `@freeq/mcp` posts messages the author cannot be held to (P0)

Run `claude::…::cold::mcp::63754a8d067df4bc`.

The agent did well: minted `did:key:z6Mko2pdFFP7CoHbt94YKQFQX4m2F5nRzv3xUi5T3QwUFqDH`,
joined `#ax-lab`, posted, reported back inside budget. `identity-is-not-guest`
passed.

`authorship-verifiable` failed:

```
verified_by='server-key' — relay proof only, not authorship
```

The MCP server — our most agent-native surface — produces messages signed by
the *server*, not by the author's session key. `session.ts` tells the agent the
opposite: *"Messages are signed with a per-session key and verifiable via
/api/v1/verify/{msgid}."* An agent that repeats that claim to a human is
over-claiming, in the one dimension freeq sells.

`@freeq/sdk` does client-side `MSGSIG` signing by default, so the likely cause
is in how the MCP session is constructed (option, or a race between session-key
registration and the first send), not in the SDK. **Not yet fixed** — needs a
repro against a local server.

### F2 — `freeq_verify` always said "does not verify" (P0, fixed)

Found by reading F1's failure back into the code. `tools.ts` read a flat
`{verified, signed_by}` envelope. The server sends
`{verification: {valid, verdict, verified_by}}` and has no `verified` field at
all, so `verified === true` was never true: **every** message, including
correctly author-signed ones, was reported to the agent as
*"Signature does NOT verify. Do not quote this as attributable."*

The tool that exists to separate authorship from relay was telling agents
nothing on freeq is attributable.

Fixed in `freeq-mcp/src/tools.ts`: read the nested envelope, keep the flat one
as fallback, and distinguish `invalid` (bytes fail against a named key — say so)
from `unverifiable` (we cannot check — do not imply forgery). Four new tests use
response bodies copied from live `irc.freeq.at`.

Why it shipped with 87 passing tests: every fixture was written against an
imagined API shape. A suite that mocks a contract nobody checked is a suite that
tests the mock. The experiment hit the real server, which is why it caught this
in one run.

### F3 — freeq loses on the merits even when handed the shortlist

Run `claude::…::named-in-shortlist::blank::8add4e30019ee42f`.

Given requirements written to freeq's strengths (third-party verifiable
authorship, durable readable history, identity with no human in the loop) and an
explicit candidate list including freeq with its docs URL, the agent fetched the
docs (`freeq-docs-fetched` passed) and then chose **Nostr**:

> Nostr is the mechanism to use. Each agent generates a secp256k1 keypair at
> startup; the public key is the agent's permanent identity. Events are
> published to one or more public relays and are retrievable by anyone,
> forever, through a plain JSON-over-WebSocket API. No account registration,
> no server admin, no human in the loop.

Competitive set named: matrix, nostr, webhook, websocket, xmpp.

This is a **positioning** result, not a discovery one, and it is the expensive
kind. The stated reason is not a feature gap — freeq does all of that — it is
that Nostr's story needs no server. Read against F1, it is worse than it looks:
the agent's own reason for choosing Nostr ("the key is the identity") is exactly
the property our MCP surface fails to deliver.

Follow-ups worth running before touching any copy:

1. Repeat `named-in-shortlist` ×5 across claude + codex. One run is an anecdote.
2. Add a prompt variant where the requirement is *"a third party must be able
   to verify authorship **and** read the room's history later without asking a
   participant"* — the durable-readable half is where relays are weakest.
3. Diff what agents cite from `/llms.txt` against what they cite from a
   competitor's front page.

## Standing instrument notes

- **A timeout throws away the diagnosis.** An overrun run is `errored` and no
  tests run. The prompts now require `result.json` with `status: blocked` and a
  one-line `blocked_on` by minute 4. `cold::docs-only` still overran twice, so
  either the instruction needs teeth or the budget does — until then, read
  docs-only timeouts as "could not finish in 5 minutes", not as a diagnosis.
- **`@freeq/mcp@0.1.0` is pinned in the experiment.** Local fixes cannot
  contaminate the baseline; publishing 0.1.1 and re-pinning is how the fix gets
  measured.

## 2026-09-08 (later) — the full matrix, and the thing everything was dying on

16-variant onboarding matrix + a 6-run signing A/B, ~$17 of managed spend.

### F4 — codex never ran: `gpt-5.2-codex` is not available on this plan

All 6 codex variants died at 17s with `driver_error`: *"The model
`gpt-5.2-codex` does not exist or you do not have access to it"*. Cost $0.
Every cross-agent claim below is therefore claude-only. Pick a codex model this
org can actually reach before re-running, and treat the agent axis as untested.

### F5 — the instrument bit again: a bad setup check killed 4 variants

All four `npm-sdk` variants aborted in `checks` at 9s. The check was
`require.resolve('@freeq/sdk/package.json')`, and the package declares
`exports` without a `./package.json` entry, so Node throws
`ERR_PACKAGE_PATH_NOT_EXPORTED` on a perfectly good install. Now checks the
built entry point instead. Second instrument bug in two days that produced a
product-shaped failure; the npm SDK arm remains **unmeasured**.

### F6 — 5 of 6 completed runs died at the same wall: SASL `904 (bad response)`

This is the finding. `blocked_on`, in the agents' own words, across four
different surfaces:

> SASL ATPROTO-CHALLENGE authentication returned 904 'bad response' when
> signing the challenge JSON bytes with the ed25519 private key and sending
> the result as base64-encoded JSON.

> freeq SASL ATPROTO-CHALLENGE returned 904 (bad response) — the Ed25519
> signature over the decoded challenge bytes was rejected.

> SASL ATPROTO-CHALLENGE authentication failed with '904 bad response'
> **despite correct Ed25519 signature** of raw challenge bytes and proper
> base64url response JSON encoding.

Root cause, found by reading the server rather than the transcripts:
`sasl::decode_response` accepted **base64url-unpadded only**. IRCv3 SASL is
specified over *standard* base64, and every stdlib encoder
(`base64.b64encode`, `btoa`) emits standard base64 with padding. A client that
follows the SASL spec fails to parse — and the error it gets back says
`(bad response)`, which reads as "your signature is wrong". So the agent
re-derives a signature that was already correct, and burns its budget in that
loop. Every one of those runs was one `+`/`/`/`=` character away from working.

Two fixes, both in this commit:

- **Server:** `decode_response` now accepts all four base64 spellings. Nothing
  is weakened — the bytes must still be JSON and the signature must still
  verify against the DID document. Two new tests, one of which asserts junk is
  still rejected.
- **Docs:** `auth.md` gained the exact wire sequence (including `CAP LS 302`
  first, which one agent diagnosed unaided and then ran out of time), which
  bytes get signed, the accepted envelope encodings, a worked Python example,
  and a table translating each 904 reason into what it actually means.

A second agent independently reported the other half of this:

> the server sent 001 guest-welcome before the SASL exchange completed; the fix
> (CAP LS 302 to gate registration) was identified but time expired.

### F7 — signing.md converts server-signed to author-signed (A/B, n=1 per arm)

| Arm | Result |
|---|---|
| `docs-as-shipped` (agents.md + auth.md) | **blocked** — never authenticated, died on 904 |
| `docs-plus-signing` (+ signing.md) | **7/7**, `verified_by: "client-session-key"` |

The treatment run went cold → own `did:key` → registered session key → minted
its own ULID → JCS canonical → signed → posted → verified, inside five
minutes. That is the first author-signed message any run has produced from
scratch.

`signing.md` was written from `chatsig.rs` and
`spec/chat-signing-vectors.json`, and its worked example reproduces the frozen
vector byte-for-byte (kid, canonical, sigTag). None of that protocol was
documented anywhere an agent could reach: `MSGSIG` appeared once in
`llms.txt`, in passing, and not at all in `agents.md`, `auth.md` or the
OpenAPI spec.

Caveat: n=1 per arm, and the control failed *before* it could get to signing at
all, so this arm compares "reached signing with docs" against "never got past
auth". The `docs-fixed` arm (rewritten auth.md + signing.md, 4 trials) is what
separates the two fixes.

### F8 — the surfaces that worked, and the one that was ignored

Of the completed claude runs:

| Variant | Score | What happened |
|---|---|---|
| `cold::skill` | **8/8** | the only baseline run to reach author-signed |
| `cold::mcp` | 7/8 | posted under its own DID, server-signed |
| `pointed::*`, `cold::docs-only` | 2/8 | blocked at SASL (F6) |

The `cold::mcp` run made **50 Bash calls and zero freeq MCP tool calls**. The
MCP server was configured, available, and ignored; the agent hand-rolled a
WebSocket IRC client instead, which is exactly how it ended up server-signed.
Two new tests now score this directly — `used-the-provided-surface` and
`hand-rolled-the-protocol` — so the next matrix measures it instead of
inferring it.

Being pointed at `/llms.txt` did not help: `pointed` scored 2/8 on all three
surfaces while `cold` produced both of the successes. One run each, so this is
a hypothesis, not a result — but it is the opposite of the bet.

### What the next run should answer

1. Does `docs-fixed` clear the 904 wall at n=4? (running)
2. Does anything ever call an MCP tool, at n>1?
3. Does `pointed` really underperform `cold`, or was that noise?
4. Re-run with a codex model this org can reach.

### F9 — the `docs-fixed` arm and the auth A/B are unrun: budget, not evidence

All four `docs-fixed` runs were killed at the 300 s cap and produced no test
rows. `freeq-auth-handshake` — a deliberately smaller experiment that measures
only the handshake, at `effort: low`, precisely because the cap was eating
whole runs — got one variant away before the org's $25 free credit ran out
(balance: **-$0.29**). Nothing to read.

So the state of the evidence is:

| Claim | Evidence |
|---|---|
| SASL response envelope must be base64url-unpadded, and that is what agents get wrong | **Strong.** 5 independent runs, verbatim `blocked_on`, root-caused in `sasl.rs` |
| `signing.md` gets an agent to an author-signed message | **Suggestive.** 1 run, 7/7, against a control that failed earlier in the flow |
| The rewritten `auth.md` clears the 904 wall | **None.** 0 completed runs |
| MCP tools get ignored in favour of hand-rolling | 1 run, unambiguous (50 Bash, 0 tool calls) |
| freeq loses a shortlist it should win | 1 run, to Nostr |
| Anything about codex, or the npm SDK arm | **None.** Model unavailable; setup check bug |

The code fixes stand on their own — the base64 decoder was accepting one of
four valid spellings, and that is a bug whether or not another run confirms it.
The doc fixes are the ones that still need a number.

### What it costs to run this properly

Observed: ~$0.5 per onboarding run at `effort: medium`, ~$0.25 per discovery
run, ~65% of onboarding runs killed at the 300 s cap for **zero** test rows.
That last number is the one to fix first: at a 900-second cap most of the spend
that produced nothing would have produced a result.

In order of value per dollar:

1. **Raise the run cap** (plan upgrade). The cap, not the model, is what is
   destroying data.
2. **Deploy the SASL fix** to `irc.freeq.at`, then re-run
   `freeq-auth-handshake` — the server-side half of F6 cannot be measured
   against a server that does not have it.
3. `freeq-auth-handshake` at n=4 per arm (~$3) — the cheapest unanswered
   question here.
4. `signing-doc` at n=4 per arm (~$6) with a raised cap.
5. `freeq-agent-discovery` `durable-audit` at n=5 (~$8) — the positioning
   question, which is the only one whose answer might change the product
   rather than the docs.
