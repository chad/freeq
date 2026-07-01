#!/bin/bash
# Deploy freeq-site to Miren
# Copies docs from repo root before deploying

set -e
cd "$(dirname "$0")"

# Copy docs from parent repo (these get uploaded with the deploy)
rm -rf docs
cp -r ../docs ./docs

# Write git commit hash for the /version endpoint
git -C .. rev-parse --short HEAD 2>/dev/null > .git_commit || echo "unknown" > .git_commit

echo "Deploying freeq-site (commit: $(cat .git_commit))..."
# Pin the target cluster: the freeq-site app lives on BlueYard. Without this the
# deploy targets whatever cluster is currently active (`miren cluster`), which
# 403s if that's a cluster this identity has no deploy rights on.
miren deploy -f -C BlueYard

echo "Deployed! Docs will be at https://www.freeq.at/docs/"
