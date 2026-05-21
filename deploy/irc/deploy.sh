#!/bin/bash
set -e

# Deploy freeq IRC server to Miren
# Builds in a temp directory with the full workspace

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
TMPDIR=$(mktemp -d)

echo "Preparing deploy in $TMPDIR..."

# Copy workspace files
cp "$REPO_ROOT/Cargo.toml" "$TMPDIR/"
cp "$REPO_ROOT/Cargo.lock" "$TMPDIR/"
cp -r "$REPO_ROOT/freeq-sdk" "$TMPDIR/"
cp -r "$REPO_ROOT/freeq-server" "$TMPDIR/"

# Cargo needs every workspace member referenced by Cargo.toml to exist on
# disk, even if cargo-build only compiles freeq-server. Copy the whole
# source tree for the rest — cargo will skip them since they're not deps
# of `--package freeq-server`. (Mirror of deploy/staging/deploy.sh.)
for dir in freeq-tui freeq-auth-broker freeq-bots freeq-bot-id freeq-sdk-ffi freeq-windows-core freeq-av-client; do
    [ -d "$REPO_ROOT/$dir" ] && cp -r "$REPO_ROOT/$dir" "$TMPDIR/"
done

# Miren app config
mkdir -p "$TMPDIR/.miren"
cat > "$TMPDIR/.miren/app.toml" << 'EOF'
name = 'freeq-irc'
post_import = ''
env = []
include = []
EOF

# Procfile — Miren sets $PORT. Binary lives at /app/freeq-server because
# our custom Dockerfile copies it there from the builder stage (the
# target/release/ path only exists during the Cargo build, not in the
# slim runtime image).
cat > "$TMPDIR/Procfile" << 'EOF'
web: /app/freeq-server --listen-addr 127.0.0.1:16667 --web-addr 0.0.0.0:${PORT:-8080} --server-name irc.freeq.at --db-path /app/data/freeq.db --data-dir /app/data --motd "Welcome to freeq — IRC with AT Protocol identity. https://freeq.at"
EOF

# Remove any nested .miren dirs
rm -rf "$TMPDIR/freeq-server/.miren"

# Copy the local Dockerfile so Miren uses it instead of falling back to a
# cargo buildpack that expects a binary named after the app (freeq-irc).
cp "$SCRIPT_DIR/Dockerfile" "$TMPDIR/Dockerfile.miren"

cd "$TMPDIR"
echo "Deploying from $TMPDIR..."
miren deploy -f

echo "Setting route..."
miren route set irc.freeq.at freeq-irc 2>/dev/null || true

# Cleanup
rm -rf "$TMPDIR"
echo "Done!"
