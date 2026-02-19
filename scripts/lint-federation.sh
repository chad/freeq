#!/usr/bin/env bash
#
# lint-federation.sh — catch local-only lookups in federation-aware code
#
# This script greps for patterns that indicate a command handler is using
# nick_to_session or remote_members directly instead of going through the
# canonical routing/resolver layer. Every match is a potential asymmetric
# federation bug (works A→B, fails B→A).
#
# Run in CI alongside tests. Exit 1 if violations found.
#
# ALLOWED locations (the resolvers/routing themselves):
#   - connection/helpers.rs (resolve_channel_target, resolve_network_target)
#   - connection/routing.rs (relay_to_nick)
#   - server.rs (S2S message handlers — they ARE the receiving side)
#   - web.rs (web API — not federation-facing)
#
# FLAGGED locations (command handlers that should use resolvers):
#   - connection/messaging.rs
#   - connection/channel.rs
#   - connection/queries.rs (lower priority but still flagged)
#
set -euo pipefail

cd "$(dirname "$0")/.."

VIOLATIONS=0
RED='\033[0;31m'
YELLOW='\033[0;33m'
GREEN='\033[0;32m'
NC='\033[0m'

echo "🔍 Checking for local-only federation patterns..."
echo ""

# ── Pattern 1: raw nick_to_session.get() in command handlers ──────────
# These should use resolve_channel_target, resolve_network_target, or relay_to_nick
echo "── nick_to_session.get() in command handlers ──"
HITS=$(rg -n 'nick_to_session.*\.get\(' \
    freeq-server/src/connection/messaging.rs \
    freeq-server/src/connection/channel.rs \
    2>/dev/null || true)

if [ -n "$HITS" ]; then
    echo -e "${RED}VIOLATION: raw nick_to_session.get() in command handler${NC}"
    echo "$HITS"
    echo ""
    echo "  → Use relay_to_nick() for PM routing"
    echo "  → Use resolve_channel_target() for channel operations"
    echo "  → Use resolve_network_target() for network-wide lookups"
    echo ""
    VIOLATIONS=$((VIOLATIONS + $(echo "$HITS" | wc -l)))
else
    echo -e "${GREEN}  ✓ No raw nick_to_session in command handlers${NC}"
fi
echo ""

# ── Pattern 2: remote_members.contains_key() as a routing gate ────────
# remote_members is a display cache. Using it to decide whether to relay
# a message causes asymmetric failures when sync hasn't completed.
echo "── remote_members.contains_key() as routing gate ──"
HITS=$(rg -n 'remote_members\.contains_key\(' \
    freeq-server/src/connection/messaging.rs \
    2>/dev/null || true)

if [ -n "$HITS" ]; then
    echo -e "${RED}VIOLATION: remote_members used as routing gate in messaging${NC}"
    echo "$HITS"
    echo ""
    echo "  → Use relay_to_nick() instead — it routes through S2S without"
    echo "    requiring the target to be in any channel's remote_members."
    echo ""
    VIOLATIONS=$((VIOLATIONS + $(echo "$HITS" | wc -l)))
else
    echo -e "${GREEN}  ✓ No remote_members routing gates in messaging${NC}"
fi
echo ""

# ── Pattern 3: ad-hoc remote_members scan in channel.rs ───────────────
# channel.rs should use resolve_channel_target / resolve_network_target
echo "── ad-hoc remote_members scan in channel.rs ──"
HITS=$(rg -n 'remote_members\.(contains_key|get)\(' \
    freeq-server/src/connection/channel.rs \
    2>/dev/null || true)

if [ -n "$HITS" ]; then
    echo -e "${YELLOW}WARNING: direct remote_members access in channel.rs${NC}"
    echo "$HITS"
    echo ""
    echo "  → Verify these are display-only (NAMES listing) or post-resolver."
    echo "  → If used to gate an ACTION (kick, mode, invite), use a resolver."
    echo ""
    # Don't count as violations — some are legitimate (removing after resolve)
else
    echo -e "${GREEN}  ✓ No ad-hoc remote_members scans in channel.rs${NC}"
fi
echo ""

# ── Pattern 4: ERR_NOSUCHNICK in paths that have S2S peers ───────────
# If we have S2S peers, returning "no such nick" is almost always wrong
# for PRIVMSG — the nick might exist on a peer we haven't synced with.
echo "── ERR_NOSUCHNICK usage audit ──"
HITS=$(rg -n 'ERR_NOSUCHNICK' \
    freeq-server/src/connection/messaging.rs \
    freeq-server/src/connection/channel.rs \
    freeq-server/src/connection/queries.rs \
    2>/dev/null || true)

if [ -n "$HITS" ]; then
    echo -e "${YELLOW}INFO: ERR_NOSUCHNICK locations (verify each is after federation check):${NC}"
    echo "$HITS"
    echo ""
else
    echo -e "${GREEN}  ✓ No ERR_NOSUCHNICK in command handlers${NC}"
fi
echo ""

# ── Summary ───────────────────────────────────────────────────────────
if [ "$VIOLATIONS" -gt 0 ]; then
    echo -e "${RED}✗ $VIOLATIONS violation(s) found — fix before merging${NC}"
    echo ""
    echo "See freeq-server/src/connection/routing.rs for the architectural rules."
    exit 1
else
    echo -e "${GREEN}✓ No federation routing violations${NC}"
    exit 0
fi
