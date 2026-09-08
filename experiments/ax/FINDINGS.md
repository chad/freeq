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
