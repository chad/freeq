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

# Create minimal freeq-tui stub (needed for workspace but not built)
mkdir -p "$TMPDIR/freeq-tui/src"
cp "$REPO_ROOT/freeq-tui/Cargo.toml" "$TMPDIR/freeq-tui/"
echo "fn main() {}" > "$TMPDIR/freeq-tui/src/main.rs"

# Miren app config
mkdir -p "$TMPDIR/.miren"
cat > "$TMPDIR/.miren/app.toml" << 'EOF'
name = 'freeq-irc'
post_import = ''
env = []
include = []
EOF

# Procfile — Miren sets $PORT
cat > "$TMPDIR/Procfile" << 'EOF'
web: ./target/release/freeq-server --listen-addr 127.0.0.1:16667 --web-addr 0.0.0.0:${PORT:-8080} --server-name irc.freeq.at --db-path /app/data/freeq.db --data-dir /app/data --motd "Welcome to freeq — IRC with AT Protocol identity. https://freeq.at"
EOF

# Remove any nested .miren dirs
rm -rf "$TMPDIR/freeq-server/.miren"

cd "$TMPDIR"
echo "Deploying from $TMPDIR..."
miren deploy -f

echo "Setting route..."
miren route set irc.freeq.at freeq-irc 2>/dev/null || true

# Cleanup
rm -rf "$TMPDIR"
echo "Done!"
