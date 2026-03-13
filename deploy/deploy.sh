#!/usr/bin/env bash
# Deploy freeq updates (run after setup.sh)
set -euo pipefail

REPO_DIR="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO_DIR"

echo "==> Pulling latest..."
git pull --ff-only

echo "==> Building server (release)..."
cargo build --release --bin freeq-server

echo "==> Building web app..."
cd freeq-app
npm ci --silent
npm run build
cd "$REPO_DIR"

echo "==> Restarting service..."
sudo systemctl restart freeq-server

echo "==> Status:"
sudo systemctl status freeq-server --no-pager
