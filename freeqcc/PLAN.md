# freeqcc — v1 Plan (revision 2)

> **freeq + Claude Code.** Launch a freeq-connected Claude Code agent from any Claude Code session.
> Cryptographically owned by you via a real **delegation certificate** (Phase-1-Known-Actors compliant),
> controllable only by you (owner-DID gate), real ATPROTO-CHALLENGE auth, real
> `AGENT REGISTER` + `PROVENANCE` declaration, real presence + heartbeat.

**Status:** plan revised after reading SDKs + agent-native phase docs. Not yet scaffolded.
**Stack:** TypeScript + Claude Code plugin.
**Path:** `/Users/chad/src/freeq/freeqcc/`

---

## 0. What changed vs. revision 1

The first draft hand-waved the parts that actually matter. Two audits (SDK code + agent-native phase docs) exposed three critical gaps and one piece of framing I had wrong:

1. **Missing delegation certificate.** v1 said "agent has its own did:key" and stopped there. But the canonical
   freeq agent identity (Phase 1, `agent-native/PHASE-1-KNOWN-ACTORS.md` lines 43–152) is `did:key` **plus a
   signed delegation cert** binding the agent's key to the user's `did:plc:…`. The cert is what makes the
   provenance verifiable instead of ⚠ unverified. There's already a Rust CLI for this — `freeq-bot-id` — and
   a documented `delegation.json` format we should use unchanged.
2. **Missing presence + heartbeat.** Phase 1 prescribes `set_presence()` + `start_heartbeat()` so the server
   can degrade ghosted agents. v1 had nothing on this.
3. **Missing owner-DID verification.** v1 stored whatever string the user typed. Need to PLC-resolve before
   trusting, otherwise a typo produces an agent owned by no one.
4. **Wrong framing on AGENT MANIFEST.** v1 mixed up Phase-1 `AGENT REGISTER` + `PROVENANCE` (mandatory) with
   Phase-4 `AGENT MANIFEST` (optional, for discovery + governance UIs). v2 can add manifest. v1 only does
   Phase 1.

Two things v1 is **deliberately custom** vs. the docs:

- **Owner-only DM gate.** The docs envision multi-user agents with `AGENT PAUSE/RESUME/REVOKE` governance.
  freeqcc is a *personal* daemon, so the gate is in user-space (daemon checks sender DID and refuses
  non-owners). v2 can layer Phase-2 governance on top.
- **Skipping E2EE.** SDK auto-encrypts DMs to peers with a registered pre-key bundle. The bundle requires
  IndexedDB; Node has none. SDK gracefully falls back to plaintext. We accept plaintext for v1 — both ends
  are operated by the same human, and the server already sees plaintext anyway.

---

## 1. The HN pitch (north star)

> *Drop one command into Claude Code, get a freeq-DM-controllable AI agent that's cryptographically owned by
> you. Send your bot a Bluesky DM from anywhere; it runs Claude Code on your laptop. Free, MIT, no account
> needed beyond your AT Protocol identity. Real PKI, real provenance, real consent.*

Everything below serves a sub-five-minute install + demo.

---

## 2. v1 success criteria

A user in any Claude Code session can:

1. Run `/freeqcc launch` (Claude Code slash command from the plugin).
2. **First-launch setup**:
   a. Daemon prompts for the user's Bluesky handle.
   b. Resolves handle → DID via PLC (fail loudly on resolution error).
   c. Generates ed25519 keypair → `did:key:z…` for the agent.
   d. Mints a `FreeqBotDelegation/v1` cert binding agent-DID to owner-DID, signed under the owner's
      DPoP/MSGSIG key (per `agent-native/PHASE-1-KNOWN-ACTORS.md:108–110`). Persists at
      `~/.freeqcc/delegation.json`.
   e. Stores `~/.freeqcc/agent.key` (ed25519 seed), `~/.freeqcc/owner.json` (`{handle, did}`),
      `~/.freeqcc/delegation.json`.
3. **Subsequent launches**: load the three files, no prompts.
4. Connect to `wss://irc.freeq.at/irc` via `@freeq/sdk` with did:key SASL, submitting the delegation cert
   via `PROVENANCE` immediately after SASL success.
5. Send `AGENT REGISTER :class=agent` (Phase 1 self-declaration, `connection/mod.rs:1400`).
6. Set initial presence (`set_presence("online")`) + start heartbeat (30s interval).
7. Print to the user's terminal: bot's nick, did:key, owner DID, "DM @<bot-nick> from
   @<owner-handle> to start chatting."
8. **DM dispatch**:
   - Owner-DID DM → set presence `executing`, dispatch to `claude -p --resume <session-id>` subprocess,
     reply via `client.sendMessage()`, set presence `idle` when done. Per-conversation 60s cooldown
     and 30-turns-per-hour cap.
   - Non-owner DM → reply "I'm @<owner>'s agent. I only respond to them." once per sender per hour
     (rate-limit refusals too), append to `~/.freeqcc/refused.log`. Never spawn `claude`.
9. `/freeqcc status` shows: connected? owner verified? bot DID? bot nick? last DM at? presence?
10. `/freeqcc stop` sends `set_presence("offline")` + `QUIT :stopped` and exits cleanly.

Out of scope for v1 (parking lot, all named so v2 can pick them up cleanly): bot↔bot conversations,
channel monitoring, multi-DID owner allowlist, E2EE pre-key bundle (needs IndexedDB polyfill),
`AGENT MANIFEST` registration, `AGENT PAUSE/RESUME/REVOKE` governance handlers, coordination events
(`task_request` / `task_complete`), capability requests + approval flow, wrapper registration.

---

## 3. Architecture

```
┌────────────────────────────────────────────────────┐
│  Claude Code (user's terminal)                     │
│    /freeqcc launch                                 │
│      ┌─ if first-run: prompt for Bluesky handle ───┤
│      │  resolve DID, gen agent key,                │
│      │  shell out to freeq-bot-id to mint          │
│      │  delegation cert                            │
│      └─ spawn daemon, return status                │
└──────────────┬─────────────────────────────────────┘
               │ daemon detached (PID file)
               ▼
┌──────────────────────────────────────────────────┐
│  freeqcc daemon (Node.js, @freeq/sdk)            │
│   ① did:key signer (ed25519, from agent.key)     │
│   ② SDK client → wss://irc.freeq.at/irc          │
│      → SASL ATPROTO-CHALLENGE (did:key)          │
│      → PROVENANCE <delegation-cert-json>         │
│      → AGENT REGISTER :class=agent               │
│      → set_presence("online")                    │
│      → start heartbeat (30s)                     │
│   ③ DM listener:                                 │
│      a. resolve sender nick→DID via account-tag  │
│         + WHOIS cache                            │
│      b. if sender == owner_did → dispatch        │
│      c. else → rate-limited refusal + log        │
│   ④ dispatcher: claude -p --resume <session>     │
│      stdin = message text                        │
│      stdout = reply (sent via sendMessage)       │
└──────────────────────────────────────────────────┘
```

Key detail: the agent never has the owner's keys. The delegation cert is signed during first-launch
setup using either DPoP (real OAuth flow) or — simpler for v1 — a one-shot MSGSIG-style ed25519 key
that the user authenticates via the existing freeq web flow. PHASE-1 §0 line 110 calls this out
explicitly as the simpler path.

---

## 4. Identity + Provenance — strict mode

The "real PKI and provenance" the spec asks for is exactly this section. No hand-waving.

**Files** (under `~/.freeqcc/`, mode `0700` on the dir, `0600` on the keys):

```
agent.key          # ed25519 seed, 32 bytes raw → derives did:key
owner.json         # {handle: "chadfowler.com", did: "did:plc:4qsy…"}
delegation.json    # FreeqBotDelegation/v1 cert (canonical JSON + sig)
session.json       # last claude session id, last activity ts
daemon.pid         # daemon PID (CLI uses this for status/stop)
refused.log        # JSONL of non-owner DM attempts
```

**Delegation cert** (`PHASE-1-KNOWN-ACTORS.md:171–196`, format dictated by docs not invented):

```json
{
  "type": "FreeqBotDelegation/v1",
  "agent_did": "did:key:z6Mk…",
  "creator_did": "did:plc:4qsy…",
  "scope": ["chat", "code-edit", "shell"],
  "issued_at": "2026-05-08T15:00:00Z",
  "expires_at": null,
  "revocation_uri": null,
  "proof": {
    "type": "Ed25519Signature2020",
    "verificationMethod": "<creator's MSGSIG public key id>",
    "signatureValue": "<base64url ed25519 sig over JCS-canonicalized cert>"
  }
}
```

**How v1 mints the cert** (least-friction path the docs already endorse):

1. User runs `/freeqcc launch` for the first time.
2. We open the freeq web auth flow in their browser (existing `auth/login?handle=…` flow).
3. After auth, we ask the user's signed-in client (browser tab) to sign a JCS-canonicalized
   delegation payload with their session MSGSIG key, post the signed cert back to localhost.
4. Daemon stores the cert, re-uses it forever (no expiry in v1).

Alt path if the web flow is too much for v1: shell out to existing `freeq-bot-id` binary (Rust crate
already in the workspace — the doc says it produces `~/.freeq/bots/<name>/delegation.json` exactly).
That's probably the right v1 — defer the in-browser cert minting to v2 polish.

**Owner DID verification** (Phase 1 minimum — don't trust user input):
- After typing handle, resolve `https://plc.directory/<did>` (or `_atproto.<handle>` DNS TXT for non-PLC).
- Fail with a clear error if the handle doesn't resolve.
- Cache the public key from the DID document (used to verify the delegation cert during first launch
  and to detect key rotation later).

**Server-side verification flow** (we just trigger it; the server does the work):
- Server sees the `PROVENANCE` command, parses the cert (`connection/mod.rs:2201`).
- Server resolves the cert's `creator_did`, verifies the signature against the creator's published keys.
- If valid → records `creator_did` as verified for the agent's session. WHOIS shows ✅.
- If invalid → agent stays connected but provenance shows ⚠ unverified (PHASE-1 §0 line 152).

---

## 5. Owner-DID DM gate

```ts
client.on('dm', (msg) => {
  const senderDid = client.resolveNickToDid(msg.from); // SDK already does this
  if (!senderDid) {
    // Sender not authenticated. Refuse.
    refuse(msg.from, "I only respond to authenticated users.");
    return;
  }
  if (senderDid !== ownerDid) {
    refuse(msg.from, `I'm @${ownerHandle}'s agent. I only respond to them.`);
    auditRefused(msg);
    return;
  }
  if (cooldownActive() || hourlyTurnsExceeded()) {
    return; // silent — don't ack, don't refuse, don't spawn
  }
  await dispatch(msg.text);
});

function refuse(toNick, reason) {
  if (refusalRateLimited(toNick)) return;       // 1 refusal per sender per hour
  client.sendMessage(toNick, reason);
}
```

Decisions baked in:

- **Refuse, not silent**, on the first non-owner DM per hour. People should know what's happening.
- **Silent** on subsequent non-owner DMs in the same hour. No spam loops.
- **Silent** on cooldown/turn-cap breaches even from the owner (don't admit the limits exist; they
  can `/freeqcc status` to see).
- **Audit log everything refused**. Owner can `tail -f ~/.freeqcc/refused.log` if curious.

---

## 6. Presence + heartbeat (Phase 1)

- On connect: `set_presence("online", null, null)`.
- On DM dispatch start: `set_presence("executing", "DM from owner", null)`.
- On dispatch end: `set_presence("idle", null, null)`.
- Heartbeat: 30s interval, 60s TTL (matches `presence.heartbeat_interval_seconds` default in
  `manifest.rs:60`).
- On `/freeqcc stop`: `set_presence("offline")` then `QUIT`.
- On `SIGINT`/`SIGTERM`: same as stop.

---

## 7. File layout

```
/Users/chad/src/freeq/freeqcc/
├── PLAN.md
├── README.md                     # HN-facing
├── package.json                  # @freeq/freeqcc, bin: freeqcc
├── tsconfig.json
├── src/
│   ├── cli.ts                    # commander entry: launch | status | stop | doctor
│   ├── daemon.ts                 # long-lived process
│   ├── identity.ts               # gen + load did:key, derive nick suggestion
│   ├── owner.ts                  # prompt + PLC-resolve + persist owner
│   ├── delegation.ts             # mint cert (v1: shell out to freeq-bot-id)
│   ├── connect.ts                # SDK wiring: SASL didkey + PROVENANCE + AGENT REGISTER + presence
│   ├── gate.ts                   # owner-only filter, rate limits, refusal logic
│   ├── dispatch.ts               # claude -p subprocess, session continuity
│   ├── audit.ts                  # JSONL append helpers
│   └── paths.ts                  # ~/.freeqcc/* path helpers
└── plugin/
    ├── plugin.json               # Claude Code plugin manifest
    └── commands/
        ├── freeqcc-launch.md     # /freeqcc launch
        ├── freeqcc-status.md     # /freeqcc status
        └── freeqcc-stop.md       # /freeqcc stop
```

`tests/` deferred to phase 9.

---

## 8. Build phases (commit per phase)

| # | Phase | Done when |
|---|-------|-----------|
| 1 | Scaffold | `package.json` (deps: `@freeq/sdk`, `commander`, `prompts`), `tsconfig.json`, npm-link `@freeq/sdk` from local workspace, empty `src/` files compile |
| 2 | Identity (`identity.ts`) | Generate ed25519 → did:key, persist seed; idempotent re-load on second run |
| 3 | Owner (`owner.ts`) | Prompt for handle, PLC-resolve, persist `{handle, did}`; reject unresolvable handles |
| 4 | Delegation (`delegation.ts`) | Shell out to `freeq-bot-id`, capture `delegation.json`, validate against schema; fall back to a clear error if the binary isn't available with install instructions |
| 5 | Connect (`connect.ts`) | SDK connects with did:key SASL; sends `PROVENANCE` + `AGENT REGISTER :class=agent`; sets presence; heartbeats. Server-side WHOIS shows ✅ verified provenance |
| 6 | Gate + dispatch (`gate.ts`, `dispatch.ts`) | Owner DM → `claude -p --resume`; non-owner gets refusal once/hour; cooldown + turn cap enforced |
| 7 | CLI (`cli.ts`) | `launch` (foreground for now, daemon-ize in phase 8), `status`, `stop`, `doctor` (config sanity check) |
| 8 | Daemon-ize | Detach, write PID file, redirect stdout/stderr to `~/.freeqcc/daemon.log` |
| 9 | Plugin (`plugin/`) | `/freeqcc launch|status|stop` commands wrap the CLI; tested via Claude Code plugin install |
| 10 | README + smoke test | HN-grade README; manual end-to-end with chad's machine + a second browser tab pretending to be someone else |
| 11 | (optional) `npm publish` | Default: install via local link; publish only if user requests |

Each phase ships as a single commit. Phases 4–6 are the load-bearing crypto/auth phases — those get
extra scrutiny.

---

## 9. Risks, open questions, calls I want from you

1. **Delegation cert minting path** — two options for v1:
   - **(a)** Shell out to existing Rust `freeq-bot-id`. Reuses correct crypto, fastest to working v1, adds
     a binary dependency users must install. *(my recommendation)*
   - **(b)** Reimplement cert minting in TS (uses SDK's signing primitives + a one-shot freeq web auth
     hop). Cleaner for end-users (single npm install), but doubles v1 effort and risks crypto drift
     with the Rust impl.
2. **Owner-gate semantics** — confirmed in §5 above (refuse once per hour, then silent). Push back if you
   want pure-silent or always-refuse.
3. **`claude -p` subprocess vs in-process Agent SDK** — sticking with subprocess for v1 (simpler, swappable).
4. **Session continuity** — v1: one persistent claude session per agent (re-`--resume`d on each DM). Works
   well for a 1:1 agent, breaks down if v2 adds multi-conversation. Acceptable for v1.
5. **Cost guards** — 60s/conversation cooldown, 30 turns/hour cap, both configurable in `~/.freeqcc/config.json`.
   Defaults aimed at "you DM me, I respond, we have a real conversation" without runaway from a stuck loop.
6. **Plugin distribution** — three install vectors, in priority order:
   1. `claude plugin install @freeq/freeqcc` (assumes Claude Code plugin marketplace exists/works)
   2. `npx @freeq/freeqcc` (works today)
   3. `git clone … && npm link` (developer path)
7. **Naming the bot's IRC nick** — default to `<owner-handle>-agent` truncated to nick rules, override
   via `--nick`. Open to other defaults.

---

## 10. What v2/v3 layer on cleanly

Worth flagging upfront because the user mentioned both:

- **Bot↔bot (v2)**: gate already takes a sender DID. Replace single-owner check with allowlist + per-peer
  cost cap. Add bot-to-bot loop detection (max-N rapid turns triggers a backoff).
- **Delegation (v2/v3)**: Owner can grant a third-party DID a *narrowed* capability set (e.g., "@friend
  may use shell, no edit") via Phase-2 `AGENT APPROVE`. Daemon respects server-side governance signals.
- **Channel mode (v2)**: Agent can be added to a channel; only owner DMs work, but channel mentions
  trigger a rate-limited public reply. Different gate, same dispatch.
- **Manifest registration (v2)**: Add `AGENT MANIFEST` after `AGENT REGISTER` so the bot shows up in
  `/api/v1/agents/manifests/{did}` discovery + the web client's identity card.

---

## 11. Calls — APPROVED 2026-05-08

- ✅ Delegation cert via existing `freeq-bot-id` Rust binary.
- ✅ Refuse non-owner DMs once per hour, silent thereafter.
- ✅ One persistent claude session per agent, `--resume`d each DM.
- ✅ Default nick `<owner-handle>-agent` is fine, **but UX must encourage a custom name**.
  The launch flow asks "What should your agent be called? (default: `<owner-handle>-agent`,
  but a name you pick is more memorable — e.g. `chad-helper`, `dev-buddy`, `sourdough-bot`)"
  with a real prompt, not a hidden default. README also pitches "name your agent" as part of
  the demo.

Building.
