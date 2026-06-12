#!/bin/bash
set -euo pipefail

# Deploy a self-hosted freeq instance (IRC server + web client) to Miren.
#
# Works from a fresh `git clone` — stages the Cargo workspace in a temp
# directory (cargo needs every workspace member referenced by Cargo.toml to
# exist on disk, even though only freeq-server is compiled), generates a
# Miren app config + Procfile, and runs `miren deploy`.
#
# Usage:
#   ./deploy/miren/deploy.sh [APP_NAME] [DOMAIN]
#
# or with env vars:
#   APP_NAME=freeq DOMAIN=irc.example.com ./deploy/miren/deploy.sh
#
# Optional env:
#   SERVER_NAME        IRC server name shown in messages (default: DOMAIN, else APP_NAME)
#   MOTD               message of the day
#   EXTRA_SERVER_ARGS  extra freeq-server flags, e.g. "--iroh --s2s-peers <endpoint-id>"
#   MIREN_CONTEXT      Miren org/context to deploy into (passed as -C)
#
# Runtime env passthrough: if any of OPER_DIDS, OPER_PASSWORD,
# BROKER_SHARED_SECRET, GITHUB_CLIENT_ID, GITHUB_CLIENT_SECRET, or RUST_LOG
# are set when you run this script, they are written into the app's env.

APP_NAME="${1:-${APP_NAME:-freeq}}"
DOMAIN="${2:-${DOMAIN:-}}"
SERVER_NAME="${SERVER_NAME:-${DOMAIN:-$APP_NAME}}"
MOTD="${MOTD:-Welcome to freeq — IRC with AT Protocol identity.}"
EXTRA_SERVER_ARGS="${EXTRA_SERVER_ARGS:-}"
MIREN_CONTEXT="${MIREN_CONTEXT:-}"

if ! command -v miren >/dev/null 2>&1; then
    echo "error: miren CLI not found. Install it and log in first — see https://miren.dev/" >&2
    exit 1
fi

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
TMPDIR=$(mktemp -d)
trap 'rm -rf "$TMPDIR"' EXIT

echo "Preparing deploy of '$APP_NAME' in $TMPDIR..."

# ── Stage the Cargo workspace ──────────────────────────────────────────────
cp "$REPO_ROOT/Cargo.toml" "$REPO_ROOT/Cargo.lock" "$TMPDIR/"

# Members freeq-server actually depends on
cp -r "$REPO_ROOT/freeq-sdk" "$TMPDIR/"
cp -r "$REPO_ROOT/freeq-server" "$TMPDIR/"

# Remaining workspace members — cargo must resolve them all even when only
# building freeq-server, but it won't compile them.
for dir in freeq-tui freeq-auth-broker freeq-bots freeq-bot-id freeq-sdk-ffi freeq-windows-core freeq-av-client; do
    [ -d "$REPO_ROOT/$dir" ] && cp -r "$REPO_ROOT/$dir" "$TMPDIR/"
done

# ── Stage the web client ───────────────────────────────────────────────────
# freeq-app depends on @freeq/sdk via `file:../freeq-sdk-js`, so copy both
# (the Dockerfile builds the JS SDK before the web app).
cp -r "$REPO_ROOT/freeq-app" "$TMPDIR/web-client"
rm -rf "$TMPDIR/web-client/node_modules" "$TMPDIR/web-client/dist" "$TMPDIR/web-client/src-tauri"
cp -r "$REPO_ROOT/freeq-sdk-js" "$TMPDIR/freeq-sdk-js"
rm -rf "$TMPDIR/freeq-sdk-js/node_modules" "$TMPDIR/freeq-sdk-js/dist"

# ── Dockerfile ─────────────────────────────────────────────────────────────
# Custom Dockerfile so Miren doesn't fall back to a cargo buildpack that
# expects a binary named after the app.
cp "$SCRIPT_DIR/Dockerfile" "$TMPDIR/Dockerfile.miren"

# ── Miren app config ───────────────────────────────────────────────────────
# Pass through runtime env vars set in the caller's environment.
# TODO(verify): the app.toml `env` entry format is assumed to be 'KEY=VALUE'
# strings (the maintainer's config only shows an empty `env = []`). If your
# Miren version configures env differently (e.g. `miren env set`), leave
# these unset here and set them after the first deploy instead.
ENV_LINES=""
for var in OPER_DIDS OPER_PASSWORD BROKER_SHARED_SECRET GITHUB_CLIENT_ID GITHUB_CLIENT_SECRET RUST_LOG; do
    if [ -n "${!var:-}" ]; then
        ENV_LINES="${ENV_LINES}'${var}=${!var}', "
    fi
done
ENV_LINES="${ENV_LINES%, }"

mkdir -p "$TMPDIR/.miren"
cat > "$TMPDIR/.miren/app.toml" << EOF
name = '$APP_NAME'
post_import = ''
env = [$ENV_LINES]
include = []
EOF

# ── Procfile ───────────────────────────────────────────────────────────────
# Miren sets \$PORT (Heroku-style). The binary lives at /app/freeq-server
# because the Dockerfile copies it there from the builder stage. Plain-TCP
# IRC stays on loopback — Miren routes HTTP(S) only, so clients connect via
# the WebSocket transport at /irc on your domain.
cat > "$TMPDIR/Procfile" << EOF
web: /app/freeq-server --listen-addr 127.0.0.1:16667 --web-addr 0.0.0.0:\${PORT:-8080} --web-static-dir /app/web --server-name $SERVER_NAME --db-path /app/data/freeq.db --data-dir /app/data --motd "$MOTD"${EXTRA_SERVER_ARGS:+ $EXTRA_SERVER_ARGS}
EOF

# Remove any nested .miren dirs that came along with source copies
find "$TMPDIR" -mindepth 2 -name ".miren" -type d -exec rm -rf {} + 2>/dev/null || true

# ── Deploy ─────────────────────────────────────────────────────────────────
cd "$TMPDIR"
echo "Deploying from $TMPDIR..."
if [ -n "$MIREN_CONTEXT" ]; then
    miren deploy -f -C "$MIREN_CONTEXT"
else
    miren deploy -f
fi

if [ -n "$DOMAIN" ]; then
    echo "Setting route $DOMAIN -> $APP_NAME..."
    if [ -n "$MIREN_CONTEXT" ]; then
        miren route set "$DOMAIN" "$APP_NAME" -C "$MIREN_CONTEXT" 2>/dev/null || \
            echo "warning: 'miren route set' failed — set the route manually: miren route set $DOMAIN $APP_NAME -C $MIREN_CONTEXT"
    else
        miren route set "$DOMAIN" "$APP_NAME" 2>/dev/null || \
            echo "warning: 'miren route set' failed — set the route manually: miren route set $DOMAIN $APP_NAME"
    fi
fi

echo ""
echo "Done! Next steps:"
if [ -n "$DOMAIN" ]; then
    echo "  1. DNS: point $DOMAIN (A/AAAA record) at your Miren host."
    echo "     TLS is terminated by Miren's router for routed domains."
    echo "  2. Open https://$DOMAIN — the web client is served at the root,"
    echo "     WebSocket IRC at /irc, REST API at /api/v1/*."
else
    echo "  1. Attach a domain:  miren route set irc.example.com $APP_NAME"
    echo "     then point DNS (A/AAAA) at your Miren host."
fi
echo "  3. Secrets: set OPER_PASSWORD / OPER_DIDS / BROKER_SHARED_SECRET etc."
echo "     either by exporting them before re-running this script, or via the"
echo "     Miren CLI (see deploy/miren/README.md)."
echo "  4. AT Protocol web login needs the auth broker — see the 'Auth broker'"
echo "     section in deploy/miren/README.md."
echo "  5. Data (SQLite db + keys) lives at /app/data inside the app."
echo "     Read the backup notes in deploy/miren/README.md before going live."
