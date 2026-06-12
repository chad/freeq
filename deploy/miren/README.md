# Self-hosting freeq on Miren

The recommended way to self-host freeq. [Miren](https://miren.dev/) is a
self-hosted, Heroku-style PaaS: you point the CLI at your Miren host, it
builds the app from a Dockerfile, injects `$PORT`, runs the `Procfile`
process, and routes HTTPS traffic for your domain to it.

One deploy gives you the IRC server, the web client (served at the root of
your domain), WebSocket IRC at `/irc`, and the REST API at `/api/v1/*`.

## 10-minute quickstart

**Prerequisites**

- A running Miren instance (your own server, or a hosted one)
- The `miren` CLI installed and logged in to that instance
  <!-- TODO(verify): exact login command — `miren login` is the expected shape -->
- `git`

**Deploy**

```bash
git clone https://github.com/chad/freeq
cd freeq
DOMAIN=irc.example.com ./deploy/miren/deploy.sh
```

That's it. The script:

1. Stages the Cargo workspace + web client in a temp directory
2. Generates `.miren/app.toml`, a `Procfile`, and a `Dockerfile.miren`
3. Runs `miren deploy -f` (Docker build happens on the Miren host)
4. Routes your domain to the app (`miren route set irc.example.com freeq`)

First build takes a while (full Rust release build); later deploys reuse
Docker layer caching on the Miren host.

**Then point DNS** — an A/AAAA record for `irc.example.com` at your Miren
host. Miren's router terminates TLS for routed domains. Open
`https://irc.example.com` and you should see the freeq web client.

**Knobs** (all optional):

```bash
APP_NAME=my-freeq \
DOMAIN=irc.example.com \
SERVER_NAME=irc.example.com \
MOTD="welcome to my server" \
MIREN_CONTEXT=my-org \
./deploy/miren/deploy.sh
```

## Secrets

The server reads these from its environment:

| Var | Purpose |
|---|---|
| `OPER_PASSWORD` | Enables the `OPER` command |
| `OPER_DIDS` | DIDs auto-granted server operator (comma-separated) |
| `BROKER_SHARED_SECRET` | HMAC secret shared with the auth broker |
| `GITHUB_CLIENT_ID` / `GITHUB_CLIENT_SECRET` | GitHub OAuth for the credential verifier |

The simplest path: export them before running `deploy.sh` — the script
writes any that are set into the app's env config:

```bash
OPER_PASSWORD=$(openssl rand -hex 24) \
BROKER_SHARED_SECRET=$(openssl rand -hex 32) \
DOMAIN=irc.example.com ./deploy/miren/deploy.sh
```

Alternatively, set them on the running app via the Miren CLI:

```bash
# TODO(verify): exact Miren CLI syntax for app env vars
miren env set freeq OPER_PASSWORD=...
```

## Domain & TLS

```bash
miren route set irc.example.com freeq
```

(`deploy.sh` does this for you when `DOMAIN` is set.) Point DNS at the Miren
host; Miren's router handles the TLS certificate for routed domains.
<!-- TODO(verify): whether your Miren instance issues certs automatically
(Let's Encrypt) or needs certs configured on the router. -->

## Data & backups

Everything persistent lives at **`/app/data`** inside the app container:

| File | Purpose |
|---|---|
| `freeq.db` (+ `-wal`/`-shm`) | Messages, channels, users (SQLite) |
| `*.secret` | Signing keys, DB encryption key, iroh identity |

> **Critical**: `db-encryption-key.secret` is required to read stored
> messages — if you lose it, history is irrecoverable. Back up the key
> files alongside the database.

<!-- TODO(verify): how your Miren instance persists app data across
redeploys — confirm /app/data is on a volume (and where it lives on the
Miren host) before relying on history surviving an upgrade. -->

Backing up SQLite safely — either:

```bash
# Hot backup, server running (from a shell inside the app container):
sqlite3 /app/data/freeq.db ".backup /app/data/freeq-backup.db"
# or: sqlite3 /app/data/freeq.db "VACUUM INTO '/app/data/freeq-backup.db'"
```

or stop the app and copy `freeq.db` **together with** its `-wal` and `-shm`
files (copying the `.db` alone while the server runs can produce a corrupt
snapshot). Copy the `*.secret` files too.

```bash
# TODO(verify): exact Miren CLI for a shell / file copy into the app,
# e.g. `miren exec freeq -- sqlite3 ...` or `miren cp`.
```

## Upgrading

```bash
cd freeq
git pull
DOMAIN=irc.example.com ./deploy/miren/deploy.sh   # same args as the first deploy
```

Each deploy builds a fresh image and replaces the running release. Database
schema migrations run automatically on startup. Re-export your secret env
vars when redeploying if you passed them via the script (or set them once
via the Miren CLI so they stick to the app).

## Auth broker (AT Protocol web login)

Password-less AT Protocol (Bluesky) login from the **web client** needs the
OAuth broker (`freeq-auth-broker`) running as a separate service. SASL login
from the TUI/SDK works without it.

Run it as a second Miren app on its own domain (e.g. `auth.example.com`).
The broker honors `$PORT`, so a minimal Procfile app works:

```
web: /app/freeq-auth-broker
```

with env:

| Var | Value |
|---|---|
| `BROKER_SHARED_SECRET` | same value as on the server (required) |
| `BROKER_PUBLIC_URL` | `https://auth.example.com` |
| `FREEQ_SERVER_URL` | `https://irc.example.com` |
| `BROKER_DB_PATH` | `/app/data/broker.db` |

There's no turnkey script for the broker yet — adapt `deploy.sh` (the
workspace staging is identical; build `--package freeq-auth-broker` instead)
or run it anywhere else that can reach your server over HTTPS. In the web
client, set the broker URL under **Advanced** on the connect screen (it
defaults to the web client's own origin).

## Federation

Pass S2S flags through `EXTRA_SERVER_ARGS`:

```bash
EXTRA_SERVER_ARGS="--iroh --s2s-peers <peer-endpoint-id> --s2s-allowed-peers <peer-endpoint-id>" \
DOMAIN=irc.example.com ./deploy/miren/deploy.sh
```

The server prints its own iroh endpoint ID on startup (check the app logs)
— give that to your peers. iroh is QUIC over UDP with built-in NAT
traversal via relays, so it generally works even though Miren only routes
HTTP; pin `--iroh-port` if you want to open a UDP port on the Miren host
directly. See [docs/federation.md](../../docs/federation.md) and
[docs/SECURITY.md](../../docs/SECURITY.md) (use `--s2s-allowed-peers` — don't
run open federation in production).

## Troubleshooting

- **Build fails resolving workspace members** — run the script from a clean
  checkout; it must copy every crate referenced by the root `Cargo.toml`.
- **App starts but the web client 404s** — the image builds the web client
  into `/app/web`; check the build logs for the `web-builder` stage.
- **AT login fails on the web client** — you need the auth broker (above)
  and `BROKER_SHARED_SECRET` set identically on both apps.
