#!/usr/bin/env bash
#
# Start two local freeq-server instances peered via iroh for S2S testing.
#
# Usage:
#   ./scripts/start-test-servers.sh          # start and wait
#   ./scripts/start-test-servers.sh stop     # kill any leftover servers
#
# Ports:
#   Server A: 16667  (accepts incoming S2S)
#   Server B: 16668  (peers with A)
#
# State dir: /tmp/freeq-test-{a,b}/
# PID files: /tmp/freeq-test-{a,b}.pid
# Logs:      /tmp/freeq-test-{a,b}/server.log
#
# Once running, use scripts/run-local-tests.sh to execute tests.
# Press Ctrl-C or run with "stop" to tear down.

set -euo pipefail
cd "$(dirname "$0")/.."

PORT_A=16667
PORT_B=16668
DIR_A="/tmp/freeq-test-a"
DIR_B="/tmp/freeq-test-b"
PID_FILE_A="/tmp/freeq-test-a.pid"
PID_FILE_B="/tmp/freeq-test-b.pid"

stop_servers() {
    local stopped=0
    for pidfile in "$PID_FILE_A" "$PID_FILE_B"; do
        if [ -f "$pidfile" ]; then
            pid=$(cat "$pidfile")
            if kill -0 "$pid" 2>/dev/null; then
                kill "$pid" 2>/dev/null
                echo "Stopped pid $pid"
                stopped=1
            fi
            rm -f "$pidfile"
        fi
    done
    # Also kill anything on our ports
    for port in $PORT_A $PORT_B; do
        lsof -ti :"$port" 2>/dev/null | xargs kill -9 2>/dev/null || true
    done
    if [ $stopped -eq 0 ]; then
        echo "No servers were running."
    fi
}

if [ "${1:-}" = "stop" ]; then
    echo "Stopping test servers..."
    stop_servers
    exit 0
fi

# Clean up any previous runs
stop_servers 2>/dev/null
sleep 0.5

# Build
echo "═══ Building freeq-server ═══"
cargo build --release --bin freeq-server 2>&1 | tail -3
BINARY="$(pwd)/target/release/freeq-server"
echo ""

# Fresh state dirs
rm -rf "$DIR_A" "$DIR_B"
mkdir -p "$DIR_A" "$DIR_B"

# ── Start Server A ──
echo "═══ Starting Server A on :$PORT_A ═══"
RUST_LOG=freeq_server=info "$BINARY" \
    --listen-addr "127.0.0.1:$PORT_A" \
    --server-name "server-a.test" \
    --data-dir "$DIR_A" \
    --db-path "$DIR_A/irc.db" \
    --iroh \
    >> "$DIR_A/server.log" 2>&1 &
echo $! > "$PID_FILE_A"
PID_A=$(cat "$PID_FILE_A")
echo "  PID: $PID_A"

# Wait for iroh endpoint
echo "  Waiting for iroh..."
IROH_ID_A=""
for i in $(seq 1 30); do
    if ! kill -0 "$PID_A" 2>/dev/null; then
        echo "  ERROR: Server A died on startup"
        cat "$DIR_A/server.log"
        exit 1
    fi
    if grep -q "Iroh ready" "$DIR_A/server.log" 2>/dev/null; then
        IROH_ID_A=$(grep "Iroh ready" "$DIR_A/server.log" | grep -oE '[0-9a-f]{64}' | head -1)
        break
    fi
    sleep 0.5
done

if [ -z "$IROH_ID_A" ]; then
    echo "  ERROR: Server A failed to start iroh (timeout)"
    cat "$DIR_A/server.log"
    stop_servers
    exit 1
fi
echo "  Iroh ID: ${IROH_ID_A:0:16}..."
echo ""

# ── Start Server B (peered with A) ──
echo "═══ Starting Server B on :$PORT_B (peered with A) ═══"
RUST_LOG=freeq_server=info "$BINARY" \
    --listen-addr "127.0.0.1:$PORT_B" \
    --server-name "server-b.test" \
    --data-dir "$DIR_B" \
    --db-path "$DIR_B/irc.db" \
    --iroh \
    --s2s-peers "$IROH_ID_A" \
    >> "$DIR_B/server.log" 2>&1 &
echo $! > "$PID_FILE_B"
PID_B=$(cat "$PID_FILE_B")
echo "  PID: $PID_B"

# Wait for S2S link
echo "  Waiting for S2S link..."
for i in $(seq 1 30); do
    if ! kill -0 "$PID_B" 2>/dev/null; then
        echo "  ERROR: Server B died on startup"
        cat "$DIR_B/server.log"
        stop_servers
        exit 1
    fi
    if grep -q "S2S link established" "$DIR_B/server.log" 2>/dev/null; then
        break
    fi
    sleep 0.5
done

# Verify both accepting connections
for port in $PORT_A $PORT_B; do
    if ! nc -z 127.0.0.1 "$port" 2>/dev/null; then
        echo "ERROR: Server on port $port not accepting connections"
        stop_servers
        exit 1
    fi
done

if grep -q "S2S link established" "$DIR_A/server.log" "$DIR_B/server.log" 2>/dev/null; then
    echo "  ✓ S2S link established"
else
    echo "  ⚠ S2S link may still be connecting..."
fi

echo ""
echo "═══════════════════════════════════════════"
echo "  Test servers running"
echo ""
echo "  Server A:  127.0.0.1:$PORT_A  (pid $PID_A)"
echo "  Server B:  127.0.0.1:$PORT_B  (pid $PID_B)"
echo ""
echo "  Logs:"
echo "    tail -f $DIR_A/server.log"
echo "    tail -f $DIR_B/server.log"
echo ""
echo "  Run tests:"
echo "    ./scripts/run-local-tests.sh"
echo "    ./scripts/run-local-tests.sh s2s_inv10"
echo ""
echo "  Stop:"
echo "    Ctrl-C  or  ./scripts/start-test-servers.sh stop"
echo "═══════════════════════════════════════════"
echo ""

# Wait for Ctrl-C
trap 'echo ""; echo "Shutting down..."; stop_servers; exit 0' INT TERM
while kill -0 "$PID_A" 2>/dev/null && kill -0 "$PID_B" 2>/dev/null; do
    sleep 2
done

echo "A server exited unexpectedly."
echo "Server A log (last 20 lines):"
tail -20 "$DIR_A/server.log" 2>/dev/null || true
echo "Server B log (last 20 lines):"
tail -20 "$DIR_B/server.log" 2>/dev/null || true
stop_servers
exit 1
