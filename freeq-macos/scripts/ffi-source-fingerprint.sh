#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
cd "$REPO_ROOT"

git ls-files -z -- \
  Cargo.lock \
  freeq-sdk/Cargo.toml \
  freeq-sdk/src \
  freeq-sdk-ffi/Cargo.toml \
  freeq-sdk-ffi/src \
  freeq-sdk-ffi/uniffi.toml \
  | LC_ALL=C sort -z \
  | xargs -0 shasum -a 256 \
  | shasum -a 256 \
  | awk '{print $1}'
