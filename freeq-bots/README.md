# freeq-bots

AI agent bots that do real, observable work in IRC channels.

## Bots

### 🏭 Software Factory (`/factory`)
A multi-agent development team that builds software collaboratively in a channel. Agent roles:
- **Product Lead** — clarifies requirements, writes spec
- **Architect** — proposes design and stack
- **Builder** — writes code using real tools
- **Reviewer** — critiques quality and spec alignment
- **QA** — generates and runs tests
- **Deploy** — deploys to staging with live URL

### 🔍 Architecture Auditor (`/audit`)
Clones a GitHub repo, analyzes structure, and posts findings: system diagram, bottlenecks, coupling risks, and refactor suggestions.

### ⚡ Spec-to-Prototype (`/prototype`)
Drop in a product spec, get a deployed application back in minutes. From idea → live URL.

## Running

```bash
# Set your API key
export ANTHROPIC_API_KEY=sk-ant-...

# Run the bot
cargo run --release --bin freeq-bots -- \
  --server irc.freeq.at:6667 \
  --nick factory \
  --channel "#factory"
```

## Commands

| Command | Description |
|---------|-------------|
| `/factory build <spec>` | Start the full factory pipeline |
| `/factory status` | Current factory phase and project |
| `/factory pause` | Pause the pipeline |
| `/factory resume` | Resume the pipeline |
| `/factory spec` | Show the current project spec |
| `/factory files` | List generated project files |
| `/audit <repo-url>` | Architecture audit of a GitHub repo |
| `/prototype <spec>` | Quick spec → deployed prototype |
| `/help` | List all commands |

## Architecture

```
freeq-bots/
├── src/
│   ├── main.rs          # IRC event loop, command routing
│   ├── lib.rs           # Module exports
│   ├── llm.rs           # Claude API client with tool use
│   ├── memory.rs        # SQLite-backed project memory
│   ├── tools.rs         # Real tools: filesystem, shell, miren deploy
│   ├── output.rs        # IRC message formatting per agent role
│   ├── factory/         # Multi-agent software factory
│   ├── auditor/         # Architecture audit bot
│   └── prototype/       # Spec-to-prototype bot
```

## Requirements

- `ANTHROPIC_API_KEY` — Claude API access
- `miren` CLI — for deployment (optional, deploy tool will fail gracefully)
- freeq server — any freeq or standard IRC server

## How It Works

1. Bot connects to IRC as a single user
2. Commands trigger agent pipelines
3. Agents use Claude with tool-use to write code, run commands, deploy
4. All work is posted to the channel in real-time with role prefixes: `[builder]`, `[architect]`, etc.
5. Project state persists in SQLite memory across sessions
