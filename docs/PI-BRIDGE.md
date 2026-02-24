# Pi IRC Bridge

This bridge lets you send instructions to the local pi session over IRC.
It listens to a control channel (or DMs), only accepts commands from your DID,
and writes them to a local JSONL queue.

## Components

- **pi bridge bot**: `freeq-bots --bin pi_bridge`
- **inbox reader**: `scripts/pi-inbox.py` (reads queue and prints new tasks)
- **optional replies**: write JSONL to `/tmp/freeq-pi-replies.jsonl`

## Configure

Required env vars:

- `PI_ALLOWED_DID` — DID that can issue commands (e.g. `did:plc:...` for chadfowler.com)
- `PI_BROKER_TOKEN` — broker session token from `https://auth.freeq.at/auth/login?handle=chadfowler.com`

Optional:

- `PI_SERVER_ADDR` — default `irc.freeq.at:6667`
- `PI_BROKER_URL` — default `https://auth.freeq.at`
- `PI_CHANNEL` — control channel (e.g. `#pi-control`)
- `PI_PREFIX` — default `!pi`
- `PI_OUTBOX` — default `/tmp/freeq-pi-queue.jsonl`
- `PI_REPLY_INBOX` — default `/tmp/freeq-pi-replies.jsonl`

## Run the bot

```bash
PI_ALLOWED_DID=did:plc:... \
PI_BROKER_TOKEN=... \
PI_CHANNEL=#pi-control \
cargo run -p freeq-bots --bin pi_bridge
```

## Send a command

From IRC (DM or in `#pi-control`):

```
!pi implement rate limiting for JOIN spam
```

The bot appends to `/tmp/freeq-pi-queue.jsonl`.

## Read commands in the pi session

```bash
./scripts/pi-inbox.py
```

## Send replies back to IRC

Append JSONL lines to `/tmp/freeq-pi-replies.jsonl`:

```json
{"target":"#pi-control","text":"acknowledged — working on it"}
```

If `target` is omitted, the bot uses `PI_CHANNEL`.
