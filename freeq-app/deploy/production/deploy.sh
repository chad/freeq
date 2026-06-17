#!/bin/bash
set -e

# Production deploy of irc.freeq.at — freeq-server (iroh AV) + web client in one
# image, mirroring the validated staging deploy. This ships:
#   - iroh/QUIC AV transport (reliable video — the WS fallback drops video tracks)
#   - the web client INCLUDING the camera-publish reliability fix
#
# ⚠️  PRODUCTION: irc.freeq.at is a live, multi-user service. /app/data is NOT a
# persistent volume in this image, so server-side state/history may reset on
# redeploy. Run this only when you've accepted that (or after wiring a volume).

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
# This script lives at freeq-app/deploy/production, so the repo root is THREE up.
REPO_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
TMPDIR=$(mktemp -d)

echo "Preparing PRODUCTION deploy in $TMPDIR..."

cp "$REPO_ROOT/Cargo.toml" "$REPO_ROOT/Cargo.lock" "$TMPDIR/"

# Copy ALL workspace members — derived from Cargo.toml so it can never go stale
# (cargo-chef needs every member present to extract the package graph).
for dir in $(sed -n '/^members = \[/,/^\]/p' "$REPO_ROOT/Cargo.toml" | grep -o '"[^"]*"' | tr -d '"' | cut -d/ -f1 | sort -u); do
    if [ -d "$REPO_ROOT/$dir" ] && [ ! -d "$TMPDIR/$dir" ]; then
        cp -r "$REPO_ROOT/$dir" "$TMPDIR/"
    fi
done

cp -r "$REPO_ROOT/freeq-app" "$TMPDIR/web-client"
rm -rf "$TMPDIR/web-client/node_modules" "$TMPDIR/web-client/dist" "$TMPDIR/web-client/src-tauri"

cp -r "$REPO_ROOT/freeq-sdk-js" "$TMPDIR/freeq-sdk-js"
rm -rf "$TMPDIR/freeq-sdk-js/node_modules" "$TMPDIR/freeq-sdk-js/dist"

# Reuse the staging Dockerfile (server + web build) verbatim.
cp "$REPO_ROOT/freeq-app/deploy/staging/Dockerfile" "$TMPDIR/Dockerfile.miren"

mkdir -p "$TMPDIR/.miren"
cat > "$TMPDIR/.miren/app.toml" << 'EOF'
name = 'freeq-irc'
post_import = ''
env = []
include = []
EOF

cat > "$TMPDIR/Procfile" << 'EOF'
web: /app/freeq-server --listen-addr 127.0.0.1:16667 --web-addr 0.0.0.0:${PORT:-8080} --web-static-dir /app/web --server-name irc.freeq.at --db-path /app/data/freeq.db --data-dir /app/data --iroh --motd "Welcome to freeq — IRC with AT Protocol identity. https://freeq.at"
EOF

find "$TMPDIR" -mindepth 2 -name ".miren" -type d -exec rm -rf {} + 2>/dev/null || true

cd "$TMPDIR"
echo "Deploying freeq-irc (production)..."
miren deploy -f

echo "Setting route..."
miren route set irc.freeq.at freeq-irc 2>/dev/null || true

rm -rf "$TMPDIR"
echo "Done! Live at https://irc.freeq.at (iroh AV + camera-publish fix)."
