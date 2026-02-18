#!/usr/bin/env bash
#
# Run acceptance tests against locally-running test servers.
#
# Prerequisites: ./scripts/start-test-servers.sh must be running in another terminal.
#
# Usage:
#   ./scripts/run-local-tests.sh                    # all tests
#   ./scripts/run-local-tests.sh single             # single-server tests only
#   ./scripts/run-local-tests.sh s2s                # S2S tests only
#   ./scripts/run-local-tests.sh inv                # all INV tests
#   ./scripts/run-local-tests.sh s2s_inv10          # specific test
#   ./scripts/run-local-tests.sh s2s_inv10 s2s_inv2 # multiple tests

set -euo pipefail
cd "$(dirname "$0")/.."

PORT_A=16667
PORT_B=16668

# Verify servers are running
for port in $PORT_A $PORT_B; do
    if ! nc -z 127.0.0.1 "$port" 2>/dev/null; then
        echo "ERROR: No server on port $port"
        echo ""
        echo "Start test servers first:"
        echo "  ./scripts/start-test-servers.sh"
        exit 1
    fi
done

export SERVER="127.0.0.1:$PORT_A"
export LOCAL_SERVER="127.0.0.1:$PORT_A"
export REMOTE_SERVER="127.0.0.1:$PORT_B"

MODE="${1:-all}"
shift 2>/dev/null || true

case "$MODE" in
    all)
        echo "▶ Running ALL acceptance tests"
        echo "  Server A: $LOCAL_SERVER"
        echo "  Server B: $REMOTE_SERVER"
        echo ""
        cargo test -p freeq-server --test s2s_acceptance -- \
            --nocapture --test-threads=1
        ;;
    single|single_server)
        echo "▶ Running single-server tests against $SERVER"
        cargo test -p freeq-server --test s2s_acceptance -- \
            --nocapture --test-threads=1 single_server
        ;;
    s2s|federation)
        echo "▶ Running S2S tests: $LOCAL_SERVER ↔ $REMOTE_SERVER"
        cargo test -p freeq-server --test s2s_acceptance -- \
            --nocapture --test-threads=1 s2s_
        ;;
    inv)
        echo "▶ Running all INV tests"
        cargo test -p freeq-server --test s2s_acceptance -- \
            --nocapture --test-threads=1 inv
        ;;
    *)
        # Run specific test(s) — pass all remaining args as test filters
        FILTERS="$MODE $*"
        echo "▶ Running test(s): $FILTERS"
        for filter in $FILTERS; do
            cargo test -p freeq-server --test s2s_acceptance -- \
                --nocapture --test-threads=1 "$filter"
        done
        ;;
esac
