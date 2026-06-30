# RFC v0.2: `freeq.at/act` — stateful, signed, addressed actions for IRCv3

*(with `handoff` as the first action kind)*

**Status:** draft / request for comments · **Author:** Chad Fowler (freeq) · **Audience:** agent-coordination builders, IRCv3, AT Protocol

This is a casual RFC. Poke holes in it.

> **What changed since v0.1** (thanks to review feedback): the primitive is no longer a bespoke "handoff inbox." It's a small **stateful-action substrate** — handoff is its *first verb-set*, with approvals, capability-grants, votes, etc. as future kinds on the same rails. There's **no new store**: actions ride the message/DM/channel layer freeq already has. The signing claims are corrected — handoffs need a **new canonical** (the existing one assumes a message body and a wall-clock timestamp), and cross-server non-repudiation is called out honestly as **not yet real**.

---

## TL;DR

A typed, addressed, signed, **stateful** message: an action with a lifecycle (`offer → accept/decline → progress → complete/fail/cancel`), distinguished from chat by an `act` kind tag and correlated by a ULID. Its state is validated server-side and materialized into a queryable view; the signed message log stays the source of truth.

`handoff` — transferring a unit of work that survives the recipient being offline — is the first kind. The same substrate carries `approval`, `grant`, and friends later. If those reuse it without reinventing, the shape is right.

## Motivation

AI agents call tools fine but coordinate badly *across time*: when an agent goes offline, in-flight work and context evaporate. The common answer (e.g. AIRC) is a separate HTTP registry + inbox just for agents.

freeq already has the hard parts of that — DID identity, per-message signing, msgid ULIDs, replay-on-connect (CHATHISTORY / DM history), server-to-server federation. So the missing piece isn't infrastructure; it's **semantics on top of the existing message layer**. Model it natively as an IRCv3 client-tag extension and you get durable agent coordination *and* the ability to escalate an action into a live channel or voice room when async needs to become a conversation — something a pure HTTP inbox can't do.

## The reframing: an action substrate, not a handoff inbox

Once a handoff is a typed, signed, stateful action on a message, it stops looking unique. Reactions/edits/deletes/pins/replies are already "actions on a message." Approvals (`approve/deny` a deploy), capability grants (`grant/pause/revoke`), votes, acks, attestations are all `offer→resolve` state machines. They all want the same three things:

1. a **verb-tagged typed message**,
2. a **transition validator** (who may move it to which state),
3. a **materialized view** of current state.

So the wire uses generic `act-*` tags with the kind as a value. `handoff` is one kind:

```
@+freeq.at/act=handoff;+freeq.at/act-verb=offer;+freeq.at/act-id=01JABC…;
 +freeq.at/act-to=did:plc:scholar;+freeq.at/act-title=Cite 3 sources on X;
 +freeq.at/act-ctx=freeq:blob/cap/abc;+freeq.at/act-ctx-h=sha256:9f…;
 +freeq.at/act-caps=freeq.at/web-search;+freeq.at/act-deadline=1788000000;
 +freeq.at/sig=ed25519:… TAGMSG #ops
```

…and a deploy approval is the *same substrate*, different kind:

```
@+freeq.at/act=approval;+freeq.at/act-verb=request;+freeq.at/act-id=01KDEF…;
 +freeq.at/act-to=did:plc:opslead;+freeq.at/act-title=Deploy factory-bot v12;
 +freeq.at/act-ctx-h=sha256:1a…;+freeq.at/sig=… TAGMSG #ops
```

Same `act-id` correlation key, same `act-ref` to link replies, same validator mechanics, same view, same REST shape. The kind is a row in a registry, not a subsystem.

**Build discipline:** implement `handoff` *concretely* and factor the substrate out from it — do **not** design an abstract framework first (that way lies the over-engineered version). Acceptance test: when `approval`/`grant` land — and they will — do they reuse this or reinvent it? Reuse obvious ⇒ shape is right. "handoff" welded into storage/wire ⇒ it isn't.

> **Important caveat on generality:** the substrate generalizes the *plumbing* (wire, validator mechanics, view, REST), **not the policy**. Each `kind` must ship its own **transition table + authorization rules** as a first-class artifact — those differ per kind and are the actual hard design. "handoff is just a verb-set" is true for the plumbing and undersells the policy.

## Two orthogonal axes

DM-vs-channel conflates two independent knobs. Keep them separate:

- **Assignment** — *who does it.*
  - **directed**: `act-to=did:plc:bob` → starts assigned to Bob.
  - **open / claimable**: `act-to=#swarm` + `act-caps=…` → starts unassigned; any capable agent `claim`s it; first valid claim wins.
- **Visibility** — *where the event is posted.*
  - **channel**: visible to the room, logged in channel history.
  - **DM**: private to two DIDs.

These compose. A *directed* action can still be posted **in-channel** (`act-to=<did>` on a `TAGMSG #ops`) so it's assigned to one agent but everyone watches it happen.

**Channel is the default for multi-agent**, because it gives observability + logging for free (channel history already persists the whole `offer→complete` stream), enables an orchestrator agent to watch/reassign/escalate live, enables claimable work queues, and sidesteps E2EE entirely (channel `act-*` tags are server-visible, so the validator/view just work). DM is the private-directed special case.

## Lifecycle & the transition validator

`handoff` verb-set and its rules:

| verb | who may send | precondition |
|---|---|---|
| `offer` | anyone | mints `act-id` |
| `accept` | the addressed DID (directed) | state = offered, before deadline |
| `claim` | any DID matching `act-caps` (open) | state = open; **first valid wins** |
| `decline` | the addressed DID | state = offered |
| `progress` | the assignee | state = assigned |
| `complete` | the assignee | state = assigned |
| `fail` | the assignee | state = assigned |
| `cancel` | the offerer | state = offered/assigned, before complete |

The validator, on each incoming event: look up prior events for `act-id`, check the verb is a legal transition **and** the sender is authorized, then store + route it like any message. Reject otherwise.

### Claim semantics (open/claimable)

`claim` is just a verb with one extra rule: **first valid claim wins, atomically.** The action's **home server** is the single authority — it flips `open → assigned(did)` on the view and rejects later claims. Locally this is straightforward; across federation it requires the home server as the serialization point, which depends on the trust/sig work below. So: **local claimable first; cross-server claimable only after federated sig + key distribution land.**

## Storage & delivery: ride what already exists

There is **no new inbox/store.** Delivery and durability come from the message layer freeq already has:

- A **channel** action is in channel history; replayed via CHATHISTORY on reconnect.
- A **directed** action rides the DM store (keyed by `canonical_dm_key`), replayed on reconnect.
- An **open** action lives in the target channel's history, claimable while non-terminal.

Net-new code is two small things, identical whether it's handoff/approval/grant:

1. **A transition validator** (above).
2. **A materialized view** — a read-side index (`act-id → latest state, assignee, caps, deadline`) so you can answer "actions assigned to me" / "open actions I can claim" without scanning the log. **The signed message log is the source of truth; the view is rebuildable from it and never authoritative.**

> **E2EE note:** for DM-delivered actions, the actionable state lives in the **cleartext `act-*` tags** (which the server validates and indexes); only freeform `progress` prose would sit in an encrypted body. Channel actions are server-visible by definition. So validation never depends on reading an encrypted payload.

## Context (`act-ctx` / `act-ctx-h`)

The real axis is **not payload size — it's whether the bytes live somewhere freeq commits to keeping.** A signed action that points at a rotted URL has lost the auditability the signature was for.

- **Default: freeq-hosted** context (capability URL), lifecycle tied to the action's retention. Only setup where the audit guarantee holds.
- **External refs** (gist, S3, an AT-Proto record on another PDS) are allowed but **explicitly best-effort**: ref dies, guarantee dies — caller's call.
- **The signature always covers a content hash** (`act-ctx-h`), so whatever you fetch later is checkable against what was signed — tamper-evidence wherever the bytes live, and the only integrity check you get at all for external refs.
- Tiny payloads may be inlined as a convenience; that's not a durability story, just an optimization.

## Signing & canonicalization

⚠️ This does **not** reuse freeq's current PRIVMSG signing. Today's canonical is `{sender_did}\0{target}\0{text}\0{timestamp}` with `timestamp` minted at send and never stored — it assumes a message body and a wall-clock field, neither of which a body-less `TAGMSG` action has. So this is a **new signing model**, deliberately designed to survive federation:

- **Canonical:** deterministic JSON (JCS / RFC 8785) over an explicit, fixed field set: `act`, `act-verb`, `act-id`, `act-from`, `act-to`, `act-title`, `act-ctx-h` (the hash, not raw context), `act-caps`, `act-deadline`, `act-ref`.
- **Sign over the ULID (`act-id`), not a wall-clock timestamp.** PRIVMSG sigs die across S2S because the receiver re-mints `timestamp`. A ULID embeds its own creation time, is immutable, and already travels as a first-class tag — signing over it kills the regenerated-timestamp failure mode entirely. (Structural advantage these actions have that PRIVMSG didn't.)
- **S2S relays the signed tags verbatim** (`act-from`, `act-id`, `sig`, plus the canonical fields) and the **receiver rebuilds the canonical from them — never re-mints.** Since DID and ULID are both already tags, this is far more achievable than retrofitting PRIVMSG.

## Trust & non-repudiation — today vs goal

Stated plainly so nobody over-reads the guarantee:

- **Canonicalization makes the signature *reconstructable*, not *trustless*.**
- The **DID↔signing-key binding is unattested today.** `MSGSIG` registers a bare ed25519 pubkey, and the server is the one publishing per-DID keys (`/api/v1/signing-keys/{did}` is local, server-controlled). A malicious server could publish its own key as yours and forge.
- **Net: non-repudiation holds against an *honest origin server*, not a malicious one** — until key distribution is server-independent.
- **Goal / path to real E2E non-repudiation:** anchor the signing key in the **DID document** (attest the ed25519 key via the AT-Proto identity — did:plc/did:web), so any party verifies the key independently of the freeq server. This is the same root-of-trust gap the broader "identity = DID, never the server's say-so" work cares about. It's a prerequisite for trustworthy cross-server claimable queues.

This RFC specifies the wire/validator/view; it **flags** the trust gap and does not pretend to close it.

## Capabilities (`act-caps`)

Freeform, and **the server never interprets them** (it can't verify an agent really does `web-search` anyway). Caps are a self-declared hint for the recipient/router/claimer to self-select — store, filter, route, never interpret. Fuzzy/semantic matching belongs in the agents.

- No protocol-baked capability registry (it'd be stale in months and a governance chore).
- The one convention worth fixing now is **namespacing** — reverse-DNS / AT-style (`freeq.at/web-search`) — with meanings converging socially. Reserve well-known names later if needed; starting loose costs nothing.

## Liveness, backpressure, retention

Modeling actions as messages in the existing store dissolves most of this:

- **Flooding** — offers are messages, already under freeq's flood throttle + per-IP/connection limits. No new quota machinery.
- **Storage growth** — same message/DM/channel store under existing retention. The view stays small by construction (indexes only non-terminal actions) and is rebuildable.
- **The one genuinely new policy is liveness, not storage:** an action stuck in `accepted/progress` that never reaches a terminal state. `act-deadline` covers *offer* expiry; nothing clocks an abandoned in-progress task. So a small **sweep auto-expires non-terminal actions past a TTL** (mark `fail`/`expired`), acting on the **view**, not storage.

## Federation

Action events propagate over S2S like any tagged message, preserving `act-id`, the canonical fields, and `sig`. A directed action to a DID on a remote server routes to that server's delivery; the **home server owns delivery, replay, and (for open actions) claim serialization.** Receivers rebuild and check the canonical from the relayed tags (see Signing). Cross-server claimable waits on the trust work.

## REST query interface (over the view)

A query surface over the materialized view — *not* a parallel table that owns data — so non-IRC agents and interop bridges can use it:

- `GET /api/v1/actions?kind=&to=&state=&caps=` — my inbox / claimable queue
- `GET /api/v1/actions/{act-id}` — current state + context ref + event log
- `POST /api/v1/actions` — emit an `offer`/`request`
- `POST /api/v1/actions/{act-id}/{verb}` — a transition

This shape maps cleanly onto AIRC-style `POST /messages` + payloads, so an interop bridge is a thin adapter.

## Orchestration pattern (why channel-default matters)

Put a supervisor/orchestrator agent in the channel. It watches the live `act-*` event stream and can reassign a stalled task, enforce deadlines, fan work out, or escalate an open queue. The channel *is* the coordination bus; handoffs become an **observable, logged, reassignable** stream rather than point-to-point messages. CHATHISTORY gives you the audit log for free.

## What's actually new to build

1. The `act-*` tag set + `freeq.at/act` CAP + TAGMSG handling.
2. A **transition validator** (per-kind transition table + authz).
3. A **materialized view** + the REST query interface + reconnect replay (reusing CHATHISTORY/DM replay).
4. A **liveness sweep** for non-terminal actions past TTL.
5. The **new canonical + sign-over-ULID** signing path, and S2S relaying the signed tags verbatim.

Everything else (delivery, durability, identity, msgid, flood limits, federation transport) is reuse.

## Non-goals

- Not a workflow engine / DAG executor — it's a transfer + state primitive; orchestration lives above it.
- Not a replacement for chat — actions are *tracked* units, not conversation.
- Not re-doing identity — it rides whatever identity the server already verifies (AT-Proto DIDs).
- Not (yet) solving server-independent key distribution — flagged, not closed.

## Open questions

- **Substrate now, or handoff-first then factor?** (Hunch: handoff-first, factor out — but get the `act-*` shape right so approvals/grants reuse it.)
- **Per-kind authz spec format** — how do we declare each kind's transition table + rules so it's reviewable and not ad-hoc?
- **Claim fairness** beyond first-wins — bidding, priority, capability scoring? Or keep dumb and let orchestrators decide?
- **External context refs** — allow AT-Proto records as a first-class (best-effort) ref type, or discourage entirely?
- **Canonical field set** — is the list above complete? Versioning the canonical.
- **WG venue** — keep `+freeq.at/*` until the trust pieces are solid, then pitch IRCv3 WG? (Design the wire to be de-vendorable now regardless.)

---

*Feedback welcome — comment on the gist, or find me on freeq (`irc.freeq.at`) / Bluesky.*
