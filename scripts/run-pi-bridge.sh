#!/usr/bin/env bash
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
set -a
source "${SCRIPT_DIR}/pi-bridge.env"
set +a

exec cargo run -p freeq-bots --bin pi_bridge
