#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
# Set up demo policy on a running freeq server via raw IRC commands.
#
# Usage: ./scripts/setup-demo-policy.sh [server] [port]
#   Default: 127.0.0.1 16799
#
# This connects as a bot, creates #freeq-dev, sets a policy, then disconnects.
# You must have the server running first.
# ─────────────────────────────────────────────────────────────────────────────
set -euo pipefail

SERVER="${1:-127.0.0.1}"
PORT="${2:-16799}"
NICK="PolicyBot$$"
CHANNEL="#freeq-dev"

echo "Connecting to ${SERVER}:${PORT} as ${NICK}..."

# Send IRC commands with proper timing
{
    sleep 0.5
    echo -e "NICK ${NICK}\r"
    echo -e "USER policybot 0 * :Policy Setup Bot\r"
    sleep 1
    echo -e "JOIN ${CHANNEL}\r"
    sleep 0.5
    echo -e "POLICY ${CHANNEL} SET By joining this channel you agree to the freeq Code of Conduct: be respectful, constructive, and inclusive. No spam, harassment, or off-topic flooding. Violations may result in removal. Full text: https://freeq.at/conduct\r"
    sleep 0.5
    echo -e "POLICY ${CHANNEL} INFO\r"
    sleep 0.5
    echo -e "PRIVMSG ${CHANNEL} :Policy framework is live! New users: /POLICY ${CHANNEL} INFO to see the rules, /POLICY ${CHANNEL} ACCEPT to join.\r"
    sleep 0.5
    echo -e "QUIT :Setup complete\r"
} | nc -q 3 "${SERVER}" "${PORT}" 2>/dev/null || true

echo "Done. Policy set on ${CHANNEL}."
echo
echo "Note: The bot was a guest, so the policy ACCEPT gate won't work"
echo "until an authenticated op confirms it. Connect with Bluesky auth"
echo "to set the policy as a real user."
