# Role personas

Beings geared to a **role**, not a personality. Same shape as any revenant
being — a ghostly face + voice on a freeq tile — but the brain is **Claude Code**
and the value is the *work it does and shows*, using the rich tile views
(`freeq_show_diff` / `_chart` / `_status_grid` / `_agenda`).

## The shape of a role persona

```
Claude Code (brain)
  ├─ freeq-claude-mcp        ← the AV bridge (this crate): join, listen, say,
  │                            show_diff/chart/status_grid/agenda, look
  ├─ role tools              ← what the role connects to (per role, below)
  └─ role skill (roles/*.md) ← how it behaves + which tile view to use when
```

A role persona is just Claude Code launched with:
1. the **freeq-claude-mcp** MCP server (built from this crate),
2. the role's **extra MCP servers / CLIs** (the connectors), and
3. the role **skill** loaded (`roles/<name>.md`) so the brain knows its job.

On a boxd VM this slots in where `freeq-eliza` runs today: the supervisor
launches `claude` (Code) with these instead of the Rust agent.

## The four roles

| Role | Skill | Connects to | Shows on tile | Cred needed |
|---|---|---|---|---|
| **Ada** (programmer) | `ada.md` | a git checkout + push | `freeq_show_diff`, test status grid | `gh auth` / deploy key |
| **Sentinel** (on-call) | `sentinel.md` | boxd, freeq/reth health, CI | status grid, metric chart | `boxd login`, `gh auth`, health URLs |
| **Quant** (markets) | `quant.md` | a market-data API | chart, watchlist heat-grid | market-data API key |
| **Otto** (chief of staff) | `otto.md` | Gmail + Google Calendar | agenda, triage card | Google OAuth (gmail/gcal MCP) |

Face + voice for each are in the skill frontmatter (`face:` / `voice:`) — Ada
wears eliza, Sentinel oblivion, Quant utopia, Otto narrator. They're roles, so
the *character* is just a recognizable look; swap freely.

## Launching one (sketch)

```sh
# 1. build the bridge
cargo build --release -p freeq-claude-mcp
# 2. register it + the role's connectors with Claude Code
claude mcp add freeq-claude  "$PWD/target/release/freeq-claude-mcp"
claude mcp add gmail         <gmail-mcp-server>          # Otto
claude mcp add google-cal    <gcal-mcp-server>           # Otto
# (Ada/Sentinel/Quant use Claude Code's own Bash + gh/boxd/curl)
# 3. point Claude Code at the role skill, then in-session:
#    "load the ada skill, freeq_connect to #dev, and get to work"
```

The **demo that sells the whole thing**: put two or three of these in one
channel and orchestrate by voice — "Ada, ship the fix; Sentinel, watch the
deploy; Quant, how's the market while we wait." Three working agents, three
live tiles. That's what makes it amazing — the tiles are doing real work.

## What's left to go fully live

The skills + views are done and tested. Going live per role is wiring its
connector + dropping in the one cred above, then launching on a boxd VM. Do
them in feasibility order: **Ada** (no external cred — `gh` already on the box),
then **Sentinel** (boxd/gh you already have), then **Quant** (one API key),
then **Otto** (Google OAuth app).
