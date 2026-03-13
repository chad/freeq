# Phase 2: Governable Agents — Detailed Implementation Plan

**Goal**: Agents operate under explicit, TTL-bound capabilities. Channels enforce policy on what agents can do. Humans can pause, resume, and revoke agents in real time.

**Demo**: The factory bot joins `#production`, a channel with a policy requiring approval for deploys. The bot builds a project and requests deploy approval. A channel op sees the request in the web client, clicks "Approve," and the bot deploys. The op then pauses the bot — it immediately stops and shows "paused" in the member list. The op resumes it. Meanwhile, a viewer on irssi sees all of this as regular chat messages ("🔔 factory requests approval to deploy", "✓ deploy approved by chad", "⏸ factory paused by chad").

---

## 1. Capability Grants

### Policy Extension

**File**: `freeq-server/src/policy/types.rs`

Add to `PolicyDocument`:
```rust
/// Agent-specific capability rules.
/// DID → list of capabilities. Empty means "use default_agent_capabilities".
#[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
pub agent_capabilities: BTreeMap<String, Vec<AgentCapability>>,

/// Default capabilities for any agent without explicit grants.
#[serde(default, skip_serializing_if = "Vec::is_empty")]
pub default_agent_capabilities: Vec<AgentCapability>,
```

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AgentCapability {
    /// Capability name: "post_message", "deploy", "call_tool", "merge_pr", etc.
    pub capability: String,

    /// Resource scope, e.g. "repo:chad/freeq", "*".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,

    /// TTL in seconds. Capability expires after this. 0 = no expiry.
    #[serde(default)]
    pub ttl_seconds: u64,

    /// If true, agent must request approval before exercising this capability.
    #[serde(default)]
    pub requires_approval: bool,

    /// Max invocations per hour. 0 = unlimited.
    #[serde(default)]
    pub rate_limit: u32,

    /// DID of the granter.
    pub granted_by: String,

    /// When this grant was issued (RFC 3339).
    pub granted_at: String,
}
```

### Server Enforcement

**File**: `freeq-server/src/connection/messaging.rs`

Before processing PRIVMSG/TAGMSG from an agent in a channel:
```rust
if session.actor_class == ActorClass::Agent {
    let channel_policy = state.policy_engine.get_policy(&channel_name);
    if let Some(policy) = channel_policy {
        let caps = policy.agent_capabilities.get(&session.did)
            .or_else(|| Some(&policy.default_agent_capabilities));
        if !has_capability(caps, "post_message") {
            send_err(session, ERR_CANNOTSENDTOCHAN, &channel_name,
                "Agent lacks post_message capability in this channel");
            return;
        }
    }
}
```

### Capability Grant Storage

**New SQLite table**:
```sql
CREATE TABLE IF NOT EXISTS agent_capability_grants (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    channel TEXT NOT NULL,
    agent_did TEXT NOT NULL,
    capability TEXT NOT NULL,
    scope TEXT,
    ttl_seconds INTEGER DEFAULT 0,
    requires_approval INTEGER DEFAULT 0,
    rate_limit INTEGER DEFAULT 0,
    granted_by TEXT NOT NULL,
    granted_at INTEGER NOT NULL,
    expires_at INTEGER,
    revoked_at INTEGER,
    UNIQUE(channel, agent_did, capability, scope)
);
```

### TTL Expiry

**File**: `freeq-server/src/server.rs`

Background task (runs every 30 seconds):
```rust
// Expire TTL-bound capabilities
let expired = db.query("SELECT * FROM agent_capability_grants WHERE expires_at IS NOT NULL AND expires_at < ?", now);
for grant in expired {
    db.execute("UPDATE agent_capability_grants SET revoked_at = ? WHERE id = ?", [now, grant.id]);
    // Notify the agent
    send_notice(grant.agent_did, &format!(
        "⏰ Capability '{}' in {} has expired", grant.capability, grant.channel
    ));
}
```

### REST API

**File**: `freeq-server/src/web.rs`

```
GET  /api/v1/channels/{name}/agent-capabilities
     → [{ did, capability, scope, ttl, requires_approval, granted_by, expires_at }]

POST /api/v1/channels/{name}/agent-capabilities
     Body: { agent_did, capability, scope?, ttl_seconds?, requires_approval? }
     Auth: must be channel op
     → { grant_id, expires_at }

DELETE /api/v1/channels/{name}/agent-capabilities/{grant_id}
     Auth: must be channel op or granter
     → 204
```

### SDK Changes

**File**: `freeq-sdk/src/client.rs`

```rust
/// Request capabilities in a channel. Server responds with CAP_GRANT or CAP_DENY.
pub async fn request_capabilities(&self, channel: &str, caps: &[&str]) -> Result<Vec<CapabilityGrant>>;

/// Callback when capabilities are granted.
pub fn on_capability_grant(&self, handler: impl Fn(CapabilityGrant) + Send + 'static);

/// Callback when capabilities are revoked or expire.
pub fn on_capability_revoke(&self, handler: impl Fn(String) + Send + 'static);
```

### Wire Format

Agent requests capabilities:
```
AGENT CAP_REQUEST #production :deploy,post_message
```

Server grants (or denies):
```
:server NOTICE factory :CAP_GRANT #production :post_message;ttl=3600
:server NOTICE factory :CAP_DENY #production :deploy;reason=requires_approval
```

---

## 2. Governance Signals

### Server Changes

**File**: `freeq-server/src/connection/mod.rs`

New command handling for `AGENT` subcommands:

```
AGENT PAUSE <nick> [reason]
AGENT RESUME <nick>
AGENT REVOKE <nick> [reason]
AGENT NARROW <nick> <capability_to_remove>
```

**Authorization**: sender must be a channel op in a shared channel, or a server oper.

**Processing** (`connection/agent_cmd.rs` — new file):

```rust
pub async fn handle_agent_command(state: &ServerState, session: &Session, cmd: &str, args: &[&str]) {
    match cmd {
        "PAUSE" => {
            let target_nick = args[0];
            let reason = args.get(1..).map(|a| a.join(" "));

            // Verify sender is op in a shared channel
            verify_op_authority(state, session, target_nick)?;

            // Send governance signal to the agent
            let target_session = state.nick_to_session(target_nick)?;
            send_to_session(target_session, &format!(
                "@+freeq.at/governance=pause TAGMSG {} :paused by {}{}",
                target_nick, session.nick,
                reason.map(|r| format!(" ({})", r)).unwrap_or_default()
            ));

            // Also send a human-readable NOTICE to shared channels
            for channel in shared_channels(state, session, target_session) {
                broadcast_to_channel(state, &channel, &format!(
                    ":server NOTICE {} :⏸ {} paused by {}{}",
                    channel, target_nick, session.nick,
                    reason.map(|r| format!(": {}", r)).unwrap_or_default()
                ));
            }

            // Log governance action
            db.execute("INSERT INTO governance_log (channel, target_did, action, issued_by, reason, timestamp) VALUES (?, ?, 'pause', ?, ?, ?)",
                [...]);

            // If agent doesn't ACK within 10 seconds, force the state
            spawn_governance_timeout(target_session, GovernanceAction::Pause, Duration::from_secs(10));
        }
        "RESUME" => { /* similar */ }
        "REVOKE" => {
            // Revoke all capabilities, force PART from all channels
            revoke_all_capabilities(state, target_session);
            force_part_all_channels(state, target_session, &format!("Revoked by {}", session.nick));
        }
        "NARROW" => {
            // Remove a specific capability
            let cap_name = args[1];
            revoke_capability(state, target_session, cap_name);
        }
        _ => {}
    }
}
```

### SDK Changes

**File**: `freeq-sdk/src/client.rs`

```rust
/// Register a handler for governance signals.
pub fn on_governance(&self, handler: impl Fn(GovernanceSignal) + Send + 'static);

#[derive(Debug, Clone)]
pub enum GovernanceSignal {
    Pause { by: String, reason: Option<String> },
    Resume { by: String },
    Revoke { by: String, reason: Option<String> },
    NarrowCapability { capability: String, by: String },
}
```

In the SDK event loop, parse `+freeq.at/governance` tags from TAGMSG and dispatch to the handler.

Default implementation:
- **Pause**: stop processing new commands, hold state, update presence to `Paused`.
- **Resume**: resume processing, update presence.
- **Revoke**: gracefully disconnect.
- **Narrow**: remove capability from local cap list.

### Bot Changes (Demo Code)

**File**: `freeq-bots/src/main.rs`

```rust
handle.on_governance(|signal| {
    match signal {
        GovernanceSignal::Pause { by, .. } => {
            tracing::info!("Paused by {by}");
            factory.pause();
            handle.set_presence(PresenceState::Paused, Some(&format!("paused by {by}")), None);
        }
        GovernanceSignal::Resume { by } => {
            tracing::info!("Resumed by {by}");
            factory.resume();
            handle.set_presence(PresenceState::Active, Some("resumed"), None);
        }
        GovernanceSignal::Revoke { by, .. } => {
            tracing::info!("Revoked by {by}");
            // Graceful shutdown
            std::process::exit(0);
        }
        _ => {}
    }
});
```

The factory already has `Phase::Paused` — wire it up to the governance signal.

### Governance Log Table

```sql
CREATE TABLE IF NOT EXISTS governance_log (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    channel TEXT,
    target_did TEXT NOT NULL,
    action TEXT NOT NULL,    -- pause, resume, revoke, narrow
    issued_by TEXT NOT NULL,
    reason TEXT,
    timestamp INTEGER NOT NULL
);
```

### Web Client Changes

**File**: `freeq-app/src/components/MemberList.tsx`

Right-click context menu on agent members:
- ⏸ Pause agent
- ▶ Resume agent
- ❌ Revoke agent
- 🔧 Manage capabilities...

These fire the `AGENT` commands via the IRC connection.

**New file**: `freeq-app/src/components/AgentCapabilityPanel.tsx`

Modal showing:
- Current capabilities for this agent in this channel
- TTL remaining (countdown timer)
- Toggle `requires_approval` per capability
- Add/remove capabilities
- Revoke all button

---

## 3. Approval Flows

### Server Changes

**File**: `freeq-server/src/connection/agent_cmd.rs`

New commands:
```
APPROVAL_REQUEST #channel :deploy;resource=build-landing-page
APPROVAL_GRANT <nick> :deploy;resource=build-landing-page
APPROVAL_DENY <nick> :deploy;reason=not ready yet
```

Processing:

```rust
"APPROVAL_REQUEST" => {
    // Store pending approval
    let approval_id = ulid::new();
    db.execute("INSERT INTO pending_approvals (id, channel, agent_did, capability, resource, requested_at) VALUES (?, ?, ?, ?, ?, ?)",
        [approval_id, channel, session.did, capability, resource, now]);

    // Notify channel ops
    broadcast_to_channel_ops(state, &channel, &format!(
        ":server NOTICE {} :🔔 {} requests approval for '{}' on {}. Use: APPROVAL_GRANT {} :{}",
        channel, session.nick, capability, resource, session.nick, capability
    ));

    // Also send as a structured TAGMSG for rich clients
    broadcast_to_channel(state, &channel, &format!(
        "@+freeq.at/event=approval_request;+freeq.at/approval-id={};+freeq.at/capability={} TAGMSG {} :{}",
        approval_id, capability, channel, resource
    ));
}

"APPROVAL_GRANT" => {
    // Verify sender is op
    verify_op(state, session, channel)?;

    // Mark approval granted
    db.execute("UPDATE pending_approvals SET granted_by = ?, granted_at = ? WHERE ...");

    // Notify the agent
    send_to_nick(state, target_nick, &format!(
        "@+freeq.at/governance=approval_granted;+freeq.at/capability={} TAGMSG {} :approved by {}",
        capability, target_nick, session.nick
    ));

    // Notify channel
    broadcast_to_channel(state, &channel, &format!(
        ":server NOTICE {} :✅ {} approved '{}' for {}",
        channel, session.nick, capability, target_nick
    ));
}
```

### Pending Approvals Table

```sql
CREATE TABLE IF NOT EXISTS pending_approvals (
    id TEXT PRIMARY KEY,
    channel TEXT NOT NULL,
    agent_did TEXT NOT NULL,
    capability TEXT NOT NULL,
    resource TEXT,
    requested_at INTEGER NOT NULL,
    granted_by TEXT,
    granted_at INTEGER,
    denied_by TEXT,
    denied_at INTEGER,
    deny_reason TEXT,
    expires_at INTEGER  -- auto-expire after 1 hour if not acted on
);
```

### SDK Changes

```rust
/// Request approval for a capability that requires it.
pub async fn request_approval(&self, channel: &str, capability: &str, resource: &str) -> Result<()>;

/// Callback when approval is granted.
pub fn on_approval(&self, handler: impl Fn(ApprovalResult) + Send + 'static);

#[derive(Debug, Clone)]
pub enum ApprovalResult {
    Granted { capability: String, by: String },
    Denied { capability: String, by: String, reason: Option<String> },
    Expired { capability: String },
}
```

### Bot Changes (Demo Code)

**File**: `freeq-bots/src/factory/orchestrator.rs`

In the deploy phase:
```rust
Phase::Deploying => {
    // Check if deploy requires approval in this channel
    let caps = handle.get_capabilities(&channel).await?;
    let deploy_cap = caps.iter().find(|c| c.capability == "deploy");

    if let Some(cap) = deploy_cap {
        if cap.requires_approval {
            handle.set_presence(PresenceState::BlockedOnPermission,
                Some("awaiting deploy approval"), Some(&project_name)).await?;

            handle.request_approval(&channel, "deploy", &project_name).await?;

            // Wait for approval (with timeout)
            let result = handle.wait_for_approval("deploy", Duration::from_secs(300)).await?;
            match result {
                ApprovalResult::Granted { .. } => {
                    handle.set_presence(PresenceState::Executing,
                        Some("deploying"), Some(&project_name)).await?;
                    // proceed with deploy
                }
                ApprovalResult::Denied { reason, .. } => {
                    output::status(handle, &channel, &deployer(),
                        "❌", &format!("Deploy denied: {}", reason.unwrap_or_default())).await?;
                    return Ok(());
                }
                ApprovalResult::Expired { .. } => {
                    output::status(handle, &channel, &deployer(),
                        "⏰", "Deploy approval timed out").await?;
                    return Ok(());
                }
            }
        }
    }

    // Deploy
    tools::miren_deploy(&workspace).await?;
}
```

### Web Client Changes

**New file**: `freeq-app/src/components/ApprovalQueue.tsx`

A panel (accessible from channel settings or a notification badge) showing pending approval requests:

```
┌──────────────────────────────────────────────┐
│ 🔔 Pending Approvals                         │
├──────────────────────────────────────────────┤
│ 🤖 factory requests: deploy                  │
│    Resource: build-landing-page               │
│    Requested: 2 minutes ago                   │
│    [✅ Approve]  [❌ Deny]                    │
└──────────────────────────────────────────────┘
```

Clicking Approve/Deny sends the corresponding `APPROVAL_GRANT`/`APPROVAL_DENY` command.

### REST API

```
GET  /api/v1/channels/{name}/approvals?status=pending
POST /api/v1/channels/{name}/approvals/{id}/grant   (auth: channel op)
POST /api/v1/channels/{name}/approvals/{id}/deny    (auth: channel op)
```

---

## 4. S2S Federation

### New S2S Messages

**File**: `freeq-server/src/s2s.rs`

```rust
S2sMessage::CapabilityGrant {
    channel: String,
    agent_did: String,
    capabilities: Vec<AgentCapability>,
}
S2sMessage::GovernanceSignal {
    channel: String,
    target_did: String,
    action: String,      // "pause", "resume", "revoke", "narrow"
    issued_by: String,
    reason: Option<String>,
}
S2sMessage::ApprovalRequest {
    channel: String,
    agent_did: String,
    capability: String,
    resource: Option<String>,
    approval_id: String,
}
S2sMessage::ApprovalResult {
    approval_id: String,
    granted: bool,
    by: String,
    reason: Option<String>,
}
```

**Authorization**: receiving server verifies the sender has op authority for governance signals (same pattern as existing S2S kick/mode authorization).

---

## Demo Script

### Setup

1. **Server** with a policy-gated channel:
   ```bash
   # Create #production with a policy that requires deploy approval
   curl -X POST http://localhost:8080/api/v1/channels/%23production/policy \
     -H "Content-Type: application/json" \
     -d '{
       "default_agent_capabilities": [
         {"capability": "post_message", "granted_by": "did:plc:xxx", "granted_at": "2026-03-11T00:00:00Z"},
         {"capability": "deploy", "requires_approval": true, "granted_by": "did:plc:xxx", "granted_at": "2026-03-11T00:00:00Z"}
       ]
     }'
   ```

2. **Factory bot** running with Phase 1 + Phase 2 changes.

3. **Web client** open as a channel op.

4. **irssi** connected as a guest.

### Steps

1. **Bot joins #production** — web client shows it with capabilities listed: "post_message ✅, deploy ⏳ (requires approval)".

2. **User triggers a build**: `factory: build a landing page for a coffee shop`.

3. **Bot works through phases** — presence updates visible in web client identity card.

4. **Bot reaches deploy phase** — presence changes to "blocked on permission: awaiting deploy approval". A notification badge appears in the web client's approval queue.

5. **Op approves** — clicks "Approve" in the approval queue. Bot transitions to "executing: deploying". Deploy completes. irssi user sees: "✅ chad approved 'deploy' for factory" and "🚀 Deployed to https://coffee-shop.miren.dev".

6. **Op pauses the bot** — right-click → Pause. Bot immediately shows "paused" in member list. irssi sees "⏸ factory paused by chad".

7. **Op resumes** — bot continues from where it left off.

8. **Op revokes** — bot gracefully disconnects from all channels.

### What This Proves
- Agents have explicit, inspectable permissions per channel.
- Risky actions require human approval before execution.
- Humans can intervene in real time (pause/resume/revoke).
- All governance actions are visible to everyone in the channel.
- TTL prevents stale permissions from persisting.
- Legacy clients see everything as human-readable notices.

---

## External Demo Dependencies

### Policy Configuration Tool

For the demo, we need a way to set channel policies with agent capabilities. Options:
1. **REST API** (implemented above) — curl commands or a simple admin page.
2. **IRC commands** — extend the existing `/POLICY` command to include agent capability rules.
3. **Web admin panel** — new page in freeq-app for channel ops to configure agent rules visually.

Recommendation: REST API first (demo via curl), then web admin panel.

### Factory Bot with Approval Awareness

The existing factory bot needs to:
1. Register as an agent (Phase 1).
2. Request capabilities on channel join.
3. Check `requires_approval` before deploying.
4. Handle `wait_for_approval` with timeout.
5. Respond to pause/resume/revoke governance signals.

This is ~100 lines of changes to the existing factory orchestrator.
