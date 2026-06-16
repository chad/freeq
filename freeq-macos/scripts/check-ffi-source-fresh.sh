#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
ARTIFACT="$REPO_ROOT/freeq-macos/FreeqSDK.xcframework/macos-arm64/libfreeq_sdk_ffi.a"
STAMP="$REPO_ROOT/freeq-macos/FreeqSDK.xcframework/macos-arm64/ffi-source.sha256"

if [ ! -f "$ARTIFACT" ]; then
  echo "error: missing macOS FFI artifact: $ARTIFACT" >&2
  echo "Run ./freeq-macos/build-rust.sh before building the macOS app." >&2
  exit 1
fi

if [ ! -f "$STAMP" ]; then
  echo "error: missing macOS FFI artifact fingerprint: $STAMP" >&2
  echo "Run ./freeq-macos/build-rust.sh before building the macOS app." >&2
  exit 1
fi

expected="$(tr -d '[:space:]' < "$STAMP")"
actual="$("$SCRIPT_DIR/ffi-source-fingerprint.sh")"

if [ "$actual" != "$expected" ]; then
  echo "error: macOS Rust FFI artifact is stale." >&2
  echo "Run ./freeq-macos/build-rust.sh, then rebuild the app." >&2
  echo "expected: $expected" >&2
  echo "actual:   $actual" >&2
  exit 1
fi
