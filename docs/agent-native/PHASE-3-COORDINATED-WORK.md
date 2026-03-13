# Phase 3: Coordinated Work — Detailed Implementation Plan

**Goal**: Typed coordination events, evidence attachments, and audit timelines that make agent work inspectable and traceable.

**Demo**: A user says `factory: build a todo app` in `#factory`. The factory bot creates a task, posts structured updates as each agent role works (product → architect → builder → reviewer → QA → deploy), attaches evidence (architecture doc, test results, deploy URL) at each stage, and completes the task. The web client renders this as a visual timeline with expandable evidence. An op opens the audit tab and filters by the factory bot to see every action it took, every approval it received, and every artifact it produced — all cryptographically signed. The same session viewed from irssi shows readable text summaries of each step.

---

## 1. Typed Coordination Events

### Event Model

Coordination events are carried as IRCv3 tags on TAGMSG (machine-readable) with an accompanying PRIVMSG (human-readable fallback).

**New file**: `freeq-server/src/coordination.rs`

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoordinationEvent {
    pub event_id: String,        // ULID
    pub event_type: EventType,
    pub actor_did: String,
    pub channel: String,
    pub ref_id: Option<String>,  // references a task or parent event
    pub payload: serde_json::Value,
    pub signature: Option<String>,
    pub timestamp: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum EventType {
    TaskRequest,
    TaskAccept,
    TaskUpdate,
    TaskComplete,
    TaskFailed,
    ApprovalRequest,
    ApprovalResult,
    EvidenceAttach,
    DelegationNotice,
    RevocationNotice,
    StatusUpdate,
}
```

### Wire Format

Agent sends a task creation:
```
@+freeq.at/event=task_request;+freeq.at/task-id=01JQXYZ;+freeq.at/sig=<sig> TAGMSG #factory :{"description":"Build a todo app","requested_by":"chad"}
```

Followed by a human-readable PRIVMSG:
```
PRIVMSG #factory :📋 New task: Build a todo app (task: 01JQXYZ)
```

Agent posts a status update:
```
@+freeq.at/event=task_update;+freeq.at/ref=01JQXYZ;+freeq.at/phase=designing;+freeq.at/sig=<sig> TAGMSG #factory :{"phase":"designing","summary":"Chose React + Express stack"}
PRIVMSG #factory :🏗 [architect] Designing: Chose React + Express stack
```

Agent attaches evidence:
```
@+freeq.at/event=evidence_attach;+freeq.at/ref=01JQXYZ;+freeq.at/evidence-type=test_result;+freeq.at/sig=<sig> TAGMSG #factory :{"type":"test_result","summary":"12/12 passed","url":"https://...","hash":"sha256:abc"}
PRIVMSG #factory :✅ [qa] Tests: 12/12 passed — https://...
```

Task completion:
```
@+freeq.at/event=task_complete;+freeq.at/ref=01JQXYZ;+freeq.at/sig=<sig> TAGMSG #factory :{"summary":"Todo app deployed","url":"https://todo-app.miren.dev","duration_seconds":180}
PRIVMSG #factory :🎉 Task complete: Todo app deployed at https://todo-app.miren.dev (3m 0s)
```

### Server Changes

**File**: `freeq-server/src/connection/messaging.rs`

When processing TAGMSG with `+freeq.at/event`, extract and store the coordination event:

```rust
if let Some(event_type) = tags.get("+freeq.at/event") {
    let event = CoordinationEvent {
        event_id: tags.get("msgid").unwrap_or(&ulid::new()).clone(),
        event_type: event_type.parse()?,
        actor_did: session.did.clone(),
        channel: target.clone(),
        ref_id: tags.get("+freeq.at/ref").cloned(),
        payload: parse_tagmsg_body(&body),
        signature: tags.get("+freeq.at/sig").cloned(),
        timestamp: chrono::Utc::now().timestamp(),
    };
    state.db.store_coordination_event(&event)?;
}
// Always relay the TAGMSG to channel members (existing behavior)
```

### New SQLite Table

```sql
CREATE TABLE IF NOT EXISTS coordination_events (
    event_id TEXT PRIMARY KEY,
    event_type TEXT NOT NULL,
    actor_did TEXT NOT NULL,
    channel TEXT NOT NULL,
    ref_id TEXT,
    payload_json TEXT NOT NULL,
    signature TEXT,
    timestamp INTEGER NOT NULL
);

CREATE INDEX idx_coord_channel ON coordination_events(channel, timestamp);
CREATE INDEX idx_coord_ref ON coordination_events(ref_id);
CREATE INDEX idx_coord_actor ON coordination_events(actor_did, timestamp);
```

### REST API

**File**: `freeq-server/src/web.rs`

```
GET /api/v1/channels/{name}/events
    ?type=task_request,task_update,task_complete
    &ref_id=01JQXYZ
    &actor=did:plc:xxx
    &since=2026-03-01T00:00:00Z
    &limit=50

→ [CoordinationEvent]
```

```
GET /api/v1/tasks/{task_id}
→ {
    task_id: "01JQXYZ",
    description: "Build a todo app",
    status: "complete",
    actor_did: "did:plc:abc",
    channel: "#factory",
    created_at: "...",
    completed_at: "...",
    events: [CoordinationEvent],   // all events referencing this task
    evidence: [EvidenceRecord]      // all evidence for this task
  }
```

### SDK Changes

**File**: `freeq-sdk/src/client.rs`

```rust
/// Emit a typed coordination event to a channel.
pub async fn emit_event(&self, channel: &str, event_type: EventType, payload: serde_json::Value, ref_id: Option<&str>) -> Result<String>;

/// Convenience: create a new task and return its ID.
pub async fn create_task(&self, channel: &str, description: &str) -> Result<String>;

/// Convenience: update a task's status.
pub async fn update_task(&self, channel: &str, task_id: &str, phase: &str, summary: &str) -> Result<()>;

/// Convenience: complete a task.
pub async fn complete_task(&self, channel: &str, task_id: &str, summary: &str, url: Option<&str>) -> Result<()>;

/// Convenience: fail a task.
pub async fn fail_task(&self, channel: &str, task_id: &str, error: &str) -> Result<()>;
```

Each of these sends both the TAGMSG (structured) and PRIVMSG (human-readable).

### Bot Changes (Demo Code)

**File**: `freeq-bots/src/factory/orchestrator.rs`

Replace the current freeform `output::status()` calls with structured coordination events:

```rust
// Current (Phase 0):
output::status(handle, &channel, &product(), "📋", "Clarifying requirements...").await?;

// Phase 3:
let task_id = handle.create_task(&channel, &format!("Build: {}", spec)).await?;
handle.update_task(&channel, &task_id, "specifying", "Clarifying requirements").await?;

// ... later ...
handle.update_task(&channel, &task_id, "designing", "Chose React + Express stack").await?;

// ... after tests pass ...
handle.attach_evidence(&channel, &task_id, Evidence {
    evidence_type: "test_result".into(),
    summary: format!("{passed}/{total} tests passed"),
    url: None,
    hash: None,
    raw: Some(test_output),
}).await?;

// ... after deploy ...
handle.complete_task(&channel, &task_id, "Deployed successfully", Some(&deploy_url)).await?;
```

Each factory phase maps to a task update:

| Factory Phase | Event Sequence |
|---|---|
| `factory: build <spec>` | `task_request` → human-readable "📋 New task: ..." |
| Specifying | `task_update(phase=specifying)` |
| Designing | `task_update(phase=designing)` + `evidence_attach(type=architecture)` |
| Building | `task_update(phase=building)` |
| Reviewing | `task_update(phase=reviewing)` + `evidence_attach(type=code_review)` |
| Testing | `task_update(phase=testing)` + `evidence_attach(type=test_result)` |
| Deploying | `task_update(phase=deploying)` |
| Complete | `task_complete` with deploy URL |
| Failed | `task_failed` with error |

---

## 2. Evidence Attachments

### Evidence Model

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Evidence {
    /// Type of evidence: test_result, code_review, architecture_doc, deploy_log, artifact_link
    pub evidence_type: String,

    /// Human-readable summary
    pub summary: String,

    /// URL to the full evidence (optional)
    pub url: Option<String>,

    /// Content hash for integrity verification (optional)
    pub hash: Option<String>,

    /// Inline content for small evidence (< 4KB)
    pub raw: Option<String>,
}
```

### Wire Format

```
@+freeq.at/event=evidence_attach;+freeq.at/ref=01JQXYZ;+freeq.at/evidence-type=test_result;+freeq.at/sig=<sig> TAGMSG #factory :{"summary":"12/12 passed","url":"https://ci.example.com/run/123","hash":"sha256:abc"}
PRIVMSG #factory :✅ [qa] Evidence attached: 12/12 tests passed — https://ci.example.com/run/123
```

### Storage

Evidence is stored as coordination events with `event_type = evidence_attach`. The `payload_json` contains the full `Evidence` struct. The REST endpoint `/api/v1/tasks/{id}` aggregates all evidence for a task.

### Evidence Types for Factory Demo

| Phase | Evidence Type | Content |
|---|---|---|
| Specifying | `spec_document` | The requirements/spec text generated by the product agent |
| Designing | `architecture_doc` | Architecture decisions, stack choices, file structure |
| Building | `file_manifest` | List of files created with line counts |
| Reviewing | `code_review` | Review comments, issues found, approval status |
| Testing | `test_result` | Test output: passed/failed counts, failure details |
| Deploying | `deploy_log` | Deploy output, URL, timing |

### Bot Changes

**File**: `freeq-bots/src/factory/orchestrator.rs`

After each phase completes, attach evidence:

```rust
// After specifying:
handle.attach_evidence(&channel, &task_id, Evidence {
    evidence_type: "spec_document".into(),
    summary: "Requirements clarified".into(),
    raw: Some(spec_text.clone()),
    ..Default::default()
}).await?;

// After designing:
handle.attach_evidence(&channel, &task_id, Evidence {
    evidence_type: "architecture_doc".into(),
    summary: format!("Stack: {}, {} files planned", stack_name, file_count),
    raw: Some(architecture_text.clone()),
    ..Default::default()
}).await?;

// After testing:
handle.attach_evidence(&channel, &task_id, Evidence {
    evidence_type: "test_result".into(),
    summary: format!("{}/{} tests passed", passed, total),
    raw: Some(test_output.clone()),
    ..Default::default()
}).await?;

// After deploying:
handle.attach_evidence(&channel, &task_id, Evidence {
    evidence_type: "deploy_log".into(),
    summary: format!("Deployed to {}", deploy_url),
    url: Some(deploy_url.clone()),
    raw: Some(deploy_output.clone()),
    ..Default::default()
}).await?;
```

---

## 3. Audit Timeline

### Server Changes

**File**: `freeq-server/src/web.rs`

New endpoint:
```
GET /api/v1/channels/{name}/audit
    ?actor=did:plc:xxx
    ?type=governance,coordination,capability,membership
    ?since=2026-03-01T00:00:00Z
    ?until=2026-03-12T00:00:00Z
    ?limit=200

→ {
    events: [
      {
        timestamp: "2026-03-11T20:00:00Z",
        category: "membership",
        event: "join",
        actor_did: "did:plc:abc",
        actor_name: "factory",
        details: { actor_class: "agent" }
      },
      {
        timestamp: "2026-03-11T20:00:05Z",
        category: "capability",
        event: "granted",
        actor_did: "did:plc:abc",
        actor_name: "factory",
        details: { capability: "post_message", ttl: 3600, granted_by: "did:plc:xxx" }
      },
      {
        timestamp: "2026-03-11T20:01:00Z",
        category: "coordination",
        event: "task_request",
        actor_did: "did:plc:abc",
        actor_name: "factory",
        details: { task_id: "01JQXYZ", description: "Build a todo app" },
        signature: "..."
      },
      {
        timestamp: "2026-03-11T20:03:00Z",
        category: "coordination",
        event: "evidence_attach",
        actor_did: "did:plc:abc",
        actor_name: "factory",
        details: { ref_id: "01JQXYZ", evidence_type: "test_result", summary: "12/12 passed" },
        signature: "..."
      },
      {
        timestamp: "2026-03-11T20:03:30Z",
        category: "governance",
        event: "pause",
        actor_did: "did:plc:abc",
        actor_name: "factory",
        details: { issued_by: "did:plc:xxx", issuer_name: "chad" }
      }
    ]
  }
```

This aggregates from multiple tables:
- `coordination_events` — task/evidence/status events
- `governance_log` — pause/resume/revoke
- `agent_capability_grants` — capability changes
- `messages` — join/part/quit (existing)

### Web Client Changes

**New file**: `freeq-app/src/components/AuditTimeline.tsx`

A timeline view accessible from the channel settings panel ("Audit" tab):

```
┌─────────────────────────────────────────────────────────┐
│ 📋 Audit Timeline — #factory                            │
│ Filter: [All actors ▼] [All types ▼] [Last 24h ▼]      │
├─────────────────────────────────────────────────────────┤
│ 20:00:00  🤖 factory joined #factory                    │
│           actor_class: agent, provenance: freeq-bots    │
│                                                         │
│ 20:00:05  🔑 factory granted: post_message (1h TTL)     │
│           by: chad                                      │
│                                                         │
│ 20:01:00  📋 factory created task: Build a todo app     │
│           task: 01JQXYZ  🔒 signed                      │
│                                                         │
│ 20:01:30  📝 factory → specifying                       │
│           "Clarifying requirements"                     │
│           📎 Evidence: spec_document                     │
│              [Expand to view spec text]                  │
│                                                         │
│ 20:02:00  🏗 factory → designing                        │
│           "Chose React + Express stack"                 │
│           📎 Evidence: architecture_doc                  │
│              [Expand to view architecture]               │
│                                                         │
│ 20:02:30  🔨 factory → building                         │
│           📎 Evidence: file_manifest (8 files)           │
│                                                         │
│ 20:02:45  🔍 factory → reviewing                        │
│           📎 Evidence: code_review (no issues)           │
│                                                         │
│ 20:03:00  🧪 factory → testing                          │
│           📎 Evidence: test_result (12/12 passed) 🔒    │
│              [Expand to view test output]               │
│                                                         │
│ 20:03:15  🚀 factory → deploying                        │
│                                                         │
│ 20:03:30  ✅ factory completed task: Build a todo app   │
│           URL: https://todo-app.miren.dev               │
│           Duration: 2m 30s  🔒 signed                   │
│                                                         │
│ 20:04:00  ⏸ factory paused by chad                      │
│                                                         │
│ 20:04:15  ▶ factory resumed by chad                     │
└─────────────────────────────────────────────────────────┘
```

Features:
- 🔒 badge on signed events (click to verify signature)
- Expandable evidence sections
- Filter by actor, event type, time range
- Export as JSON for external audit

**New file**: `freeq-app/src/components/TaskTimeline.tsx`

A focused view for a single task, shown inline in the chat when clicking a task reference:

```
┌─────────────────────────────────────────────────┐
│ 📋 Task: Build a todo app (01JQXYZ)             │
│ Status: ✅ Complete                              │
│ Agent: 🤖 factory                                │
│ Duration: 2m 30s                                 │
│ URL: https://todo-app.miren.dev                  │
├─────────────────────────────────────────────────┤
│ ✅ specifying  → ✅ designing → ✅ building      │
│ → ✅ reviewing → ✅ testing → ✅ deploying       │
├─────────────────────────────────────────────────┤
│ Evidence (5 items)                               │
│  📄 spec_document                                │
│  📐 architecture_doc                             │
│  📁 file_manifest (8 files)                      │
│  🧪 test_result (12/12 passed)                   │
│  🚀 deploy_log                                   │
└─────────────────────────────────────────────────┘
```

### Message List Integration

**File**: `freeq-app/src/components/MessageList.tsx`

When rendering a PRIVMSG that has `+freeq.at/event` tags, render it as a structured card instead of plain text:

```tsx
function CoordinationEventCard({ message }: { message: Message }) {
  const eventType = message.tags?.['freeq.at/event'];
  const taskId = message.tags?.['freeq.at/ref'] || message.tags?.['freeq.at/task-id'];

  if (eventType === 'task_request') {
    return <TaskRequestCard taskId={taskId} description={message.text} />;
  }
  if (eventType === 'task_update') {
    return <TaskUpdateCard taskId={taskId} phase={message.tags?.['freeq.at/phase']} text={message.text} />;
  }
  if (eventType === 'evidence_attach') {
    return <EvidenceCard taskId={taskId} evidenceType={message.tags?.['freeq.at/evidence-type']} text={message.text} />;
  }
  if (eventType === 'task_complete') {
    return <TaskCompleteCard taskId={taskId} text={message.text} />;
  }
  // fallback: render as plain text
  return <PlainMessage message={message} />;
}
```

---

## 4. S2S Federation

### New S2S Message

**File**: `freeq-server/src/s2s.rs`

```rust
S2sMessage::CoordinationEvent {
    channel: String,
    event: CoordinationEvent,
}
```

Receiving server stores the event and relays the TAGMSG + PRIVMSG to local channel members. Signatures are preserved (they're from the originating agent's key, not the relay server's).

---

## Demo Script

### Prerequisites
- freeq-server with Phase 1–3 changes
- freeq-bots with coordination event support
- freeq-app with audit timeline and event card rendering
- irssi for legacy client comparison

### Steps

1. **Connect all clients to `#factory`**:
   - Web client (authenticated, channel op)
   - Factory bot (authenticated agent)
   - irssi (guest)

2. **Trigger a build**: `factory: build a todo app with user accounts`

3. **Watch the factory work** in real time:
   - **Web client**: messages appear as structured event cards with phase indicators, progress, and evidence attachments. The task timeline widget shows progress through each phase.
   - **irssi**: sees readable text: "📋 New task: Build a todo app...", "🏗 [architect] Designing: React + Express...", "✅ [qa] Tests: 12/12 passed", "🎉 Task complete: https://todo-app.miren.dev"

4. **Open the audit timeline** (web client → channel settings → Audit tab):
   - See every event in chronological order
   - Filter by "factory" to see only the bot's actions
   - Expand evidence items to see spec text, architecture decisions, test output
   - Verify signatures on signed events (🔒 badge)

5. **Click a task reference** in chat to see the focused task timeline:
   - Phase progression with checkmarks
   - All evidence items linked
   - Total duration and result

6. **Export audit log** as JSON for external review.

### What This Proves
- Agent work is structured and traceable, not just chat noise.
- Evidence is attached to specific tasks and phases.
- Every important action is cryptographically signed.
- Audit trails answer "what did this agent do and why" from a single view.
- Legacy clients still see everything as readable text.
- The factory bot's existing multi-agent pipeline maps naturally to the coordination model.

---

## External Demo Dependencies

### GitHub Integration for Evidence

For a more compelling demo, the factory bot could also:
1. Create a GitHub repo for the project.
2. Push commits at each phase.
3. Attach commit SHAs as evidence (`evidence_type: "commit"`).
4. Open a PR for the final code and attach the PR URL.

This uses the existing `tools::shell()` in `freeq-bots` (git commands) and would add GitHub API calls for repo creation and PR opening. ~50 lines of new tool code.

### Miren Deploy URL as Evidence

The factory bot already deploys to Miren. The deploy URL becomes evidence:
```rust
Evidence {
    evidence_type: "deploy_log",
    summary: format!("Deployed to {}", url),
    url: Some(url),
    hash: None,
    raw: Some(deploy_output),
}
```

No changes to Miren needed — just capture the output that's already there.

### Test Runner Evidence

The factory bot already runs tests via `tools::shell()`. Capture stdout as evidence:
```rust
let test_output = tools::shell(&workspace, "npm test 2>&1", 60).await?;
handle.attach_evidence(&channel, &task_id, Evidence {
    evidence_type: "test_result",
    summary: parse_test_summary(&test_output),
    raw: Some(test_output),
    ..Default::default()
}).await?;
```
