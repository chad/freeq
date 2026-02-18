#!/usr/bin/env bash
#
# Start two local servers, run all acceptance tests, then tear down.
#
# Usage:
#   ./scripts/test-local-e2e.sh              # all tests
#   ./scripts/test-local-e2e.sh inv10        # specific test filter
#
set -euo pipefail
cd "$(dirname "$0")/.."

PORT_A=16667
PORT_B=16668
DIR_A="/tmp/freeq-test-a"
DIR_B="/tmp/freeq-test-b"

cleanup() {
    echo ""
    echo "── Tearing down ──"
    [ -n "${PID_A:-}" ] && kill "$PID_A" 2>/dev/null && echo "  Stopped server A (pid $PID_A)"
    [ -n "${PID_B:-}" ] && kill "$PID_B" 2>/dev/null && echo "  Stopped server B (pid $PID_B)"
    for port in $PORT_A $PORT_B; do
        lsof -ti :"$port" 2>/dev/null | xargs kill -9 2>/dev/null || true
    done
}
trap cleanup EXIT

# Kill anything left over
for port in $PORT_A $PORT_B; do
    lsof -ti :"$port" 2>/dev/null | xargs kill -9 2>/dev/null || true
done
sleep 0.5

# Build
echo "═══ Building ═══"
cargo build --release --bin freeq-server 2>&1 | tail -3
BINARY="$(pwd)/target/release/freeq-server"

# Fresh state
rm -rf "$DIR_A" "$DIR_B"
mkdir -p "$DIR_A" "$DIR_B"

# Start Server A
echo "═══ Starting Server A (:$PORT_A) ═══"
RUST_LOG=freeq_server=info "$BINARY" \
    --listen-addr "127.0.0.1:$PORT_A" \
    --server-name "server-a.test" \
    --data-dir "$DIR_A" \
    --db-path "$DIR_A/irc.db" \
    --iroh \
    >> "$DIR_A/server.log" 2>&1 &
PID_A=$!

# Wait for iroh
IROH_ID_A=""
for i in $(seq 1 30); do
    kill -0 "$PID_A" 2>/dev/null || { echo "Server A died"; cat "$DIR_A/server.log"; exit 1; }
    IROH_ID_A=$(grep -oE '[0-9a-f]{64}' "$DIR_A/server.log" 2>/dev/null | head -1) && [ -n "$IROH_ID_A" ] && break
    sleep 0.5
done
[ -z "$IROH_ID_A" ] && { echo "Server A iroh timeout"; cat "$DIR_A/server.log"; exit 1; }
echo "  Iroh: ${IROH_ID_A:0:16}…"

# Start Server B (peered)
echo "═══ Starting Server B (:$PORT_B) ═══"
RUST_LOG=freeq_server=info "$BINARY" \
    --listen-addr "127.0.0.1:$PORT_B" \
    --server-name "server-b.test" \
    --data-dir "$DIR_B" \
    --db-path "$DIR_B/irc.db" \
    --iroh \
    --s2s-peers "$IROH_ID_A" \
    >> "$DIR_B/server.log" 2>&1 &
PID_B=$!

# Wait for S2S
for i in $(seq 1 30); do
    kill -0 "$PID_B" 2>/dev/null || { echo "Server B died"; cat "$DIR_B/server.log"; exit 1; }
    grep -q "S2S link established" "$DIR_B/server.log" 2>/dev/null && break
    sleep 0.5
done
grep -q "S2S link established" "$DIR_B/server.log" 2>/dev/null && echo "  ✓ S2S linked" || echo "  ⚠ S2S may still be connecting"

# Run tests
echo ""
echo "═══ Running acceptance tests ═══"
export SERVER="127.0.0.1:$PORT_A"
export LOCAL_SERVER="127.0.0.1:$PORT_A"
export REMOTE_SERVER="127.0.0.1:$PORT_B"

FILTER="${1:-}"
if [ -n "$FILTER" ]; then
    echo "  Filter: $FILTER"
    cargo test -p freeq-server --test s2s_acceptance -- --nocapture --test-threads=1 "$FILTER"
else
    cargo test -p freeq-server --test s2s_acceptance -- --nocapture --test-threads=1
fi
