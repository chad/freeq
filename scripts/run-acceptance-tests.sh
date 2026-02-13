#!/usr/bin/env bash
#
# Run acceptance tests against live servers.
#
# Usage:
#   ./scripts/run-acceptance-tests.sh                    # single-server (localhost:6667)
#   ./scripts/run-acceptance-tests.sh s2s                # S2S tests (localhost + irc.freeq.at)
#   ./scripts/run-acceptance-tests.sh all                # everything
#   ./scripts/run-acceptance-tests.sh <test_name>        # run a specific test
#
# Environment overrides:
#   SERVER=host:port              # single-server target
#   LOCAL_SERVER=host:port        # S2S local server
#   REMOTE_SERVER=host:port       # S2S remote server
#
# Examples:
#   SERVER=irc.freeq.at:6667 ./scripts/run-acceptance-tests.sh
#   LOCAL_SERVER=localhost:6667 REMOTE_SERVER=irc.freeq.at:6667 ./scripts/run-acceptance-tests.sh s2s
#   ./scripts/run-acceptance-tests.sh single_server_nick_change

set -euo pipefail
cd "$(dirname "$0")/.."

: "${SERVER:=localhost:6667}"
: "${LOCAL_SERVER:=$SERVER}"
: "${REMOTE_SERVER:=irc.freeq.at:6667}"

MODE="${1:-all}"

export SERVER LOCAL_SERVER REMOTE_SERVER

case "$MODE" in
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
    all)
        echo "▶ Running ALL acceptance tests"
        echo "  Single-server: $SERVER"
        echo "  S2S: $LOCAL_SERVER ↔ $REMOTE_SERVER"
        echo ""
        cargo test -p freeq-server --test s2s_acceptance -- \
            --nocapture --test-threads=1
        ;;
    *)
        echo "▶ Running test: $MODE"
        cargo test -p freeq-server --test s2s_acceptance -- \
            --nocapture --test-threads=1 "$MODE"
        ;;
esac
