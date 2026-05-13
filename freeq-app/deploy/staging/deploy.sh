#!/bin/bash
set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
TMPDIR=$(mktemp -d)

echo "Preparing staging deploy in $TMPDIR..."

cp "$REPO_ROOT/Cargo.toml" "$REPO_ROOT/Cargo.lock" "$TMPDIR/"

# Copy workspace members that freeq-server depends on
cp -r "$REPO_ROOT/freeq-sdk" "$TMPDIR/"
cp -r "$REPO_ROOT/freeq-server" "$TMPDIR/"
[ -d "$REPO_ROOT/freeq-av-client" ] && cp -r "$REPO_ROOT/freeq-av-client" "$TMPDIR/"

# Copy remaining workspace members (stubs for Cargo workspace resolution)
for dir in freeq-tui freeq-auth-broker freeq-bots freeq-bot-id freeq-sdk-ffi freeq-windows-core; do
    [ -d "$REPO_ROOT/$dir" ] && cp -r "$REPO_ROOT/$dir" "$TMPDIR/"
done

cp -r "$REPO_ROOT/freeq-app" "$TMPDIR/web-client"
rm -rf "$TMPDIR/web-client/node_modules" "$TMPDIR/web-client/dist" "$TMPDIR/web-client/src-tauri"

# freeq-app depends on @freeq/sdk via `file:../freeq-sdk-js`. Copy the source
# in (without node_modules/dist) so the Dockerfile can build it before web.
cp -r "$REPO_ROOT/freeq-sdk-js" "$TMPDIR/freeq-sdk-js"
rm -rf "$TMPDIR/freeq-sdk-js/node_modules" "$TMPDIR/freeq-sdk-js/dist"

cp "$SCRIPT_DIR/Dockerfile" "$TMPDIR/Dockerfile.miren"

mkdir -p "$TMPDIR/.miren"
cat > "$TMPDIR/.miren/app.toml" << 'EOF'
name = 'freeq-staging'
post_import = ''
env = []
include = []
EOF

cat > "$TMPDIR/Procfile" << 'EOF'
web: /app/freeq-server --listen-addr 127.0.0.1:16667 --web-addr 0.0.0.0:${PORT:-8080} --web-static-dir /app/web --server-name staging.freeq.at --db-path /app/data/freeq.db --data-dir /app/data --iroh --motd "freeq staging — AV with iroh-live"
EOF

find "$TMPDIR" -mindepth 2 -name ".miren" -type d -exec rm -rf {} + 2>/dev/null || true

cd "$TMPDIR"
echo "Deploying from $TMPDIR..."
miren deploy -f -C blueyard-projects

echo "Setting route..."
miren route set staging.freeq.at freeq-staging -C blueyard-projects 2>/dev/null || true

rm -rf "$TMPDIR"
echo "Done! App should be live at https://staging.freeq.at"
