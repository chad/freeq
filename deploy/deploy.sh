#!/usr/bin/env bash
set -euo pipefail

cd /home/chad/src/freeq

echo "==> Pulling latest..."
git pull --ff-only

echo "==> Building server (release)..."
cargo build --release --bin freeq-server

echo "==> Building web app..."
cd freeq-app
npm ci --silent
npm run build
cd ..

echo "==> Installing service file..."
sudo cp deploy/freeq-server.service /etc/systemd/system/freeq-server.service
sudo systemctl daemon-reload

echo "==> Restarting service..."
sudo systemctl restart freeq-server

echo "==> Status:"
sudo systemctl status freeq-server --no-pager
