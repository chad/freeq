# freeq-claude-mcp

A working example agent built on [`freeq-agent-kit`](../../): bridges a
Claude Code session into a freeq AV channel as a real-time voice + chat
+ visual participant.

This is the "build your own bot" reference. Everything that's
character-agnostic (voice activity segmentation, address detection,
hallucination filtering, speech post-processing) comes from
`freeq-agent-kit`. Everything that's specific to *this* bot — the
streaming STT bridge, the MCP tool surface, the particle-face video
tile with overlays — lives here.

## What it ships

- A **Model Context Protocol server** (`bin/mcp_server.rs`) that Claude
  Code connects to over stdio. Exposes the tools listed in
  [`SKILL.md`](./SKILL.md): connect, listen, say, post, show / show_file /
  set_status, look, recall, participants, disconnect.
- An **orchestrator** (`src/orchestrator.rs`) that joins IRC + SASL,
  discovers or starts the AV session, taps every participant's audio,
  streams transcripts in via Deepgram (or batches via Groq Whisper if no
  Deepgram key), and broadcasts ElevenLabs TTS out — with barge-in,
  auto-thinking, an auto-heartbeat for long ops, and a persistent
  conversation memory keyed per channel.
- A **ghostly particle face** as the video tile, with a status chip,
  scene cards, file slices, quotes, and a live whiteboard graph that
  builds itself as SVO triples accumulate.
- A **stdio-JSON driver** (`bin/stdio_driver.rs`) for manual testing
  without an MCP client.
- The **`/freeq` Claude Code skill** in [`SKILL.md`](./SKILL.md). Symlink
  it into `~/.claude/skills/freeq/SKILL.md` so CC picks it up.

## Setup

```bash
# 1) Build.
cd /path/to/freeq
cargo build --release -p freeq-claude-mcp

# 2) Configure secrets. Copy this file template, fill in keys.
cat > freeq-agent-kit/examples/claude-mcp/.env <<EOF
DEEPGRAM_API_KEY=...   # optional, recommended — streaming STT
DEEPGRAM_MODEL=nova-3
GROQ_API_KEY=...       # required if no Deepgram key (batched STT fallback)
ELEVENLABS_API_KEY=... # required for TTS
ELEVENLABS_VOICE_ID=...
FREEQ_SERVER=wss://irc.freeq.at/irc
EOF
chmod 600 freeq-agent-kit/examples/claude-mcp/.env

# 3) Install the skill (one-time).
mkdir -p ~/.claude/skills/freeq
ln -sf "$(pwd)/freeq-agent-kit/examples/claude-mcp/SKILL.md" \
  ~/.claude/skills/freeq/SKILL.md

# 4) Register the MCP server with Claude Code.
claude mcp add freeq-claude "$(pwd)/target/release/freeq-claude-mcp"
```

## Usage

In a fresh Claude Code session, say something like:

> *"Let's talk on freeq in #avtest"*

The skill triggers, Claude calls `freeq_connect`, joins the AV session,
and starts listening. Address it by name (*"Claude, what do you see?"*)
to get a spoken reply. Visual artifacts (file slices, scene cards) go
on the tile; long text or links go to channel chat; the spoken voice
carries only the headline.

## Manual testing (without MCP)

The `claude-bot-stdio` binary takes JSON commands on stdin and emits
JSON events on stdout. Useful when iterating on the orchestrator
without bouncing CC sessions:

```bash
set -a; . freeq-agent-kit/examples/claude-mcp/.env; set +a
echo '{"cmd":"connect","channel":"#avtest","nick":"claude","start_if_idle":true}' \
  | target/release/claude-bot-stdio
```

## What's where

- `src/lib.rs` — re-exports
- `src/orchestrator.rs` — the bot lifecycle (~1100 LOC)
- `src/streaming_stt.rs` — Deepgram websocket client
- `src/video_face.rs` — ghostly particle face video source with shared `ParticleControl`
- `src/tile_overlay.rs` — SVG renderers for status / cards / file slices / quotes / graph
- `src/discover.rs` — SFU URL derivation + REST session lookup
- `src/bin/mcp_server.rs` — `rmcp` tool surface
- `src/bin/stdio_driver.rs` — JSON-lines test driver

## License

MIT or Apache-2.0 at your option.
