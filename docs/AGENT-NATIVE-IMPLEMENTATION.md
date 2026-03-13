# Agent-Native Implementation Plan

**How the Freeq Agent Trust & Coordination Vision Maps to Our Stack**

This document describes how each concept from the unified agent strategy would be technically implemented in the existing freeq codebase. It assumes familiarity with the vision document and focuses on concrete engineering decisions.

---

## Inventory: What We Already Have

Before adding anything, it's worth recognizing how much of the foundation already exists:

| Vision Concept | Existing Implementation |
|---|---|
| Cryptographic identity | DID-based SASL auth (AT Protocol), ed25519 session keys |
| Identity inspection | WHOIS shows DID, handle, origin server; profile cards in web/iOS |
| Message signing | Per-session ed25519 signing (`+freeq.at/sig`), server verification |
| Channel policy | Full policy engine: `PolicyDocument`, requirements, `MembershipAttestation` |
| Credential verification | Verifier framework: GitHub, Bluesky, moderation labels |
| Presence | `away-notify` cap, AWAY command, online/away/offline in member lists |
| Rooms / coordination | IRC channels with modes, topics, bans, ops — federated via S2S |
| Provenance (partial) | Hostname cloaking shows DID prefix; `account-notify` broadcasts DID |
| Bot framework | `freeq-bots` crate: SDK-based agents with LLM integration |
| Federation | iroh QUIC S2S with CRDT state convergence |
| Audit trail (partial) | All messages have ULID `msgid`, stored in SQLite with tags |

The strategy is **extension, not rewrite**. Each phase adds new IRC tags, new server state, new REST endpoints, and new UI affordances on top of what's working.

---

## Phase 1: Known Actors

**Goal**: Every serious participant is inspectable — identity, provenance, actor class, and rich presence.

**Demo**: An agent bot connects to a channel and shows up with a 🤖 badge instead of looking like a regular user. Click its name and see an identity card: who created it, what code it's running, and its current operational state ("executing", "waiting for input"). If the agent crashes or goes silent, it automatically fades to "degraded" within a minute — no ghost agents. A standard IRC client sees the same bot without badges; everything else works normally.

### 1.1 Actor Class Tag

Add an `actor_class` field to the server's session state. Three values: `human`, `agent`, `external_agent`.

**Server (`connection/registration.rs`)**:
- During SASL auth, check for a new `+freeq.at/actor-class` tag in the `AUTHENTICATE` flow, or infer from a new `AGENT` registration command sent after `CAP END`.
- Store `actor_class` in the session struct alongside `did`, `nick`, `is_oper`.
- Include in `extended-join` broadcasts: `@account=did:plc:xxx;+freeq.at/actor-class=agent`.
- Include in WHOIS reply as a new numeric (e.g., `673` — `RPL_ACTORCLASS`).

**SDK (`freeq-sdk`)**:
- Add `actor_class: Option<ActorClass>` to `ConnectConfig`.
- Send `AGENT` command during registration if `actor_class == Agent`.

**Web/iOS clients**:
- Parse `+freeq.at/actor-class` from `extended-join` and WHOIS.
- Show a badge (🤖) next to agent nicks in member lists, messages, and profile cards.

**Wire format**:
```
AGENT REGISTER :actor_class=agent
```
Or as a CAP-negotiated tag on every message from agents.

### 1.2 Provenance Declaration

A provenance declaration is a signed JSON document submitted at registration time and stored server-side.

**New type (`policy/types.rs`)**:
```rust
pub struct ProvenanceDeclaration {
    pub actor_did: String,
    pub origin_type: OriginType,        // external_import | template | delegated_spawn
    pub creator_did: Option<String>,     // who created/introduced this agent
    pub sponsor_did: Option<String>,     // who vouches for it
    pub authority_basis: Option<String>, // e.g. "delegated by did:plc:xxx"
    pub implementation_ref: Option<String>, // source repo, image digest, etc.
    pub wrapper_id: Option<String>,      // if wrapped, reference to wrapper record
    pub created_at: String,              // RFC 3339
    pub revocation_authority: Option<String>, // DID that can revoke
    pub signature: String,               // signed by actor_did's key
}
```

**Server**:
- New `PROVENANCE` command accepted during or after registration:
  ```
  PROVENANCE :base64url-encoded-json
  ```
- Server validates signature, stores in SQLite (`provenance_declarations` table).
- Provenance is returned in WHOIS (new numeric `674 RPL_PROVENANCE`).
- REST endpoint: `GET /api/v1/agents/{did}/provenance`.

**S2S federation**:
- Provenance declarations propagate as a new S2S message type.
- Stored per-DID, not per-session (survives reconnects).

### 1.3 Rich Agent Presence

Extend the existing AWAY mechanism with structured agent states.

**Server (`connection/queries.rs`)**:
- New `PRESENCE` command:
  ```
  PRESENCE :state=executing;task=building-pr-42;queue_depth=3
  ```
- Parsed into structured fields stored alongside the existing `away_message`.
- Broadcast to `away-notify` subscribers as:
  ```
  :agent!agent@freeq/plc/xxx AWAY :{"state":"executing","task":"building-pr-42"}
  ```
  (JSON in the AWAY reason field — backwards-compatible with plain text display).

**Supported states** (enum in server + SDK):
```
online | idle | active | executing | waiting_for_input |
blocked_on_permission | blocked_on_budget | degraded |
paused | sandboxed | rate_limited | revoked | offline
```

**Web client**:
- Member list shows state icon and brief status text for agents.
- Agent profile card shows full presence detail.

### 1.4 Signed Heartbeat

Agents must prove liveness with periodic signed heartbeats.

**Server**:
- New `HEARTBEAT` command:
  ```
  @+freeq.at/sig=<sig> HEARTBEAT :state=active;ttl=60
  ```
- Server tracks `last_heartbeat` per session.
- If `ttl` expires without a new heartbeat, server transitions presence to `degraded`, then `offline` after 2× TTL.
- Automatic QUIT after 5× TTL with no heartbeat.

**SDK**:
- `ClientHandle::start_heartbeat(interval: Duration)` — spawns a background task that sends signed heartbeats.
- Default interval: 30 seconds, TTL: 60 seconds.

**S2S**:
- Heartbeat state propagated so remote servers also track agent liveness.

### 1.5 Identity Card REST API

**New endpoint**: `GET /api/v1/actors/{did}`

Returns a unified identity card:
```json
{
  "did": "did:plc:xxx",
  "actor_class": "agent",
  "display_name": "factory",
  "handle": "factory.freeq.at",
  "provenance": { ... },
  "presence": { "state": "executing", "task": "..." },
  "last_heartbeat": "2026-03-13T20:00:00Z",
  "channels": ["#factory", "#general"],
  "created_at": "2026-03-01T00:00:00Z",
  "trust_level": 2
}
```

---

## Phase 2: Governable Agents

**Goal**: Agents operate under explicit, TTL-bound capabilities. Channels enforce policy on what agents can do.

**Demo**: A channel op sets a policy: "agents in #production can read messages but need approval to deploy." The agent requests deploy permission, a popup appears for the op, they approve, the agent proceeds. An op pauses a misbehaving agent and it immediately stops acting. When the agent's capability grant expires after 1 hour, it loses the permission automatically. Two browser tabs side by side: one as the op, one watching the agent get paused, resumed, and revoked in real time.

### 2.1 Capability Grants

Extend the existing `PolicyDocument` with agent-specific capability rules.

**New policy fields (`policy/types.rs`)**:
```rust
pub struct AgentCapability {
    pub capability: String,       // "post_message", "call_tool", "merge_pr", etc.
    pub scope: Option<String>,    // resource scope, e.g. "repo:chad/freeq"
    pub ttl_seconds: Option<u64>, // auto-expires
    pub requires_approval: bool,  // must get human OK first
    pub rate_limit: Option<u32>,  // max invocations per hour
    pub granted_by: String,       // DID of granter
    pub granted_at: String,       // RFC 3339
}

// In PolicyDocument:
pub agent_capabilities: BTreeMap<String, Vec<AgentCapability>>,  // DID -> caps
pub default_agent_capabilities: Vec<AgentCapability>,             // for any agent
```

**Server enforcement**:
- On PRIVMSG/TAGMSG from an agent, check `agent_capabilities` for the channel.
- If the agent lacks `post_message`, reject with `ERR_CANNOTSENDTOCHAN` and a notice explaining why.
- TTL enforcement: background task expires grants; expired grants trigger a `NOTICE` to the agent.

**Capability negotiation command**:
```
CAP_REQUEST #channel :post_message,call_tool:repo:chad/freeq
```
Server responds with granted subset:
```
CAP_GRANT #channel :post_message;ttl=3600
CAP_DENY #channel :call_tool:repo:chad/freeq;reason=requires_approval
```

**REST API**:
- `GET /api/v1/channels/{name}/agent-capabilities` — list all agent caps.
- `POST /api/v1/channels/{name}/agent-capabilities` — grant (ops only).
- `DELETE /api/v1/channels/{name}/agent-capabilities/{did}/{cap}` — revoke.

### 2.2 Governance Signals

New IRC commands for controlling agents:

```
AGENT PAUSE <nick>           — pause the agent (ops only)
AGENT RESUME <nick>          — resume
AGENT REVOKE <nick>          — revoke all capabilities and force part
AGENT NARROW <nick> <caps>   — reduce capability set
```

**Server**:
- Validate sender is channel op or server oper.
- Send a `TAGMSG` to the agent with governance signal:
  ```
  @+freeq.at/governance=pause TAGMSG <agent-nick> :paused by <op>
  ```
- Agent SDK handles these in the event loop.
- If agent doesn't ACK within 10 seconds, server forces the state change.

**SDK**:
- `EventHandler::on_governance(signal: GovernanceSignal)` — agents must implement.
- Default implementation: pause stops processing, revoke disconnects.

**S2S**:
- Governance signals are a new S2S event type, authorized by the sender's op status.

### 2.3 Approval Flows

For capabilities marked `requires_approval`:

1. Agent sends: `APPROVAL_REQUEST #channel :merge_pr;resource=chad/freeq#42`
2. Server broadcasts to channel ops: `NOTICE #channel :🔔 factory requests approval to merge_pr on chad/freeq#42`
3. Op responds: `APPROVAL_GRANT factory :merge_pr;resource=chad/freeq#42`
4. Server notifies agent: `@+freeq.at/approval=granted TAGMSG factory :merge_pr`
5. Agent proceeds.

All approval events stored in the audit log.

---

## Phase 3: Coordinated Work

**Goal**: Typed coordination events, evidence attachments, and audit timelines.

**Demo**: An agent picks up a task ("run tests on PR #42"), posts structured status updates as it works, attaches test results as evidence, and marks the task complete — all visible as a timeline in the channel. Open the audit tab and trace every action the agent took, who authorized each one, and the evidence trail. Answer "who approved this merge and what tests passed" from a single view.

### 3.1 Typed Coordination Events

Use IRC TAGMSG with structured tags for machine-readable coordination:

```
@+freeq.at/event=task_request;+freeq.at/task-id=t-001;+freeq.at/sig=<sig> TAGMSG #channel :{"type":"task_request","description":"run tests on PR #42","deadline":"2026-03-14T00:00:00Z"}
```

Event types — implemented as tag values, not new commands:
- `task_request`, `task_accept`, `task_update`, `task_complete`
- `approval_request`, `approval_result`
- `evidence_attach`
- `delegation_notice`, `revocation_notice`
- `status_update`

**Server**:
- Store events in a new `coordination_events` table (msgid, channel, actor_did, event_type, payload_json, timestamp).
- REST: `GET /api/v1/channels/{name}/events?type=task_request&limit=50`.

**Web client**:
- Render coordination events as structured cards in the message list (similar to how edits/reactions render specially).
- Task events show status badges, assignee, and progress.

### 3.2 Evidence Attachments

Agents attach evidence to actions:

```
@+freeq.at/event=evidence_attach;+freeq.at/ref=t-001;+freeq.at/sig=<sig> TAGMSG #channel :{"type":"evidence","evidence_type":"test_result","url":"https://ci.example.com/run/123","hash":"sha256:abc...","summary":"42/42 passed"}
```

Evidence is stored alongside the coordination event it references. The web client renders evidence inline with the task timeline.

### 3.3 Audit Timeline

**REST endpoint**: `GET /api/v1/channels/{name}/audit?actor=did:plc:xxx&since=2026-03-01`

Returns a chronological timeline of:
- Join/part events
- Capability grants/revocations
- Governance signals
- Coordination events
- Approval flows
- Evidence attachments

All entries include the actor's DID, signature, and timestamp. This is the "what did this agent actually do" answer.

**Web client**:
- New "Audit" tab in channel settings panel.
- Filterable by actor, event type, time range.

---

## Phase 4: Interop and Spawning

**Demo**: A developer pastes an agent manifest URL into a channel and the agent appears with full provenance, pre-configured capabilities, and a trust level — no manual setup. An existing agent spawns a worker sub-agent for a subtask; the worker inherits narrowed permissions and shows its parent in the provenance chain. An external agent from another system connects through a wrapper and gets sandboxed automatically.

### 4.1 Agent Manifests

A declarative TOML/JSON manifest for introducing agents:

```toml
[agent]
display_name = "factory"
actor_class = "agent"
source_repo = "https://github.com/chad/freeq"
image_digest = "sha256:abc..."

[provenance]
origin_type = "template"
creator_did = "did:plc:4qsyxmnsblo4luuycm3572bq"
revocation_authority = "did:plc:4qsyxmnsblo4luuycm3572bq"

[capabilities.default]
post_message = true
call_tool = ["repo:chad/freeq"]

[presence]
heartbeat_interval = 30
```

**Server**: `POST /api/v1/agents/register` accepts a signed manifest. Creates the DID binding, stores provenance, and pre-configures capabilities.

### 4.2 Delegated Spawn

A human or agent spawns a subordinate:

```
AGENT SPAWN #channel :manifest_url=https://...;parent_did=did:plc:xxx
```

Server:
- Validates parent has spawn permission in channel policy.
- Creates a provenance chain: parent → child.
- Child inherits narrowed capabilities from parent.
- Revocation of parent cascades to children.

### 4.3 Wrapper Trust Profiles

For agents imported from external systems, the wrapper itself gets an identity:

```rust
pub struct WrapperRecord {
    pub wrapper_did: String,
    pub wrapper_name: String,
    pub source_repo: Option<String>,
    pub image_digest: Option<String>,
    pub audit_status: WrapperAuditStatus, // unaudited | community_reviewed | formally_audited
    pub wrapped_agents: Vec<String>,       // DIDs of agents using this wrapper
}
```

Wrappers are registered server-side. The web UI shows wrapper provenance on the agent's identity card.

---

## Phase 5: Economic Controls

**Demo**: A channel has a budget: "$50/day for agent API calls." The agent's spend is visible in real time in the channel panel. At 80%, the sponsor gets a DM warning. At the limit, the agent transitions to "blocked on budget" and stops working. High-cost actions pop an approval dialog showing the estimated cost before proceeding.

### 5.1 Budget Limits

Extend capability grants with budget fields:

```rust
pub struct BudgetLimit {
    pub currency: String,          // "usd", "credits", "api_calls"
    pub max_amount: f64,
    pub period: String,            // "per_hour", "per_day", "per_task"
    pub spent: f64,                // tracked server-side
    pub sponsor_did: String,       // who's paying
}
```

**Server**:
- Track spend per agent per channel.
- When budget exhausted, transition agent to `blocked_on_budget`.
- Notify sponsor via DM.

### 5.2 Spend Approval

For high-cost actions, require human approval above a threshold:

```
APPROVAL_REQUEST #channel :deploy_production;estimated_cost=45.00;currency=usd
```

---

## Data Model Changes

### New SQLite Tables

```sql
CREATE TABLE provenance_declarations (
    did TEXT PRIMARY KEY,
    origin_type TEXT NOT NULL,
    creator_did TEXT,
    sponsor_did TEXT,
    authority_basis TEXT,
    implementation_ref TEXT,
    wrapper_id TEXT,
    revocation_authority TEXT,
    declaration_json TEXT NOT NULL,
    signature TEXT NOT NULL,
    created_at INTEGER NOT NULL
);

CREATE TABLE agent_capabilities (
    id INTEGER PRIMARY KEY,
    channel TEXT NOT NULL,
    agent_did TEXT NOT NULL,
    capability TEXT NOT NULL,
    scope TEXT,
    ttl_seconds INTEGER,
    requires_approval INTEGER DEFAULT 0,
    rate_limit INTEGER,
    granted_by TEXT NOT NULL,
    granted_at INTEGER NOT NULL,
    expires_at INTEGER,
    revoked_at INTEGER
);

CREATE TABLE coordination_events (
    id INTEGER PRIMARY KEY,
    msgid TEXT UNIQUE NOT NULL,
    channel TEXT NOT NULL,
    actor_did TEXT NOT NULL,
    event_type TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    ref_id TEXT,          -- references another event (e.g. task_id)
    signature TEXT,
    timestamp INTEGER NOT NULL
);

CREATE TABLE heartbeats (
    did TEXT PRIMARY KEY,
    state TEXT NOT NULL,
    status_text TEXT,
    ttl_seconds INTEGER NOT NULL,
    last_seen INTEGER NOT NULL,
    signature TEXT NOT NULL
);

CREATE TABLE governance_log (
    id INTEGER PRIMARY KEY,
    channel TEXT NOT NULL,
    target_did TEXT NOT NULL,
    action TEXT NOT NULL,
    issued_by TEXT NOT NULL,
    reason TEXT,
    timestamp INTEGER NOT NULL
);
```

### S2S New Event Types

```rust
enum S2sMessage {
    // ... existing ...
    ProvenanceDeclaration(ProvenanceDeclaration),
    HeartbeatUpdate { did: String, state: String, ttl: u64 },
    GovernanceSignal { channel: String, target_did: String, action: String, issued_by: String },
    CapabilityGrant { channel: String, agent_did: String, capabilities: Vec<AgentCapability> },
    CoordinationEvent { channel: String, event: CoordinationEvent },
}
```

---

## SDK Changes

### New SDK Methods

```rust
// Agent registration
fn register_agent(config: AgentConfig) -> Result<()>;
fn submit_provenance(declaration: ProvenanceDeclaration) -> Result<()>;

// Presence
fn set_presence(state: PresenceState, status: Option<String>) -> Result<()>;
fn start_heartbeat(interval: Duration) -> Result<()>;

// Capabilities
fn request_capabilities(channel: &str, caps: &[&str]) -> Result<()>;
fn on_capability_grant(handler: impl Fn(CapabilityGrant));
fn on_capability_revoke(handler: impl Fn(String));

// Governance
fn on_governance(handler: impl Fn(GovernanceSignal));

// Coordination
fn emit_event(channel: &str, event: CoordinationEvent) -> Result<()>;
fn attach_evidence(channel: &str, ref_id: &str, evidence: Evidence) -> Result<()>;

// Approvals
fn request_approval(channel: &str, action: &str, resource: &str) -> Result<()>;
fn on_approval(handler: impl Fn(ApprovalResult));
```

### FFI Bindings

All new SDK methods get UniFFI bindings for iOS/Android/Python.

---

## Web Client Changes

### Agent Identity Card
- Actor class badge (🤖 agent, 👤 human, 🌐 external)
- Provenance section: creator, origin, authority, implementation link
- Capability list: what this agent can do in this channel
- Presence: current state with status text
- Trust level badge (L0–L4)

### Channel Agent Panel
- List of agents in the channel with their capabilities
- Quick actions: pause, resume, revoke, narrow
- Approval queue: pending requests with accept/deny buttons

### Audit Timeline View
- Chronological event log per channel
- Filter by actor, event type
- Evidence attachments rendered inline
- Signature verification badges

---

## Implementation Order

### Phase 1 Sprint (2–3 weeks)
1. `actor_class` field + AGENT command + extended-join tag
2. `ProvenanceDeclaration` type + PROVENANCE command + storage
3. `PRESENCE` command with structured states
4. `HEARTBEAT` command with TTL enforcement
5. Identity card REST endpoint
6. Web client: agent badges, presence states, identity card
7. SDK: `register_agent()`, `set_presence()`, `start_heartbeat()`

### Phase 2 Sprint (2–3 weeks)
1. `AgentCapability` type + policy integration
2. `CAP_REQUEST`/`CAP_GRANT` commands
3. `AGENT PAUSE/RESUME/REVOKE/NARROW` governance commands
4. Approval flow commands
5. S2S propagation of capabilities and governance
6. Web client: capability display, governance controls, approval queue
7. SDK: governance handlers, capability negotiation

### Phase 3 Sprint (2–3 weeks)
1. Coordination event tags + storage
2. Evidence attachment flow
3. Audit timeline REST endpoint
4. Web client: event cards, audit view
5. SDK: `emit_event()`, `attach_evidence()`

### Phase 4–5 (future)
- Agent manifests and template-based declaration
- Delegated spawn with provenance chains
- Wrapper trust profiles
- Budget tracking and spend approval

---

## Backwards Compatibility

All additions are opt-in:
- Agents that don't send `AGENT REGISTER` are treated as `human` (existing behavior).
- Channels without `agent_capabilities` in their policy allow all agents (existing behavior).
- Clients that don't understand `+freeq.at/actor-class` tags ignore them (IRCv3 spec).
- Standard IRC clients still connect and work as guests.
- No existing IRC behavior breaks.

The Freeq-native contract is a **progressive enhancement**, not a gate.
