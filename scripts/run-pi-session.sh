#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
FIFO_PATH="${PI_FIFO:-/tmp/freeq-pi.fifo}"
PI_CMD="${PI_CMD:-pi}"

if ! command -v "$PI_CMD" >/dev/null 2>&1; then
  echo "PI_CMD '$PI_CMD' not found. Set PI_CMD to your pi executable." >&2
  exit 1
fi

if [ -e "$FIFO_PATH" ] && [ ! -p "$FIFO_PATH" ]; then
  echo "FIFO path exists and is not a pipe: $FIFO_PATH" >&2
  exit 1
fi

if [ ! -p "$FIFO_PATH" ]; then
  rm -f "$FIFO_PATH"
  mkfifo "$FIFO_PATH"
fi

echo "FIFO ready at $FIFO_PATH"

echo "Starting pi session..."
exec "$SCRIPT_DIR/pi-merge-input.py" "$FIFO_PATH" | "$PI_CMD"
