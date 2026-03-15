# Agent Context Persistence

## Problem

Every time an LLM-backed agent restarts, it loses all conversational context. The IRC channel has perfect history — but the agent starts with a blank slate.

## Solution: Tiered Context Assembly

The `freeq_bots::context` module provides layered context persistence:

```
┌─────────────────────────────────────────┐
│ Tier 0: Identity & Config               │  ~500 tokens
│ (DID, role, channel list, system prompt) │  Always loaded
├─────────────────────────────────────────┤
│ Tier 1: Structured Facts & Decisions     │  ~1-3K tokens
│ (from Memory store)                      │  Always loaded
├─────────────────────────────────────────┤
│ Tier 2: Latest Summary                   │  ~500-1K tokens
│ (compressed conversation state)          │  Always loaded
├─────────────────────────────────────────┤
│ Tier 3: Recent Raw Messages              │  ~2-5K tokens
│ (CHATHISTORY since last summary)         │  Always loaded
├─────────────────────────────────────────┤
│ Tier 4: On-Demand Retrieval              │  ~0-10K tokens
│ (search older history when referenced)   │  Loaded on demand
└─────────────────────────────────────────┘
Total baseline: ~5-10K tokens (out of 200K)
```

## Quick Start

```rust
use freeq_bots::context::{AgentContext, AgentIdentity, ContextConfig, HistoryMessage};
use freeq_bots::memory::Memory;

// 1. Open persistent memory
let memory = Arc::new(Memory::open(Path::new("mybot.db"))?);

// 2. Define identity
let identity = AgentIdentity {
    nick: "mybot".into(),
    did: Some("did:web:example.com:bots:mybot".into()),
    role: "A helpful assistant".into(),
    channels: vec!["#general".into()],
    system_prompt: Some("You are helpful and concise.".into()),
};

// 3. Create context manager
let ctx = AgentContext::new(identity, memory, ContextConfig::default());

// 4. On connect: fetch history
ctx.fetch_and_ingest(&handle, &mut events, "#general").await?;

// 5. Before LLM calls: assemble context
let system_prompt = ctx.assemble("#general", None).await;
let response = llm.complete(&system_prompt, &user_message).await?;

// 6. Record messages
ctx.record_message("#general", HistoryMessage { ... }).await;

// 7. Periodically: summarize
ctx.maybe_summarize("#general", None, &llm).await?;

// 8. After significant exchanges: extract facts
ctx.extract_facts("#general", None, &exchange, &llm).await?;
```

## Demo Bot

```bash
ANTHROPIC_API_KEY=sk-... cargo run --release --bin context-bot -- \
  --server 127.0.0.1:6667 --channel '#test' --nick contextbot
```

### Special Commands

- `contextbot: summarize` — Force a conversation summary
- `contextbot: context` — Show context stats (token estimate, message count)
- `contextbot: remember <fact>` — Explicitly store a fact
- `contextbot: forget <key>` — Remove a stored fact
- `contextbot: status` — Same as `context`

### What Persists Across Restarts

1. **Structured facts** — Decisions, preferences, action items (SQLite)
2. **Rolling summaries** — Compressed conversation state (SQLite)
3. **Recent messages** — Fetched from server via CHATHISTORY on reconnect

### What Doesn't Persist

- The exact LLM conversation history (message pairs) — too large
- Ephemeral state like "the user seems frustrated" — would need explicit storage

## Integration with Pi Bridge

The pi bridge can use context export to maintain a context file:

```rust
// After each significant exchange:
ctx.export_context_file("#pi-control", None, Path::new("/tmp/freeq-context-pi-control.md")).await?;
```

This writes a human-readable markdown file with all assembled context. Pi can read this file as part of its session context.

## Architecture Decisions

### Why not just replay full history?

Context windows are large (200K tokens) but not infinite. Raw IRC history is noisy — join/part messages, off-topic chat, repeated questions. Summaries compress 50 messages (~5K tokens) into ~500 tokens while preserving decisions and intent.

### Why structured facts AND summaries?

They serve different purposes:
- **Summaries** capture narrative flow and open threads — good for "what were we talking about?"
- **Facts** capture specific, reusable knowledge — good for "what stack did we choose?"

Summaries are lossy (each generation may lose nuance). Facts are precise but require extraction.

### Why SQLite for memory?

- Already a dependency (Memory module existed)
- Survives restarts (unlike in-memory)
- Fast enough for this use case
- Human-inspectable (`sqlite3 mybot.db "SELECT * FROM memory"`)
- No external services required

### Why not vector embeddings / RAG?

Premature for the current scale. When FTS5 lands (P2.5 TODO), keyword search over history will cover 90% of retrieval needs. True semantic search with embeddings is a future enhancement.

## Token Budget Estimates

| Tier | Content | Typical Size |
|------|---------|-------------|
| 0 | Identity, role, system prompt | ~200-500 tokens |
| 1 | 20 decisions + 20 facts + 10 action items | ~1-2K tokens |
| 2 | Rolling summary | ~300-800 tokens |
| 3 | 50 recent messages | ~3-5K tokens |
| **Total baseline** | | **~5-8K tokens** |

This leaves 190K+ tokens for the actual conversation, tool use, etc.

## Files

- `freeq-bots/src/context.rs` — Core context module
- `freeq-bots/src/memory.rs` — SQLite-backed key-value memory store
- `freeq-bots/src/bin/context_bot.rs` — Demo context-aware bot
- `docs/AGENT-CONTEXT.md` — This document
