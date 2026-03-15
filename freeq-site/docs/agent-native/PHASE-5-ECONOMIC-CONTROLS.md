# Phase 5: Economic Controls — Detailed Implementation Plan

**Goal**: Channels can set budgets for agent activity. Spend is tracked, visible, and governed. High-cost actions require explicit approval.

**Demo**: `#factory` has a daily budget of $50 for agent API calls. The factory bot builds a project — each LLM call and tool invocation is metered. The web client shows a budget gauge filling up in the channel panel. At 80%, the sponsor (chad) gets a DM warning. When the factory bot tries to start a second build that would exceed the budget, it transitions to "blocked on budget" and posts a notice explaining why. A channel op approves a $25 budget increase. The bot resumes. An irssi user sees all of this as readable notices ("💰 factory has used $41.20 / $50.00 today", "⚠ Budget 80% used", "🛑 factory blocked: daily budget exceeded").

---

## 1. Budget Model

### Types

**File**: `freeq-server/src/policy/types.rs`

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BudgetPolicy {
    /// Budget currency/unit. Could be "usd", "credits", "api_calls", "tokens".
    pub unit: String,

    /// Maximum amount per period.
    pub max_amount: f64,

    /// Period: "per_hour", "per_day", "per_week", "per_task".
    pub period: BudgetPeriod,

    /// DID of the budget sponsor (who gets notified and pays).
    pub sponsor_did: String,

    /// Threshold (0.0–1.0) at which to warn the sponsor.
    #[serde(default = "default_warn_threshold")]
    pub warn_threshold: f64,

    /// Whether exceeding the budget blocks the agent or just warns.
    #[serde(default = "default_hard_limit")]
    pub hard_limit: bool,

    /// Per-action cost threshold that triggers spend approval.
    #[serde(default)]
    pub approval_threshold: Option<f64>,
}

fn default_warn_threshold() -> f64 { 0.8 }
fn default_hard_limit() -> bool { true }

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum BudgetPeriod {
    PerHour,
    PerDay,
    PerWeek,
    PerTask,
}
```

Add to `PolicyDocument`:
```rust
/// Budget constraints for agent activity in this channel.
#[serde(default, skip_serializing_if = "Option::is_none")]
pub agent_budget: Option<BudgetPolicy>,

/// Per-agent budget overrides (DID → budget).
#[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
pub agent_budgets: BTreeMap<String, BudgetPolicy>,
```

### Spend Tracking

**New SQLite table**:
```sql
CREATE TABLE IF NOT EXISTS agent_spend (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    channel TEXT NOT NULL,
    agent_did TEXT NOT NULL,
    amount REAL NOT NULL,
    unit TEXT NOT NULL,
    description TEXT,        -- "claude-sonnet-4-20250514 API call: 1.2k tokens"
    task_ref TEXT,           -- optional task reference
    timestamp INTEGER NOT NULL
);

CREATE INDEX idx_spend_channel_agent ON agent_spend(channel, agent_did, timestamp);
CREATE INDEX idx_spend_period ON agent_spend(channel, agent_did, unit, timestamp);
```

---

## 2. Spend Reporting

### Wire Format

Agents report their spend via a new tag on messages or via a dedicated command:

```
@+freeq.at/spend=0.03;+freeq.at/spend-unit=usd;+freeq.at/spend-desc=claude-sonnet-4-20250514:1.2k-tokens PRIVMSG #factory :🏗 [architect] Designed the component structure
```

Or as a dedicated command for batch reporting:
```
SPEND #factory :amount=0.03;unit=usd;desc=claude-sonnet-4-20250514 API call: 1.2k tokens;task=01JQXYZ
```

### Server Processing

**File**: `freeq-server/src/connection/messaging.rs`

When processing a message with `+freeq.at/spend`:
```rust
if let Some(spend_str) = tags.get("+freeq.at/spend") {
    let amount: f64 = spend_str.parse()?;
    let unit = tags.get("+freeq.at/spend-unit").unwrap_or("usd");
    let desc = tags.get("+freeq.at/spend-desc");

    // Record spend
    db.execute("INSERT INTO agent_spend (channel, agent_did, amount, unit, description, task_ref, timestamp) VALUES (?, ?, ?, ?, ?, ?, ?)",
        [&channel, &session.did, amount, unit, desc, task_ref, now]);

    // Check budget
    let budget = get_budget_for_agent(state, &channel, &session.did);
    if let Some(budget) = budget {
        let period_start = budget_period_start(&budget.period);
        let total_spent: f64 = db.query_scalar(
            "SELECT COALESCE(SUM(amount), 0) FROM agent_spend WHERE channel = ? AND agent_did = ? AND unit = ? AND timestamp >= ?",
            [&channel, &session.did, &budget.unit, period_start]
        );

        let ratio = total_spent / budget.max_amount;

        // Warn at threshold
        if ratio >= budget.warn_threshold && (ratio - amount / budget.max_amount) < budget.warn_threshold {
            // First time crossing threshold this period
            send_dm(state, &budget.sponsor_did, &format!(
                "⚠ Agent {} in {} has used {:.0}% of budget ({:.2}/{:.2} {})",
                session.nick, channel, ratio * 100.0, total_spent, budget.max_amount, budget.unit
            ));
            broadcast_to_channel(state, &channel, &format!(
                ":server NOTICE {} :⚠ Budget {:.0}% used ({:.2}/{:.2} {})",
                channel, ratio * 100.0, total_spent, budget.max_amount, budget.unit
            ));
        }

        // Block at limit
        if ratio >= 1.0 && budget.hard_limit {
            set_agent_presence(state, &session.did, PresenceState::BlockedOnBudget,
                Some(&format!("Daily budget exceeded: {:.2}/{:.2} {}", total_spent, budget.max_amount, budget.unit)));

            broadcast_to_channel(state, &channel, &format!(
                ":server NOTICE {} :🛑 {} blocked: {} budget exceeded ({:.2}/{:.2} {})",
                channel, session.nick, budget.period, total_spent, budget.max_amount, budget.unit
            ));

            // Notify agent via governance signal
            send_to_session(&session, &format!(
                "@+freeq.at/governance=budget_exceeded TAGMSG {} :budget_exceeded;spent={:.2};limit={:.2};unit={}",
                session.nick, total_spent, budget.max_amount, budget.unit
            ));
        }
    }
}
```

### SDK Changes

**File**: `freeq-sdk/src/client.rs`

```rust
/// Report spend for the current action.
pub async fn report_spend(&self, channel: &str, amount: f64, unit: &str, description: &str, task_ref: Option<&str>) -> Result<()>;

/// Check remaining budget before starting expensive work.
pub async fn check_budget(&self, channel: &str) -> Result<BudgetStatus>;

#[derive(Debug, Clone)]
pub struct BudgetStatus {
    pub unit: String,
    pub spent: f64,
    pub limit: f64,
    pub remaining: f64,
    pub period: BudgetPeriod,
    pub blocked: bool,
}

/// Callback when budget is exceeded.
pub fn on_budget_exceeded(&self, handler: impl Fn(BudgetExceededInfo) + Send + 'static);
```

### Bot Changes (Demo Code)

**File**: `freeq-bots/src/llm.rs`

After each LLM API call, report spend:
```rust
pub async fn chat(&self, messages: &[Message], tools: Option<&[ToolDef]>) -> Result<Response> {
    let response = self.client.post(&self.api_url)
        .json(&request)
        .send().await?;

    let body: ApiResponse = response.json().await?;

    // Calculate cost from token usage
    let input_cost = body.usage.input_tokens as f64 * self.input_price_per_token;
    let output_cost = body.usage.output_tokens as f64 * self.output_price_per_token;
    let total_cost = input_cost + output_cost;

    // Report spend to the channel
    if let Some(ref freeq_handle) = self.freeq_handle {
        freeq_handle.report_spend(
            &self.channel,
            total_cost,
            "usd",
            &format!("{}: {}in/{}out tokens", self.model,
                body.usage.input_tokens, body.usage.output_tokens),
            self.current_task.as_deref(),
        ).await?;
    }

    Ok(body.into())
}
```

**File**: `freeq-bots/src/factory/orchestrator.rs`

Before starting a build, check budget:
```rust
let budget = handle.check_budget(&channel).await?;
if budget.blocked {
    output::status(handle, &channel, &system_agent(), "🛑",
        &format!("Cannot start build: budget exceeded ({:.2}/{:.2} {})",
            budget.spent, budget.limit, budget.unit)).await?;
    return Ok(());
}
if budget.remaining < 5.0 {
    output::status(handle, &channel, &system_agent(), "⚠",
        &format!("Low budget: {:.2} {} remaining. Build may be interrupted.",
            budget.remaining, budget.unit)).await?;
}
```

Handle budget_exceeded governance signal:
```rust
GovernanceSignal::BudgetExceeded { spent, limit, unit } => {
    tracing::warn!("Budget exceeded: {spent}/{limit} {unit}");
    factory.pause();
    handle.set_presence(PresenceState::BlockedOnBudget,
        Some(&format!("Budget exceeded: {:.2}/{:.2} {}", spent, limit, unit)),
        None).await?;
}
```

---

## 3. Spend Approval

### Model

For actions whose estimated cost exceeds `approval_threshold`, the agent must get approval first.

### Wire Format

```
APPROVAL_REQUEST #factory :deploy;estimated_cost=12.50;currency=usd;description=Deploy to production (includes CI/CD pipeline)
```

Server sends to ops:
```
:server NOTICE #factory :💰 factory requests spend approval: deploy ($12.50 USD) — Deploy to production. Use: APPROVAL_GRANT factory :deploy
```

### SDK Changes

```rust
/// Request spend approval for an expensive action.
pub async fn request_spend_approval(&self, channel: &str, action: &str, estimated_cost: f64, unit: &str, description: &str) -> Result<()>;
```

This reuses the existing approval flow from Phase 2, with the addition of cost information in the approval request.

---

## 4. Budget REST API

**File**: `freeq-server/src/web.rs`

```
GET /api/v1/channels/{name}/budget
→ {
    policy: { unit: "usd", max_amount: 50.0, period: "per_day", sponsor_did: "...", warn_threshold: 0.8, hard_limit: true },
    current_period: {
      start: "2026-03-11T00:00:00Z",
      end: "2026-03-12T00:00:00Z",
      total_spent: 23.40,
      remaining: 26.60,
      percent_used: 46.8,
      by_agent: {
        "did:plc:factory": { spent: 20.10, items: 45 },
        "did:plc:auditor": { spent: 3.30, items: 12 }
      }
    }
  }

GET /api/v1/channels/{name}/spend
    ?agent=did:plc:xxx
    ?since=2026-03-11T00:00:00Z
    ?limit=100
→ [{ amount, unit, description, task_ref, timestamp }]

POST /api/v1/channels/{name}/budget/increase
Body: { additional_amount: 25.0, reason: "Need to finish the current build" }
Auth: sponsor_did or channel op
→ { new_limit: 75.0, period: "per_day" }
```

---

## 5. Web Client Changes

### Budget Gauge

**New file**: `freeq-app/src/components/BudgetGauge.tsx`

A visual gauge shown in the channel header or sidebar when the channel has a budget policy:

```
┌─────────────────────────────────────┐
│ 💰 Budget: $23.40 / $50.00 today   │
│ ██████████░░░░░░░░░░░░  46.8%      │
│ factory: $20.10 | auditor: $3.30   │
└─────────────────────────────────────┘
```

Colors:
- Green: < 60%
- Yellow: 60–80%
- Orange: 80–95%
- Red: > 95%

Clicking opens a detailed spend breakdown.

### Spend Breakdown Panel

**New file**: `freeq-app/src/components/SpendBreakdown.tsx`

```
┌──────────────────────────────────────────────────┐
│ 💰 Spend Breakdown — #factory (Today)            │
├──────────────────────────────────────────────────┤
│ 🤖 factory — $20.10 (45 items)                   │
│   Task: Build todo app                            │
│   ├ claude-sonnet-4-20250514: 8.2k/3.1k tokens → $0.12     │
│   ├ claude-sonnet-4-20250514: 12.4k/5.6k tokens → $0.19    │
│   ├ ... (43 more)                                 │
│   └ Total: $20.10                                 │
│                                                   │
│ 🤖 auditor — $3.30 (12 items)                    │
│   Task: Audit chad/freeq                          │
│   └ Total: $3.30                                  │
├──────────────────────────────────────────────────┤
│ Budget: $50.00/day | Remaining: $26.60            │
│ Sponsor: chad | Warn at: 80%                      │
│ [📈 Increase Budget]                              │
└──────────────────────────────────────────────────┘
```

### Budget Notifications

When the server sends budget warnings, the web client shows toast notifications:
- ⚠ "Budget 80% used ($40.00 / $50.00)"
- 🛑 "factory blocked: daily budget exceeded"

---

## 6. S2S Federation

Budget policies are part of the channel's `PolicyDocument` and propagate via existing S2S policy sync. Spend tracking is local to each server (each server tracks spend for its local agents). Budget status is not federated — each server enforces independently against the same policy.

---

## Demo Script

### Setup

1. **Set channel budget**:
   ```bash
   curl -X POST http://localhost:8080/api/v1/channels/%23factory/policy \
     -H "Content-Type: application/json" \
     -d '{
       "agent_budget": {
         "unit": "usd",
         "max_amount": 50.0,
         "period": "per_day",
         "sponsor_did": "did:plc:4qsyxmnsblo4luuycm3572bq",
         "warn_threshold": 0.8,
         "hard_limit": true,
         "approval_threshold": 10.0
       }
     }'
   ```

2. **Factory bot** running with spend reporting enabled.

3. **Web client** open showing `#factory` with budget gauge visible.

### Steps

1. **Build a project**: `factory: build a weather dashboard`.

2. **Watch spend accumulate**: Budget gauge fills up as LLM calls happen. Each message from the bot has a small spend indicator: "💰 $0.15".

3. **Spend breakdown**: Click the budget gauge to see per-agent, per-task itemized costs.

4. **Warning at 80%**: When spend hits $40, toast notification appears. Chad gets a DM. irssi shows "⚠ Budget 80% used ($40.00 / $50.00)".

5. **Try another build**: `factory: build a blog`. Bot checks budget, sees $8.50 remaining, warns "⚠ Low budget: $8.50 remaining. Build may be interrupted."

6. **Budget exceeded mid-build**: Bot transitions to "blocked on budget". Presence shows 💰. Message in channel: "🛑 factory blocked: daily budget exceeded ($50.20 / $50.00)".

7. **Increase budget**: Op clicks "Increase Budget" in spend panel, adds $25. Bot resumes automatically, presence returns to "executing".

8. **Spend approval**: An action estimated at $12 triggers an approval popup before proceeding.

### What This Proves
- Agent activity has real economic visibility.
- Budgets are enforced — agents can't silently spend without limits.
- Sponsors are notified before budgets are exhausted.
- Budget increases are governed (op approval).
- High-cost actions require explicit human consent.
- All of this is visible to humans in real time, including on legacy clients.

---

## External Demo Dependencies

### LLM Cost Tracking

The factory bot needs token-to-cost conversion. Anthropic's API returns `usage.input_tokens` and `usage.output_tokens`. We need a price table:

```rust
fn cost_per_token(model: &str) -> (f64, f64) {
    match model {
        "claude-sonnet-4-20250514" => (3.0 / 1_000_000.0, 15.0 / 1_000_000.0),  // $3/M in, $15/M out
        "claude-opus-4-20250514" => (15.0 / 1_000_000.0, 75.0 / 1_000_000.0),
        _ => (3.0 / 1_000_000.0, 15.0 / 1_000_000.0), // default to sonnet pricing
    }
}
```

This already exists conceptually in the LLM client — just need to surface it.

### Budget Configuration UI

For the demo, budget is set via REST API (curl). A proper UI would be a "Budget" section in channel settings, but that's post-demo polish.
