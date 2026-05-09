# Plan: Owner-Driven IRC Actions via Per-Dispatch Capability Tokens

## Goal

Owner DMs the bot a natural-language request like *"join #foo and post hi"* and the daemon **actually performs** the IRC actions (JOIN #foo, then PRIVMSG #foo :hi). Allowlisted (non-owner) DIDs may chat with the bot but **must not** be able to drive IRC actions — only the owner authorizes side effects.

## Design at a glance

```
              owner DM
                  │
                  ▼
         ┌──────────────────┐
         │  daemon          │   handleDm() mints a UUID token
         │                  │   stores {isOwner, replyTarget, expiresAt}
         │  ControlSocket   │   spawns claude -p with env:
         │  ~/.freeqcc/     │     FREEQCC_CONTROL_SOCK
         │  control.sock    │     FREEQCC_DISPATCH_TOKEN
         │  (mode 0600)     │     FREEQCC_DISPATCH_IS_OWNER (1 only for owner)
         └──────────────────┘     FREEQCC_DISPATCH_REPLY_TARGET
                  ▲
        one-line │ JSON {token, action, args}
        one-line │ JSON {ok, ...}
                  ▼
         ┌──────────────────┐
         │  claude -p       │   Knows about `freeqcc send <action>` via
         │  subprocess      │   --append-system-prompt. Uses Bash tool.
         └──────────────────┘
                  │
                  ▼
         ┌──────────────────┐
         │  freeqcc send    │   reads env, opens sock, writes one line,
         │  CLI subcommand  │   reads one line, exit 0/non-zero
         └──────────────────┘
```

## Authorization

- **Per-dispatch token**: `handleDm` mints a fresh UUID per inbound user message. Stored in-memory: `Map<token, {isOwner, replyTarget, expiresAt}>`.
- **Token lifecycle**: alive while the dispatch is in flight. Expired by `dispatch.ts onComplete`/`onError`. Hard TTL of 10 minutes as a safety net if callbacks miss.
- **Owner gate**: `IS_OWNER=1` only when `senderDid === owner.did`. Allowlisted DIDs get a token with `IS_OWNER=0` so future read-only actions can be granted to them without rewriting the wire format. Today, every action is owner-only.

## Wire format

- Daemon binds `~/.freeqcc/control.sock` (mode 0600). Unlinks any stale socket on startup. Removes on SIGINT/SIGTERM/exit.
- One request per connection: a single line of JSON.
  ```
  {"token": "<uuid>", "action": "<name>", "args": ["..."]}
  ```
- One reply per connection: a single line of JSON.
  ```
  {"ok": true}                       // for success-only actions
  {"ok": true, "result": ...}        // when the action returns data
  {"ok": false, "error": "..."}      // failure
  ```

## Action vocabulary (v1)

| action   | args                       | owner-only | effect |
|----------|----------------------------|------------|--------|
| `join`   | `channel` (string, "#…")   | yes        | `JOIN <channel>` |
| `part`   | `channel`, `reason?`       | yes        | `PART <channel> :<reason>` |
| `privmsg`| `target`, `text`           | yes        | `PRIVMSG <target> :<text>` |
| `notice` | `target`, `text`           | yes        | `NOTICE <target> :<text>` |
| `nick`   | `newnick`                  | yes        | `NICK <newnick>` |

Errors:
- Unknown action → `{ok:false, error:"unknown action: <name>"}`
- Token unknown/expired → `{ok:false, error:"invalid or expired token"}`
- Owner-only action with `IS_OWNER=0` → `{ok:false, error:"owner-only action"}`
- Bad args → `{ok:false, error:"bad args: <reason>"}`

## CLI: `freeqcc send <action> [args…]`

- Reads `FREEQCC_CONTROL_SOCK` and `FREEQCC_DISPATCH_TOKEN` from env. If either missing, exit 2 with a clear message.
- Connects to the sock, writes the JSON line, reads one line back.
- Exit 0 on `{ok:true}`, non-zero (1) on `{ok:false}` printing `error`.
- No retries. No daemon-spawn fallback. If the sock isn't there, the user is running outside a dispatch context and the call fails fast.

## Subprocess plumbing

In `dispatch.ts dispatchToClaudeStreaming`:

```ts
const env = {
  ...process.env,
  FREEQCC_CONTROL_SOCK: paths.controlSock,
  FREEQCC_DISPATCH_TOKEN: token,
  FREEQCC_DISPATCH_IS_OWNER: isOwner ? "1" : "0",
  FREEQCC_DISPATCH_REPLY_TARGET: replyTarget,
};
```

`--append-system-prompt` informs Claude about the tool. Short, factual; tells it:
- It can run `freeqcc send <action> [args]` to perform IRC actions on behalf of the user.
- Action vocabulary.
- That actions only work when `FREEQCC_DISPATCH_IS_OWNER=1`; otherwise reject politely.
- That `FREEQCC_DISPATCH_REPLY_TARGET` is where the reply is being delivered (so it shouldn't `privmsg` the same target with the reply text — the streaming pipeline already does that).

## Files

```
src/control.ts        — Unix socket server, token store, action dispatch
src/daemon.ts         — start ControlSocket, mint tokens in handleDm
src/dispatch.ts       — pass env, expire token in onComplete/onError
src/cli.ts            — `freeqcc send` subcommand
src/paths.ts          — add controlSock path
```

## Out of scope for this iteration

- Read-only actions for non-owners (e.g. `whoami`, `list-channels`). The plumbing supports it; the action table doesn't expose any yet.
- Persistent grants ("@friend can join channels for me"). v2.
- Audit log of every action invoked. Easy to add — append to `~/.freeqcc/actions.log` on each handled request.
- Rate-limiting at the action level (e.g. max 5 joins/min). Not needed in v1; cycle detection at the dispatch level is sufficient.
