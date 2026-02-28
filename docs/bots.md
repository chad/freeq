# Bot Framework

freeq includes a Rust bot framework for building IRC bots with AT Protocol identity.

## Quick start

See the [Bot Quickstart](/docs/bot-quickstart/) for a complete 10-minute tutorial.

## Architecture

```
Bot::new("!", "mybot")
  → command routing (prefix matching)
  → permission checks (Anyone / Authenticated / Admin)
  → rate limiting (per-user token bucket)
  → input size validation
  → handler receives CommandContext
```

## Features

- **Command routing** — Prefix-based dispatch with automatic help generation
- **Permissions** — `Anyone`, `Authenticated` (requires DID), `Admin` (specific DIDs)
- **Rate limiting** — Per-user token bucket with configurable window
- **Input caps** — Reject oversized arguments
- **Rich context** — Reply, react, thread, typing indicators from handlers
- **Reconnect** — `run_with_reconnect()` with exponential backoff and auto-rejoin
- **Fallback handler** — Catch non-command messages

## Command permissions

```rust
// Anyone can use
bot.command("ping", "Pong!", handler);

// Must be authenticated with a DID
bot.auth_command("whoami", "Show your DID", handler);

// Only admin DIDs
let bot = Bot::new("!", "mybot").admin("did:plc:abc123");
bot.admin_command("kick", "Kick a user", handler);
```

## Handler context

Every handler receives a `CommandContext` with:

- `ctx.reply("text")` — Send to channel or PM
- `ctx.reply_to("text")` — Reply with `nick: text`
- `ctx.reply_in_thread("text")` — Threaded reply
- `ctx.react("🔥")` — React to the triggering message
- `ctx.typing()` / `ctx.typing_done()` — Typing indicator
- `ctx.sender`, `ctx.sender_did`, `ctx.args`, `ctx.msgid()`

## Examples

Three examples in `freeq-sdk/examples/`:

- **`echo_bot.rs`** — Minimal (10 lines of logic)
- **`framework_bot.rs`** — Commands + permissions
- **`moderation_bot.rs`** — Full-featured: threads, reactions, rate limiting, admin commands, auto-reconnect

## Use cases

- **Moderation** — Auto-voice/op by DID, ban enforcement, spam filtering
- **Integrations** — GitHub CI reporter, webhook bridge, link unfurling
- **Knowledge** — FAQ responder, on-call rota, search
- **Ops** — Deploy notifications, health checks, metrics
