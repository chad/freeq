# Phase 1: Known Actors — Detailed Implementation Plan

**Goal**: Every serious participant is inspectable — identity, provenance, actor class, and rich presence.

**Demo**: Start the factory bot and a human user in `#factory`. The bot shows up with a 🤖 badge. Click its name in the web client to see an identity card: "Created by did:plc:xxx, running freeq-bots v0.1, source github.com/chad/freeq, current state: idle." Say `factory: build a landing page` — the identity card live-updates to "executing: specifying." If you kill the bot process, it transitions to "degraded" within 60 seconds and then disappears. A user on irssi sees the bot join and chat normally with no disruption.

---

## Design Note: Conversational Addressing, Not Slash Commands

Agents are addressed conversationally, not via slash commands. Users talk to agents by name:

```
factory: build a landing page for a coffee shop
@factory what's the status?
factory, pause
```

**Why not slash commands?** Slash commands (`/factory build ...`) only work in our web client. On irssi, weechat, or any standard IRC client, `/factory` tries to execute a local IRC command called `FACTORY`, which doesn't exist. Conversational addressing works everywhere because it's just a regular PRIVMSG.

This also matches the vision: agents are first-class participants in a shared room, not hidden services invoked with magic prefixes. Everyone sees the request and the response as a natural conversation.

The bot matches messages addressed to its nick (prefix `nick:`, `nick,`, or `@nick`). The SDK provides a helper:

```rust
/// Check if a message is addressed to this agent.
pub fn is_addressed_to_me(text: &str, my_nick: &str) -> Option<&str> {
    // Matches: "factory: build ...", "factory, build ...", "@factory build ..."
    // Returns the text after the address prefix, or None.
}
```

---

## 0. Bot Identity: `did:web`, `did:key`, and the `freeq-bot-id` Tool

Before anything else, agents need DIDs. Humans get theirs from AT Protocol (Bluesky accounts), but requiring bot operators to create a Bluesky account and go through phone verification is a non-starter. Agents need a self-sovereign identity path.

### `freeq-bot-id` CLI Tool

**New crate**: `freeq-bot-id/`

A tiny CLI (~150 lines of Rust) that generates bot identities:

```bash
# With a domain — produces did:web
freeq-bot-id create --name factory --domain freeq.at
#   ✅ Private key: ~/.freeq/bots/factory/key.ed25519
#   ✅ DID document: ./factory/did.json
#   ✅ DID: did:web:freeq.at:bots:factory
#
#   Serve did.json at: https://freeq.at/bots/factory/did.json
#   Connect with: freeq-bots --did did:web:freeq.at:bots:factory --key ~/.freeq/bots/factory/key.ed25519

# Without a domain — produces did:key (zero infrastructure)
freeq-bot-id create --name factory
#   ✅ Private key: ~/.freeq/bots/factory/key.ed25519
#   ✅ DID: did:key:z6MkrTQ...
#
#   Connect with: freeq-bots --did did:key:z6MkrTQ... --key ~/.freeq/bots/factory/key.ed25519

# Inspect an existing identity
freeq-bot-id info --name factory
#   DID: did:web:freeq.at:bots:factory
#   Public key: z6MkrTQ...
#   Created: 2026-03-11
#   Key file: ~/.freeq/bots/factory/key.ed25519

# Rotate key (generates new keypair, updates did.json, prints old key for revocation)
freeq-bot-id rotate --name factory
```

**Implementation**:

```rust
// freeq-bot-id/src/main.rs
use clap::{Parser, Subcommand};
use ed25519_dalek::{SigningKey, VerifyingKey};
use rand::rngs::OsRng;

#[derive(Parser)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Create {
        #[arg(long)]
        name: String,
        #[arg(long)]
        domain: Option<String>,
    },
    Info {
        #[arg(long)]
        name: String,
    },
    Rotate {
        #[arg(long)]
        name: String,
    },
}

fn create(name: &str, domain: Option<&str>) -> Result<()> {
    let signing_key = SigningKey::generate(&mut OsRng);
    let verifying_key = signing_key.verifying_key();
    let multibase_pub = multibase_encode_ed25519(&verifying_key);

    let key_dir = dirs::home_dir().unwrap().join(".freeq/bots").join(name);
    std::fs::create_dir_all(&key_dir)?;
    std::fs::write(key_dir.join("key.ed25519"), signing_key.to_bytes())?;

    if let Some(domain) = domain {
        // did:web
        let did = format!("did:web:{}:bots:{}", domain, name);
        let did_doc = serde_json::json!({
            "@context": ["https://www.w3.org/ns/did/v1", "https://w3id.org/security/multikey/v1"],
            "id": did,
            "authentication": [{
                "id": format!("{}#key-1", did),
                "type": "Multikey",
                "controller": did,
                "publicKeyMultibase": multibase_pub,
            }],
        });
        let doc_dir = PathBuf::from(name);
        std::fs::create_dir_all(&doc_dir)?;
        std::fs::write(doc_dir.join("did.json"), serde_json::to_string_pretty(&did_doc)?)?;

        println!("✅ DID: {did}");
        println!("✅ Private key: {}", key_dir.join("key.ed25519").display());
        println!("✅ DID document: {}/did.json", doc_dir.display());
        println!("\n   Serve did.json at: https://{}/bots/{}/did.json", domain, name);
    } else {
        // did:key
        let did = format!("did:key:{}", multibase_pub);
        println!("✅ DID: {did}");
        println!("✅ Private key: {}", key_dir.join("key.ed25519").display());
    }

    println!("\n   Connect with: freeq-bots --did <DID> --key {}", key_dir.join("key.ed25519").display());
    Ok(())
}
```

### DID Document Format

The generated `did.json` for `did:web:freeq.at:bots:factory`:

```json
{
  "@context": [
    "https://www.w3.org/ns/did/v1",
    "https://w3id.org/security/multikey/v1"
  ],
  "id": "did:web:freeq.at:bots:factory",
  "authentication": [
    {
      "id": "did:web:freeq.at:bots:factory#key-1",
      "type": "Multikey",
      "controller": "did:web:freeq.at:bots:factory",
      "publicKeyMultibase": "z6MkrTQ..."
    }
  ]
}
```

### Server: `did:web` Resolution

**File**: `freeq-server/src/connection/sasl.rs` (or wherever DID resolution lives)

Add `did:web` resolver alongside existing `did:plc` resolution:

```rust
async fn resolve_did(did: &str) -> Result<DidDocument> {
    if did.starts_with("did:plc:") {
        // Existing AT Protocol resolution via plc.directory
        resolve_did_plc(did).await
    } else if did.starts_with("did:web:") {
        resolve_did_web(did).await
    } else if did.starts_with("did:key:") {
        resolve_did_key(did)
    } else {
        Err(anyhow!("Unsupported DID method: {}", did))
    }
}

async fn resolve_did_web(did: &str) -> Result<DidDocument> {
    // did:web:freeq.at:bots:factory → https://freeq.at/bots/factory/did.json
    // did:web:example.com → https://example.com/.well-known/did.json
    let parts: Vec<&str> = did.strip_prefix("did:web:").unwrap().split(':').collect();
    let domain = parts[0].replace("%3A", ":");  // port encoding
    let path = if parts.len() > 1 {
        format!("/{}/did.json", parts[1..].join("/"))
    } else {
        "/.well-known/did.json".to_string()
    };
    let url = format!("https://{}{}", domain, path);

    let doc: DidDocument = reqwest::get(&url).await?.json().await?;
    Ok(doc)
}

fn resolve_did_key(did: &str) -> Result<DidDocument> {
    // did:key:z6Mk... → extract public key from the multibase string
    let multibase = did.strip_prefix("did:key:").unwrap();
    let public_key = decode_multibase_ed25519(multibase)?;

    // Synthesize a DID document from the key
    Ok(DidDocument {
        id: did.to_string(),
        authentication: vec![VerificationMethod {
            id: format!("{}#key-1", did),
            key_type: "Multikey".to_string(),
            controller: did.to_string(),
            public_key_multibase: Some(multibase.to_string()),
        }],
    })
}
```

This is ~60 lines total for both resolvers. The SASL challenge-response flow stays identical — server sends challenge, bot signs with private key, server verifies against the public key from the resolved DID document.

### Server: `did:key` SASL Flow

For `did:key`, the SASL `AUTHENTICATE` message carries the DID directly. The server:
1. Extracts the public key from the DID string (no network fetch needed).
2. Sends a challenge.
3. Verifies the signature.
4. Binds the session to the `did:key:...` identity.

This means `did:key` bots can authenticate with zero external dependencies — no PDS, no domain, no HTTP fetch. Just a keypair.

### Hosting DID Documents for Our Bots

**File**: `freeq-site/app.py`

Add a static route for our own bots' DID documents:

```python
@app.route('/bots/<name>/did.json')
def bot_did(name):
    path = os.path.join('bots', name, 'did.json')
    if os.path.exists(path):
        return send_file(path, mimetype='application/json')
    abort(404)
```

Then `did:web:freeq.at:bots:factory` resolves to `https://freeq.at/bots/factory/did.json`.

### Summary of Identity Options

| Method | Infrastructure Needed | Human-Readable | Suitable For |
|---|---|---|---|
| `did:plc` (Bluesky) | PDS account | Via handle | Humans, bots with social presence |
| `did:web` | Domain + HTTPS | Yes (`did:web:freeq.at:bots:factory`) | Production bots, org-operated agents |
| `did:key` | Nothing | No | Ephemeral bots, sub-agents, dev/testing |

---

## 1. Actor Class Registration

### Server Changes

**File**: `freeq-server/src/connection/mod.rs`

Add to `ConnectionState`:
```rust
pub(crate) actor_class: ActorClass,
```

```rust
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum ActorClass {
    Human,
    Agent,
    ExternalAgent,
}

impl Default for ActorClass {
    fn default() -> Self { ActorClass::Human }
}
```

**File**: `freeq-server/src/connection/registration.rs`

After CAP END / registration complete, accept a new command:
```
AGENT REGISTER :class=agent
```

Parse and store in session state. This is optional — if not sent, `actor_class` stays `Human`.

**File**: `freeq-server/src/connection/cap.rs`

Add `freeq.at/agent-info` to the CAP LS response. Clients that negotiate this cap receive `+freeq.at/actor-class` tags on JOIN and WHOIS.

**File**: `freeq-server/src/connection/channel.rs`

In extended-join broadcast, include the tag:
```
@account=did:plc:xxx;+freeq.at/actor-class=agent :factory!factory@freeq/plc/abc JOIN #factory * :freeq AI factory bot
```

**File**: `freeq-server/src/connection/queries.rs`

New WHOIS numeric `RPL_ACTORCLASS` (673):
```
:server 673 requester factory :actor_class=agent
```

### SDK Changes

**File**: `freeq-sdk/src/client.rs`

Add to `ConnectConfig`:
```rust
pub actor_class: Option<ActorClass>,
```

After registration, if `actor_class` is `Some(Agent)`, send `AGENT REGISTER :class=agent`.

### Web Client Changes

**File**: `freeq-app/src/store.ts`

Add `actorClass: 'human' | 'agent' | 'external_agent'` to the `Member` type. Parse from `+freeq.at/actor-class` in JOIN handler and WHOIS response.

**File**: `freeq-app/src/components/MemberList.tsx`

Show 🤖 badge next to agent nicks. Sort agents into a separate "Agents" section below humans in the member list.

### S2S Changes

**File**: `freeq-server/src/s2s.rs`

Include `actor_class` in `S2sMessage::Join`. Remote servers store it in `remote_members`.

---

## 2. Provenance Declarations

### Server Changes

**New file**: `freeq-server/src/provenance.rs`

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProvenanceDeclaration {
    pub actor_did: String,
    pub origin_type: OriginType,
    pub creator_did: Option<String>,
    pub sponsor_did: Option<String>,
    pub authority_basis: Option<String>,
    pub implementation_ref: Option<String>,  // e.g. "github.com/chad/freeq/freeq-bots@v0.1"
    pub source_repo: Option<String>,
    pub image_digest: Option<String>,
    pub revocation_authority: Option<String>,
    pub created_at: String,
    pub signature: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum OriginType {
    ExternalImport,
    Template,
    DelegatedSpawn,
    ChannelAssignment,
}
```

**New SQLite table** (in `server.rs` schema init):
```sql
CREATE TABLE IF NOT EXISTS provenance_declarations (
    did TEXT PRIMARY KEY,
    origin_type TEXT NOT NULL,
    creator_did TEXT,
    sponsor_did TEXT,
    authority_basis TEXT,
    implementation_ref TEXT,
    source_repo TEXT,
    image_digest TEXT,
    revocation_authority TEXT,
    declaration_json TEXT NOT NULL,
    signature TEXT NOT NULL,
    created_at INTEGER NOT NULL
);
```

**File**: `freeq-server/src/connection/mod.rs`

New command handler for `PROVENANCE`:
```
PROVENANCE :eyJ...base64url-encoded-json...
```

Server decodes, validates the signature against the session DID's key, stores in SQLite.

**File**: `freeq-server/src/web.rs`

New REST endpoint:
```
GET /api/v1/agents/{did}/provenance → ProvenanceDeclaration JSON
```

### SDK Changes

**File**: `freeq-sdk/src/client.rs`

```rust
pub async fn submit_provenance(&self, decl: ProvenanceDeclaration) -> Result<()>;
```

Signs the declaration with the session key, base64url-encodes, sends `PROVENANCE` command.

### Bot Changes (Demo Code)

**File**: `freeq-bots/src/main.rs`

After connecting and registering as an agent, submit provenance:
```rust
let provenance = ProvenanceDeclaration {
    actor_did: my_did.clone(),
    origin_type: OriginType::ExternalImport,
    creator_did: Some("did:plc:4qsyxmnsblo4luuycm3572bq".into()), // chad
    sponsor_did: Some("did:plc:4qsyxmnsblo4luuycm3572bq".into()),
    authority_basis: Some("Operated by server administrator".into()),
    implementation_ref: Some("freeq-bots@0.1.0".into()),
    source_repo: Some("https://github.com/chad/freeq".into()),
    revocation_authority: Some("did:plc:4qsyxmnsblo4luuycm3572bq".into()),
    ..Default::default()
};
handle.submit_provenance(provenance).await?;
```

### Web Client Changes

**File**: `freeq-app/src/components/MemberList.tsx`

The agent profile card fetches `/api/v1/agents/{did}/provenance` and shows:
- **Created by**: display name of `creator_did`
- **Source**: link to `source_repo`
- **Implementation**: `implementation_ref`
- **Revocation authority**: display name of `revocation_authority`

---

## 3. Rich Agent Presence

### Server Changes

**File**: `freeq-server/src/connection/mod.rs`

Add to `ConnectionState`:
```rust
pub(crate) agent_presence: Option<AgentPresence>,
```

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentPresence {
    pub state: PresenceState,
    pub status_text: Option<String>,
    pub task_ref: Option<String>,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum PresenceState {
    Online,
    Idle,
    Active,
    Executing,
    WaitingForInput,
    BlockedOnPermission,
    BlockedOnBudget,
    Degraded,
    Paused,
    Sandboxed,
    RateLimited,
    Revoked,
    Offline,
}
```

New command:
```
PRESENCE :state=executing;status=specifying requirements;task=build-landing-page
```

The server:
1. Parses the key-value pairs.
2. Stores in `agent_presence`.
3. Broadcasts to channel members who negotiated `freeq.at/agent-info` cap, using the existing AWAY mechanism:
   ```
   :factory!factory@freeq/plc/abc AWAY :{"state":"executing","status":"specifying requirements","task":"build-landing-page"}
   ```
   (JSON in AWAY reason — old clients display it as text, new clients parse it.)

**File**: `freeq-server/src/web.rs`

Include presence in the identity card REST response:
```
GET /api/v1/actors/{did} → { ..., presence: { state: "executing", status: "...", task: "..." } }
```

### SDK Changes

**File**: `freeq-sdk/src/client.rs`

```rust
pub async fn set_presence(&self, state: PresenceState, status: Option<&str>, task: Option<&str>) -> Result<()>;
```

### Bot Changes (Demo Code)

**File**: `freeq-bots/src/factory/orchestrator.rs`

At each phase transition, update presence:
```rust
// In the specifying phase:
handle.set_presence(PresenceState::Executing, Some("specifying requirements"), Some(&project_name)).await?;

// When waiting for user input:
handle.set_presence(PresenceState::WaitingForInput, Some("awaiting human feedback"), None).await?;

// When idle:
handle.set_presence(PresenceState::Idle, None, None).await?;
```

The factory already has `Phase` enum (Idle, Specifying, Designing, Building, Reviewing, Testing, Deploying, Complete, Paused) — map each to the corresponding `PresenceState`.

### Web Client Changes

**File**: `freeq-app/src/components/MemberList.tsx`

For agents, replace the simple online/away dot with a state-specific indicator:
- 🟢 `online`/`idle` — green dot
- ⚡ `executing`/`active` — animated lightning bolt
- ⏳ `waiting_for_input` — hourglass
- 🔒 `blocked_on_permission` — lock
- 💰 `blocked_on_budget` — coin
- 🟡 `degraded` — yellow dot
- ⏸️ `paused` — pause icon
- 🔴 `revoked` — red dot

Show `status_text` as a subtitle under the agent's nick in the member list.

---

## 4. Signed Heartbeat

### Server Changes

**File**: `freeq-server/src/connection/mod.rs`

Add to `ConnectionState`:
```rust
pub(crate) last_heartbeat: Option<i64>,  // unix timestamp
pub(crate) heartbeat_ttl: u64,           // seconds
```

New command:
```
@+freeq.at/sig=<sig> HEARTBEAT :state=active;ttl=60
```

The server:
1. Verifies the signature against the session's registered signing key.
2. Updates `last_heartbeat` and `heartbeat_ttl`.
3. Updates presence state from heartbeat payload.

**File**: `freeq-server/src/server.rs`

Background task (runs every 15 seconds):
```rust
// For each agent session:
if let Some(last) = session.last_heartbeat {
    let elapsed = now - last;
    let ttl = session.heartbeat_ttl;
    if elapsed > ttl * 5 {
        // Force disconnect
        disconnect_session(session_id, "heartbeat timeout");
    } else if elapsed > ttl * 2 {
        // Transition to offline
        set_presence(session_id, PresenceState::Offline);
    } else if elapsed > ttl {
        // Transition to degraded
        set_presence(session_id, PresenceState::Degraded);
    }
}
```

### SDK Changes

**File**: `freeq-sdk/src/client.rs`

```rust
pub fn start_heartbeat(&self, interval: Duration) -> Result<()>;
```

Spawns a background tokio task:
```rust
loop {
    tokio::time::sleep(interval).await;
    let sig = sign_message(b"HEARTBEAT", &self.signing_key);
    self.send_raw(&format!(
        "@+freeq.at/sig={} HEARTBEAT :state={};ttl={}",
        sig, current_state, interval.as_secs() * 2
    )).await?;
}
```

### Bot Changes (Demo Code)

**File**: `freeq-bots/src/main.rs`

After registration:
```rust
handle.start_heartbeat(Duration::from_secs(30))?;
```

That's it. The SDK handles the rest.

### New SQLite Table

```sql
CREATE TABLE IF NOT EXISTS heartbeats (
    did TEXT PRIMARY KEY,
    state TEXT NOT NULL,
    status_text TEXT,
    ttl_seconds INTEGER NOT NULL,
    last_seen INTEGER NOT NULL,
    signature TEXT NOT NULL
);
```

---

## 5. Identity Card REST Endpoint

### Server Changes

**File**: `freeq-server/src/web.rs`

```
GET /api/v1/actors/{did}
```

Response:
```json
{
  "did": "did:plc:abc123",
  "actor_class": "agent",
  "display_name": "factory",
  "handle": "factory.bsky.social",
  "online": true,
  "channels": ["#factory"],
  "provenance": {
    "origin_type": "external_import",
    "creator_did": "did:plc:4qsyxmnsblo4luuycm3572bq",
    "creator_name": "chad",
    "source_repo": "https://github.com/chad/freeq",
    "implementation_ref": "freeq-bots@0.1.0",
    "revocation_authority": "did:plc:4qsyxmnsblo4luuycm3572bq"
  },
  "presence": {
    "state": "executing",
    "status": "building landing page",
    "task": "build-landing-page",
    "updated_at": "2026-03-11T20:00:00Z"
  },
  "heartbeat": {
    "last_seen": "2026-03-11T20:00:30Z",
    "ttl_seconds": 60,
    "healthy": true
  },
  "created_at": "2026-03-01T00:00:00Z"
}
```

### Web Client Changes

**New file**: `freeq-app/src/components/AgentCard.tsx`

Full identity card component. Shown when clicking an agent in the member list (replaces the current profile panel for agents).

Sections:
1. **Header**: avatar placeholder + 🤖 badge + nick + DID
2. **Status**: presence state icon + status text + task reference
3. **Provenance**: creator, source repo (linked), implementation version
4. **Heartbeat**: last seen, TTL, health indicator (green/yellow/red)
5. **Channels**: list of channels the agent is in

---

## 6. S2S Federation

All new state propagates over S2S:

**File**: `freeq-server/src/s2s.rs`

New S2S message variants:
```rust
S2sMessage::AgentInfo {
    did: String,
    actor_class: ActorClass,
    provenance: Option<ProvenanceDeclaration>,
}
S2sMessage::PresenceUpdate {
    did: String,
    presence: AgentPresence,
}
S2sMessage::HeartbeatUpdate {
    did: String,
    state: PresenceState,
    ttl: u64,
    timestamp: i64,
}
```

Sent on agent registration, presence change, and heartbeat. Remote servers store in their local state and serve via the same REST endpoints.

---

## Demo Script

### Prerequisites
- freeq-server running locally or on irc.freeq.at
- freeq-bots built with Phase 1 changes
- freeq-app (web client) built with Phase 1 changes
- irssi or weechat for the "old client" comparison

### Steps

1. **Start the server** — no config changes needed, Phase 1 is all opt-in.

2. **Connect with irssi** (guest mode):
   ```
   /connect irc.freeq.at
   /join #factory
   ```
   Observe: normal IRC behavior. You see the factory bot join.

3. **Connect with the web client** (authenticated):
   - Open irc.freeq.at in browser, log in with AT Protocol.
   - Join `#factory`.
   - Observe: member list shows the factory bot with 🤖 badge.
   - Click the bot's name → identity card appears with provenance, presence state, heartbeat health.

4. **Trigger bot activity**:
   - Say `factory: build a landing page for a coffee shop` in `#factory`.
   - Watch the identity card update: state goes from "idle" → "executing: specifying requirements" → "executing: designing" → etc.
   - irssi user sees the same chat messages, just without the visual presence updates.

5. **Kill the bot process** (Ctrl+C):
   - Within 60 seconds, the web client shows the bot transition to "degraded" (yellow dot).
   - Within 2.5 minutes, it transitions to "offline" and disappears from the member list.
   - irssi user sees a normal QUIT message.

6. **Restart the bot**:
   - It reconnects, re-registers as agent, re-submits provenance, starts heartbeating.
   - Web client shows it reappear with 🤖 badge and "online" state.

### What This Proves
- Agents are visually distinguishable from humans.
- Provenance is inspectable (who created this, where's the code).
- Operational state is live and meaningful.
- Liveness detection works automatically.
- Zero disruption to legacy IRC clients.
