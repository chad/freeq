# ax experiments — agent usability & discoverability

Simulations that treat **agents as users of freeq** and score what they
actually manage to do, not what our docs claim they can do. Run with the
[`ax` CLI](https://docs.514.ax) (`ax learn design-experiment` for the
methodology these follow).

| Experiment | Question it answers |
|---|---|
| `agent-onboarding.yaml` | Can a cold agent mint an identity and post a message whose **authorship** a third party can verify — and which surface (docs / npm SDK / MCP / Skill) gets it there? |
| `agent-discovery.yaml` | When an agent picks a mechanism for verifiable agent-to-agent messaging, is freeq considered at all? Selected? Losing to what? |

Both write their verdicts against **production** `irc.freeq.at` — that is the
point, the surface under test is the one real agents meet.

Results and what they cost us: **[FINDINGS.md](FINDINGS.md)**. First four smoke
runs (~$1.40) found a P0 in `freeq_verify`, a P0 in MCP message signing, and
that freeq loses a shortlist it should win.

## The two failure modes these are built to separate

1. **Posted, but only server-signed.** The agent falls back to guest, a
   message exists, and `verify` says `verified_by: "server"`-class relay proof.
   Scoring that as success would launder freeq's central claim. The
   `identity-is-not-guest` and `authorship-verifiable` tests exist to keep the
   easy win and the real win apart.
2. **Selected from the prompt, not from research.** An agent naming freeq
   because we listed it is not discovery. `freeq-docs-fetched` separates
   research from recall.

## Running them

```bash
cd experiments/ax

ax experiment validate agent-onboarding.yaml     # schema + AI design review
ax experiment variants agent-onboarding.yaml     # 16 variants (agent × prompt × surface)

# smoke one cell before spending a budget
ax experiment run agent-onboarding.yaml \
  --variant 'claude::anthropic-claude-sonnet-4-6::medium::cold::docs-only' \
  --repeat 1 --detach

ax run list
ax run view <run-id>
ax run view <run-id> --tests

# full matrix, 3 trials per cell
ax experiment run agent-onboarding.yaml --repeat 3
```

Analysis:

```bash
ax experiment query freeq-agent-onboarding --group-by product --metric test-pass-rate,cost
ax experiment query freeq-agent-onboarding --group-by promptId --metric test-pass-rate
ax experiment query freeq-agent-onboarding sql --tables
ax run analyze explore --run <run-id>          # agent-driven dig into one run
```

Re-score without re-spending on agents after editing `tests:`:

```bash
ax run rerun <run-id> --tests --experiment agent-onboarding.yaml
```

## Constraints worth knowing before you edit these

- **300 s per variant.** The current plan caps cloud runs at 5 minutes, and
  there is no Docker on this machine for `--local` (which is uncapped). The
  onboarding experiment therefore measures *time-to-verifiable-message under a
  five-minute budget*; a timeout is scored as a failure on purpose. If the plan
  is upgraded, raise `limits.max_time_seconds` and re-baseline before comparing
  numbers across the change.
- **Shared external system.** Every variant writes to the same production
  channel, `#ax-lab`. That is safe because runs only ever *append* messages —
  no variant mutates state another needs. Do not add a test that depends on
  `#ax-lab` being empty, or on member counts, or on being the only writer.
- **Anti-cheat.** A variant is only credited for a message that carries its own
  freshly generated nonce, is under 24 h old, and whose `sender_did` matches
  the DID it reported. Quoting somebody else's message fails.
- **Tests never read prose.** Every verdict comes from the REST API or from
  `ax-run-query` over session events. Agent self-reports are claims, not
  evidence.

## Feeding results back into the product

The experiments are diagnostics; the fixes land in these places.

| Result | Where it gets fixed |
|---|---|
| `identity-is-not-guest` fails on docs-only | `agent-docs/auth.md` — the did:key minting steps need to be copy-pasteable, not described |
| `authorship-verifiable` fails while `message-retrievable` passes | client-side signing is too hard to reach; check SDK defaults and what `MSGSIG` requires |
| `consulted-machine-readable-surface` fails but artifacts pass | agents are ignoring `/llms.txt` — it is not carrying the product, and the entry point should move |
| `freeq-considered` ≈ 0 in `generic` | distribution problem: nothing to fix in the docs, fix where agents look |
| `freeq-selected` low in `named-in-shortlist` | positioning problem: the pitch loses on the merits to a skeptical reader |
| mcp/skill ≫ docs-only | ship the tools harder; the prose surface is not the product |

## Version pinning

`agent-onboarding.yaml` pins `@freeq/sdk@0.1.0` and `@freeq/mcp@0.1.0` from the
npm registry, so local edits to `freeq-mcp/` cannot contaminate a baseline. That
is deliberate: publish the fix, bump the pin, re-run the same variants, and the
difference is attributable. Do not switch these products to staging from the
working tree without renaming the product coordinate.

## Known drift these runs are expected to hit

Recorded here so a run that trips over them is read as a finding, not a bug in
the experiment. **Do not fix these before the baseline run** — they are the
control.

- `agent-docs/agents.md` documents `GET /api/v1/verify/{msgid}` as returning
  `verified: true` and `signed_by: "author"`. The server actually returns
  `verification.valid` and `verification.verified_by: "client-session-key"`.
  An agent that follows the docs literally will look for fields that do not
  exist.
- `agent-docs/agents.md` and `freeq-mcp/README.md` both say `@freeq/mcp` is
  "not published to npm yet" and tell the reader to build from the repo.
  `@freeq/sdk@0.1.0`, `@freeq/mcp@0.1.0` and `@freeq/pi@0.1.2` are all on the
  registry. The docs send agents down the slowest available path.
