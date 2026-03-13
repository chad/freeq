# Phase 4: Interop and Spawning — Detailed Implementation Plan

**Goal**: Standardize how agents are introduced, allow agents to spawn sub-agents, and bridge external agents into Freeq with full provenance.

**Demo**: A developer pastes a manifest URL into `#factory` and a new "auditor" agent appears — fully configured with provenance, capabilities, and heartbeat, without any manual setup. The factory bot spawns a short-lived "qa-worker" sub-agent to run tests in parallel; the worker shows the factory as its parent in the provenance chain. An MCP-connected external agent joins `#support` through a Freeq wrapper; the wrapper translates its capabilities and sandboxes it automatically. All three patterns are visible in the web client's identity cards.

---

## 1. Agent Manifests

### Manifest Format

A declarative TOML file that describes an agent's identity, provenance, and default capabilities.

**New file**: `freeq-server/src/manifest.rs`

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentManifest {
    pub agent: AgentInfo,
    pub provenance: ManifestProvenance,
    pub capabilities: ManifestCapabilities,
    pub presence: ManifestPresence,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentInfo {
    pub display_name: String,
    pub actor_class: ActorClass,       // always "agent"
    pub description: Option<String>,
    pub source_repo: Option<String>,
    pub image_digest: Option<String>,
    pub version: Option<String>,
    pub documentation_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestProvenance {
    pub origin_type: OriginType,
    pub creator_did: String,
    pub revocation_authority: String,
    pub authority_basis: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestCapabilities {
    /// Capabilities the agent requests by default when joining any channel.
    pub default: Vec<String>,
    /// Channel-specific capability overrides.
    #[serde(default)]
    pub channels: BTreeMap<String, Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestPresence {
    pub heartbeat_interval_seconds: u64,
}
```

### Example Manifest

```toml
# auditor.freeq.toml

[agent]
display_name = "auditor"
actor_class = "agent"
description = "Architecture auditor — analyzes repos and provides recommendations"
source_repo = "https://github.com/chad/freeq"
version = "0.1.0"
documentation_url = "https://freeq.at/docs/bots/auditor"

[provenance]
origin_type = "template"
creator_did = "did:plc:4qsyxmnsblo4luuycm3572bq"
revocation_authority = "did:plc:4qsyxmnsblo4luuycm3572bq"
authority_basis = "Operated by freeq core team"

[capabilities]
default = ["post_message", "read_channel"]

[capabilities.channels]
"#factory" = ["post_message", "read_channel", "call_tool"]

[presence]
heartbeat_interval_seconds = 30
```

### Server Changes

**File**: `freeq-server/src/web.rs`

New endpoint:
```
POST /api/v1/agents/register
Body: { manifest_url: "https://example.com/auditor.freeq.toml" }
  or: { manifest: <inline TOML or JSON> }
Auth: must be server oper or have agent_admin capability

→ {
    agent_did: "did:plc:...",
    registered: true,
    capabilities_granted: ["post_message", "read_channel"]
  }
```

Processing:
1. Fetch manifest from URL (or parse inline).
2. Validate: creator_did must exist, revocation_authority must exist.
3. Store manifest in SQLite.
4. Pre-register the agent: when it connects with this DID, auto-apply manifest settings (actor_class, provenance, capability requests).

**New SQLite table**:
```sql
CREATE TABLE IF NOT EXISTS agent_manifests (
    agent_did TEXT PRIMARY KEY,
    manifest_json TEXT NOT NULL,
    manifest_url TEXT,
    registered_by TEXT NOT NULL,   -- DID of who registered this manifest
    registered_at INTEGER NOT NULL,
    active INTEGER DEFAULT 1
);
```

**File**: `freeq-server/src/connection/registration.rs`

After SASL auth completes, check if the authenticated DID has a registered manifest:
```rust
if let Some(manifest) = state.db.get_agent_manifest(&session.did) {
    session.actor_class = ActorClass::Agent;
    session.provenance = Some(manifest_to_provenance(&manifest));
    // Auto-request capabilities on channel join
    session.default_capabilities = manifest.capabilities.default.clone();
}
```

### IRC Command for Manifest Registration

For the demo, allow inline registration from chat:

```
AGENT MANIFEST https://example.com/auditor.freeq.toml
```

Server fetches, validates, and registers. Responds:
```
:server NOTICE nick :✅ Agent manifest registered for auditor (did:plc:...). Capabilities: post_message, read_channel.
```

### SDK Changes

**File**: `freeq-sdk/src/client.rs`

```rust
/// Load and submit an agent manifest during registration.
pub async fn register_with_manifest(&self, manifest: AgentManifest) -> Result<()>;

/// Load manifest from a TOML file.
pub fn load_manifest(path: &Path) -> Result<AgentManifest>;
```

### Bot Changes (Demo Code)

**File**: `freeq-bots/src/main.rs`

Add `--manifest` flag:
```rust
#[arg(long)]
manifest: Option<PathBuf>,
```

At startup:
```rust
if let Some(manifest_path) = &args.manifest {
    let manifest = AgentManifest::load(manifest_path)?;
    handle.register_with_manifest(manifest).await?;
} else {
    // Existing behavior: manual AGENT REGISTER + PROVENANCE
}
```

Create `freeq-bots/manifests/factory.freeq.toml` and `freeq-bots/manifests/auditor.freeq.toml`.

---

## 2. Delegated Spawn

### Model

An existing agent (or human) creates a child agent that:
- Inherits the parent's identity chain.
- Gets a narrowed subset of the parent's capabilities.
- Can be revoked by revoking the parent or the child directly.
- Shows the parent in its provenance.

### Wire Format

```
AGENT SPAWN #factory :nick=qa-worker;capabilities=post_message,call_tool;ttl=300;task=01JQXYZ
```

Server:
1. Validates the spawner has `spawn_agent` capability in the channel.
2. Creates a virtual session for the child (no separate TCP connection needed — the parent relays).
3. Generates a child DID (or uses a session-scoped identifier like `did:freeq:session:01JQABC`).
4. Stores provenance chain: parent → child.
5. Grants narrowed capabilities (intersection of parent's caps and requested caps).
6. Broadcasts JOIN for the child.

### Provenance Chain

```json
{
  "actor_did": "did:freeq:session:01JQABC",
  "origin_type": "delegated_spawn",
  "creator_did": "did:plc:factory-did",
  "parent_actor": "did:plc:factory-did",
  "authority_basis": "Spawned by factory for task 01JQXYZ",
  "ttl_seconds": 300,
  "revocation_authority": "did:plc:factory-did"
}
```

### Virtual Sessions

**File**: `freeq-server/src/connection/mod.rs`

```rust
pub struct VirtualSession {
    pub session_id: String,
    pub parent_session_id: String,
    pub nick: String,
    pub did: String,          // session-scoped DID
    pub actor_class: ActorClass,
    pub provenance: ProvenanceDeclaration,
    pub capabilities: Vec<AgentCapability>,
    pub ttl: Option<Duration>,
    pub created_at: i64,
}
```

Virtual sessions:
- Appear in channel member lists with 🤖 badge.
- Can send messages (parent relays via `AGENT MSG <child-nick> #channel :text`).
- Have independent presence state.
- Auto-expire when TTL elapses (server sends QUIT).
- Are revoked if parent is revoked (cascade).

### SDK Changes

```rust
/// Spawn a child agent in a channel.
pub async fn spawn_agent(&self, channel: &str, nick: &str, capabilities: &[&str], ttl: Option<Duration>, task_ref: Option<&str>) -> Result<ChildAgentHandle>;

/// Send a message as a child agent.
pub async fn send_as_child(&self, child: &ChildAgentHandle, channel: &str, text: &str) -> Result<()>;

/// Despawn a child agent.
pub async fn despawn(&self, child: &ChildAgentHandle) -> Result<()>;
```

### Bot Changes (Demo Code)

**File**: `freeq-bots/src/factory/orchestrator.rs`

In the testing phase, spawn a QA worker:
```rust
Phase::Testing => {
    let qa_worker = handle.spawn_agent(
        &channel,
        "qa-worker",
        &["post_message", "call_tool"],
        Some(Duration::from_secs(300)),
        Some(&task_id),
    ).await?;

    // Run tests as the qa-worker
    handle.send_as_child(&qa_worker, &channel,
        "🧪 Running test suite...").await?;

    let test_output = tools::shell(&workspace, "npm test 2>&1", 60).await?;

    handle.send_as_child(&qa_worker, &channel,
        &format!("✅ Tests complete: {}", parse_test_summary(&test_output))).await?;

    // Despawn when done
    handle.despawn(&qa_worker).await?;
}
```

In the web client, the member list briefly shows "qa-worker" as a sub-agent with "parent: factory" in its identity card. When tests finish, it disappears.

---

## 3. Wrapper Trust Profiles

### Model

Wrappers bridge external agents into Freeq. The wrapper itself is an identifiable, auditable component.

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WrapperRecord {
    pub wrapper_did: String,
    pub wrapper_name: String,
    pub description: Option<String>,
    pub source_repo: Option<String>,
    pub image_digest: Option<String>,
    pub audit_status: WrapperAuditStatus,
    pub supported_protocols: Vec<String>,  // ["mcp", "a2a", "langchain"]
    pub wrapped_agents: Vec<String>,
    pub registered_by: String,
    pub registered_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum WrapperAuditStatus {
    Unaudited,
    CommunityReviewed,
    FormallyAudited,
}
```

### MCP Wrapper (Reference Implementation)

**New crate**: `freeq-mcp-wrapper/`

A standalone process that:
1. Connects to a Freeq server as an agent (with wrapper metadata).
2. Connects to an MCP server (external agent).
3. Translates MCP tool calls ↔ Freeq coordination events.
4. Enforces Freeq capabilities on the MCP agent's actions.
5. Signs actions with the wrapper's key.
6. Reports presence based on MCP server health.

```
┌──────────┐       ┌──────────────┐       ┌──────────────┐
│ MCP Agent│◄─────►│ Freeq MCP    │◄─────►│ Freeq Server │
│ (external)│  MCP │ Wrapper      │  IRC  │              │
└──────────┘       └──────────────┘       └──────────────┘
                   - translates identity
                   - enforces capabilities
                   - signs actions
                   - reports presence
```

### MCP Wrapper Implementation

```rust
// freeq-mcp-wrapper/src/main.rs

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    // Connect to Freeq as an agent
    let freeq = freeq_sdk::connect(&ConnectConfig {
        actor_class: Some(ActorClass::ExternalAgent),
        ..config
    }).await?;

    // Register wrapper
    freeq.submit_provenance(ProvenanceDeclaration {
        origin_type: OriginType::ExternalImport,
        implementation_ref: Some(format!("freeq-mcp-wrapper@{}", env!("CARGO_PKG_VERSION"))),
        ..provenance
    }).await?;

    // Connect to MCP server
    let mcp = mcp_client::connect(&args.mcp_server_url).await?;

    // Discover MCP tools and map to Freeq capabilities
    let mcp_tools = mcp.list_tools().await?;
    let requested_caps: Vec<String> = mcp_tools.iter()
        .map(|t| format!("call_tool:{}", t.name))
        .collect();
    freeq.request_capabilities(&args.channel, &requested_caps).await?;

    // Start heartbeat
    freeq.start_heartbeat(Duration::from_secs(30))?;

    // Event loop: relay between MCP and Freeq
    loop {
        tokio::select! {
            event = freeq.recv() => {
                match event {
                    // If someone in the channel asks the agent to do something,
                    // translate to MCP tool call
                    Event::Message { from, text, .. } if is_command(&text) => {
                        let tool_name = parse_tool_name(&text);
                        let tool_args = parse_tool_args(&text);

                        // Check capability before calling
                        if !freeq.has_capability(&format!("call_tool:{}", tool_name)) {
                            freeq.privmsg(&channel, "❌ I don't have permission to use that tool here").await?;
                            continue;
                        }

                        freeq.set_presence(PresenceState::Executing,
                            Some(&format!("calling {}", tool_name)), None).await?;

                        let result = mcp.call_tool(&tool_name, &tool_args).await?;

                        freeq.emit_event(&channel, EventType::StatusUpdate,
                            json!({ "tool": tool_name, "result_summary": summarize(&result) }),
                            None).await?;

                        freeq.privmsg(&channel, &format_result(&result)).await?;
                        freeq.set_presence(PresenceState::Online, None, None).await?;
                    }
                    // Handle governance signals
                    Event::Governance(signal) => {
                        match signal {
                            GovernanceSignal::Pause { .. } => mcp.pause().await?,
                            GovernanceSignal::Revoke { .. } => {
                                mcp.disconnect().await?;
                                break;
                            }
                            _ => {}
                        }
                    }
                    _ => {}
                }
            }
            // MCP-initiated notifications
            notification = mcp.recv() => {
                // Translate MCP notifications to Freeq messages
                freeq.privmsg(&channel, &format_notification(&notification)).await?;
            }
        }
    }
}
```

### Wrapper Registration

```
POST /api/v1/wrappers/register
Body: {
    wrapper_name: "freeq-mcp-wrapper",
    source_repo: "https://github.com/chad/freeq",
    supported_protocols: ["mcp"],
    audit_status: "community_reviewed"
}
Auth: server oper

→ { wrapper_did: "did:freeq:wrapper:01JQABC" }
```

### Web Client Changes

In the identity card for a wrapped external agent:
```
┌────────────────────────────────────────┐
│ 🌐 support-bot (External Agent)       │
│ DID: did:plc:...                       │
├────────────────────────────────────────┤
│ Wrapper: freeq-mcp-wrapper v0.1.0     │
│   Source: github.com/chad/freeq       │
│   Audit: ⚠ Community reviewed         │
│   Protocol: MCP                        │
├────────────────────────────────────────┤
│ Provenance:                            │
│   Origin: External import via MCP      │
│   Imported by: chad                    │
│   Original system: mcp.example.com     │
├────────────────────────────────────────┤
│ Capabilities: post_message,            │
│   call_tool:search, call_tool:lookup   │
│ State: 🟢 online                       │
└────────────────────────────────────────┘
```

---

## 4. S2S Federation

### New S2S Messages

```rust
S2sMessage::ManifestRegistered {
    agent_did: String,
    manifest: AgentManifest,
}
S2sMessage::AgentSpawned {
    parent_did: String,
    child_did: String,
    child_nick: String,
    channel: String,
    capabilities: Vec<String>,
    ttl_seconds: Option<u64>,
}
S2sMessage::AgentDespawned {
    child_did: String,
    channel: String,
}
S2sMessage::WrapperRegistered {
    wrapper: WrapperRecord,
}
```

Spawned agents are visible on federated servers. When a child agent sends a message via its parent, the S2S relay preserves the child's identity in the message tags.

---

## Demo Script

### Prerequisites
- freeq-server with Phase 1–4 changes
- freeq-bots with manifest support and spawn capability
- freeq-mcp-wrapper (new crate) — a simple MCP bridge
- An MCP server to bridge (e.g., a filesystem MCP server or a custom one)
- freeq-app with wrapper display in identity cards

### Steps

1. **Manifest-based agent registration**:
   ```bash
   # Register the auditor agent from a manifest
   curl -X POST http://localhost:8080/api/v1/agents/register \
     -H "Content-Type: application/json" \
     -d '{"manifest_url": "https://raw.githubusercontent.com/chad/freeq/main/freeq-bots/manifests/auditor.freeq.toml"}'
   ```

   Start the auditor bot with `--manifest manifests/auditor.freeq.toml`. It connects, auto-registers as agent, auto-submits provenance, auto-requests capabilities, starts heartbeating. Zero manual setup.

2. **Delegated spawn**:
   - Say `factory: build a landing page` in `#factory`.
   - During the testing phase, "qa-worker" appears in the member list.
   - Click its name → identity card shows "Parent: factory, TTL: 5 minutes, Task: 01JQXYZ".
   - Tests finish, qa-worker disappears.

3. **External agent via MCP wrapper**:
   ```bash
   # Start the MCP wrapper bridging an external tool server
   freeq-mcp-wrapper \
     --freeq-server irc.freeq.at:6667 \
     --mcp-server http://localhost:3001 \
     --nick support-bot \
     --channel "#support"
   ```

   In `#support`, "support-bot" appears as 🌐 (external agent). Identity card shows the wrapper info and MCP origin. Its capabilities are sandboxed to what the channel policy allows.

### What This Proves
- Agents can be introduced declaratively from manifests — no manual config.
- Agents can spawn sub-agents with automatic provenance chains and scoped TTLs.
- External agents from other protocols (MCP) can participate in Freeq with full governance.
- Wrappers are transparent and inspectable, not invisible plumbing.
- All three introduction paths produce agents with the same governance guarantees.

---

## External Demo Dependencies

### MCP Test Server

For the wrapper demo, we need a simple MCP server. Options:
1. **Filesystem MCP server**: reads/writes files in a sandbox. Available as `@modelcontextprotocol/server-filesystem`.
2. **Custom "support knowledge base" MCP server**: ~50 lines of TypeScript that responds to `search` and `lookup` tool calls with canned answers. More compelling for a demo.
3. **GitHub MCP server**: `@modelcontextprotocol/server-github`. Bridges GitHub API via MCP. Very compelling for the auditor use case.

Recommendation: Use the GitHub MCP server. Demo story: "An MCP-based GitHub agent joins `#factory` through the Freeq wrapper. It can search issues and read PRs, but only in repos the channel policy allows. The wrapper enforces the capability boundary."

### Manifest Hosting

Manifests need to be fetchable by URL. Options:
1. Host in the freeq GitHub repo under `manifests/`.
2. Serve from the freeq-site Flask app.
3. Raw GitHub URLs work fine for demo.
