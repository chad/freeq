# freeqcc

> **freeq + Claude Code.** Drop one command into Claude Code, get a
> Bluesky-DM-controllable AI agent that runs on your laptop and is
> cryptographically yours.

```text
        ┌─────────────────────────┐         ┌──────────────────┐
        │  Bluesky DM (anywhere)  │ ──────→ │  irc.freeq.at    │
        │  @you → @your-agent     │         │  (PKI + routing) │
        └─────────────────────────┘         └────────┬─────────┘
                                                     │
                                            owner-DID-gated
                                                     │
                                                     ▼
                                       ┌─────────────────────────┐
                                       │  freeqcc daemon (Node)  │
                                       │  ed25519 did:key        │
                                       │  spawns claude -p       │
                                       └─────────────────────────┘
```

## What

A Node.js daemon that:

- generates its own `did:key` cryptographic identity (ed25519)
- connects to [freeq](https://freeq.at) via SASL ATPROTO-CHALLENGE
- declares a [`FreeqBotDelegation/v1`](#identity--pki) cert binding it to *your* AT Protocol DID
- accepts DMs **only from the DID that owns it** — strangers get a polite refusal once per hour, then silence
- dispatches owner DMs to a persistent Claude Code session via `claude -p --resume`
- replies through freeq, signed end-to-end

You can DM your agent from any freeq client (web, iOS, mobile, TUI) — or from Bluesky once Bluesky DM ↔ freeq DM bridging lands.

## Install

Prerequisites: Node 22+, the [Claude Code CLI](https://claude.ai/code) on PATH, an AT Protocol handle (Bluesky works).

```sh
git clone https://github.com/chad/freeq.git
cd freeq/freeqcc
npm install
npm link              # makes `freeqcc` available globally
```

Then:

```sh
freeqcc launch --detach
```

First-time launch prompts in your terminal for:

1. **Your AT Protocol handle** (e.g. `chadfowler.com`) — resolved via Bluesky's public PLC, persisted to `~/.freeqcc/owner.json`.
2. **A bot nick** — defaults to `<your-handle>-agent` but pick a name (`dev-buddy`, `code-helper`, `sourdough-bot`). It's more memorable, especially for HN screenshots.

The daemon backgrounds itself, prints the bot's PID + log path, and starts listening.

```sh
freeqcc status        # show live state (verified provenance, presence, etc.)
freeqcc stop          # clean QUIT, takes ~1s
freeqcc doctor        # sanity-check every config file + dependency
```

## Demo

```text
$ freeqcc launch --detach
freeqcc launched (pid 12345); logs → /home/you/.freeqcc/daemon.log

$ freeqcc status
─── freeqcc status ───
daemon:         running (pid 12345)
bot nick:       sourdough-bot
owner:          @chadfowler.com (did:plc:4qsy…)
agent DID:      did:key:z6Mki…
delegation:     unsigned (v1.0)
actor.online:   true
provenance:     verified=false (Cert has no signature; declarative only)
```

Now, from any freeq-connected client signed in as `@chadfowler.com`, send a DM to `sourdough-bot`. The agent dispatches your message to a persistent Claude Code session, captures the reply, and DMs it back. The conversation is one continuous Claude Code session: ask follow-up questions, edit code, ship a PR, all over IRC DMs.

If anyone *else* tries to DM `sourdough-bot`, they get:

> I'm @chadfowler.com's agent. I only respond to them.

…once per hour. After that they're silenced. All non-owner attempts are appended to `~/.freeqcc/refused.log` (JSONL).

## Identity & PKI

freeqcc is built on freeq's agent-native Phase 1 design ([docs](https://freeq.at/docs/agent-native/)) with two cryptographic layers:

1. **Agent identity (verified today).** A fresh ed25519 keypair lives at `~/.freeqcc/agent.key` (mode 0600). The agent connects via SASL ATPROTO-CHALLENGE — freeq's server cryptographically verifies the agent owns its `did:key:z…` on every connection.
2. **Creator binding (declarative today, verified soon).** A [`FreeqBotDelegation/v1`](https://github.com/chad/freeq/blob/main/freeq-server/src/connection/provenance.rs) cert at `~/.freeqcc/delegation.json` declares `bot_did = <agent>`, `creator_did = <your-DID>`, `revocation_authority = <your-DID>`. v1.0 ships **unsigned** — the freeq web client doesn't yet expose your MSGSIG signing key in a way the daemon can consume. Server stores the cert and surfaces it via `/api/v1/actors/{did}` with `_verified: false, _verification_reason: "Cert has no signature; declarative only"`. v1.1 adds an in-browser signing flow; this same daemon then auto-upgrades to verified provenance with no client-side change.

The full server-side verification machinery is already deployed (see [freeq-server commit history](../freeq-server/src/connection/provenance.rs)) — it's the cert mint side that needs polish. The format matches the Rust struct in [`freeq-bot-id/src/main.rs`](../freeq-bot-id/src/main.rs) so signed certs from either source are interchangeable.

## Security

- **Owner gate runs in the daemon, not the server.** The freeq server doesn't enforce manifest-based PRIVMSG gating today (intentional — keeps policy in user space). The daemon checks `sender DID == creator_did` on every message and refuses everything else.
- **Default cost guards:** 60s minimum cooldown between successful dispatches, 30 dispatches/hour hard cap. Configurable in `~/.freeqcc/config.json`.
- **Persistent claude session:** one `--resume`d session per agent, persisted at `~/.freeqcc/session.json`. Restart the daemon, the conversation continues.
- **No keys leave your laptop.** The agent's ed25519 private key never crosses the wire — only signatures do, in SASL challenges and message signing.

## Status

**v1.0** (this release):

- ✅ did:key SASL identity, server-verified
- ✅ FreeqBotDelegation/v1 cert format, declarative
- ✅ owner-DID-only gate, refusal once per hour
- ✅ persistent Claude Code session via `claude -p --resume`
- ✅ presence + heartbeat (auto-degrade on crash)
- ✅ Claude Code plugin: `/freeqcc-launch`, `/freeqcc-status`, `/freeqcc-stop`

**v1.1** (next):

- in-browser signing flow → verified provenance from day one of *that* release
- bot↔bot conversation (allowlist + per-peer rate limit)
- channel mention mode (rate-limited public replies)
- delegation: granting a third-party DID a *narrowed* capability set

## Layout

```
freeqcc/
├── PLAN.md              # design notes (lifts of every decision)
├── README.md            # this file
├── package.json         # @freeq/freeqcc — bin: freeqcc (deps: @freeq/bot-kit)
├── src/                 # TypeScript daemon
│   ├── owner.ts         # AT Protocol handle resolve via Bluesky public API
│   ├── connect.ts       # delegates to @freeq/bot-kit FreeqBot for SASL +
│   │                    #   PROVENANCE + AGENT REGISTER + heartbeat lifecycle
│   ├── gate.ts          # owner-only filter + rate limits
│   ├── dispatch.ts      # claude -p --resume subprocess
│   ├── daemon.ts        # long-lived process glue
│   ├── cli.ts           # commander entry
│   ├── config.ts        # ~/.freeqcc/config.json
│   ├── audit.ts         # refusal log
│   └── paths.ts         # ~/.freeqcc/ path helpers
└── plugin/              # Claude Code plugin
    ├── .claude-plugin/plugin.json
    └── skills/
        ├── freeqcc-launch/SKILL.md
        ├── freeqcc-status/SKILL.md
        └── freeqcc-stop/SKILL.md

did:key identity (`agent.key`) and the FreeqBotDelegation/v1 cert
(`delegation.json`) live at `~/.freeqcc/` and are managed by `@freeq/bot-kit`
(`loadOrCreateIdentity` / `loadOrMintDelegation`). bot-kit's `FreeqBot.create`
is configured with `name: ".freeqcc", root: $HOME` so its stateDir lines up
with freeqcc's existing on-disk layout.
```

## License

MIT.
