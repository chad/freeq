#!/usr/bin/env bash
# Authoritative verification gate for Fabro agent runs — mirrors freeq's CI
# (.github/workflows/ci.yml) so an agent's PR can't be greener locally than it
# will be on GitHub. Fabro command nodes call this as the `goal_gate`: a
# non-zero exit fails the run, and a failed run opens no PR.
#
# Mirrors CI's crate exclusions: the heavy AV crates (freeq-av, freeq-eliza,
# freeq-av-client, freeq-av-image) need a C++/audio toolchain and are slow, so
# CI skips them in check/test/clippy and so do we. Agents are scoped to the
# CI-covered crates + freeq-app; changes inside the AV crates need the manual
# extended gate (see .fabro/README.md).
set -uo pipefail

# Locate cargo on PATH. Fabro `command` nodes run in a bare shell that may not
# inherit the container/VM's cargo location (rust:1-bookworm puts it at
# /usr/local/cargo/bin; rustup uses ~/.cargo/bin). Prepend the known spots so
# the gate finds cargo whether it runs in the docker sandbox or on the VM.
for d in "${CARGO_HOME:-$HOME/.cargo}/bin" "$HOME/.cargo/bin" /usr/local/cargo/bin /root/.cargo/bin; do
  [ -d "$d" ] && case ":$PATH:" in *":$d:"*) ;; *) PATH="$d:$PATH" ;; esac
done
[ -f "${CARGO_HOME:-$HOME/.cargo}/env" ] && . "${CARGO_HOME:-$HOME/.cargo}/env"
export PATH

ROOT="$(git rev-parse --show-toplevel)"
cd "$ROOT"

EXCLUDE=(--exclude freeq-av-client --exclude freeq-eliza --exclude freeq-av --exclude freeq-av-image)
export RUSTFLAGS="-D warnings"   # CI treats warnings as errors
fail=0

step() { echo; echo "━━━ $* ━━━"; }
run()  { echo "\$ $*"; "$@" || { echo "✗ FAILED: $*"; fail=1; }; }

step "rustfmt"
run cargo fmt --all -- --check

step "cargo check (workspace, CI exclusions)"
run cargo check --workspace "${EXCLUDE[@]}"

step "clippy -D warnings"
run cargo clippy --workspace "${EXCLUDE[@]}" -- -D warnings

step "cargo test (workspace, CI exclusions)"
run cargo test --workspace "${EXCLUDE[@]}"

# freeq-app (TypeScript) — only when the agent touched it, and only if a Node
# toolchain is present (the executor VM installs it once at provision time).
if git diff --name-only HEAD | grep -q '^freeq-app/'; then
  if command -v npm >/dev/null 2>&1; then
    step "freeq-app vitest"
    ( cd freeq-app && run npm ci --no-audit --no-fund && run npx vitest run )
  else
    echo "⚠ freeq-app changed but npm is absent — cannot verify the app gate."
    fail=1
  fi
fi

echo
if [ "$fail" -eq 0 ]; then
  echo "✅ verify.sh: all gates passed"
else
  echo "❌ verify.sh: one or more gates failed"
fi
exit "$fail"
