#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
# freeq Policy & Authority Framework — Live Demo
# ─────────────────────────────────────────────────────────────────────────────
#
# This script sets up a local freeq server and walks you through the demo.
# It does NOT start the server — you do that in a separate terminal.
#
# Usage:
#   1. Terminal 1: cargo run --release --bin freeq-server -- \
#        --listen-addr 127.0.0.1:16799 --web-addr 127.0.0.1:8080
#   2. Terminal 2: ./scripts/demo.sh
#
set -euo pipefail

SERVER="127.0.0.1"
IRC_PORT="16799"
WEB_PORT="8080"
CHANNEL="#freeq-dev"
ENCODED_CHANNEL="%23freeq-dev"

bold=$(tput bold)
reset=$(tput sgr0)
cyan=$(tput setaf 6)
green=$(tput setaf 2)
yellow=$(tput setaf 3)
red=$(tput setaf 1)
dim=$(tput dim)

banner() {
    echo
    echo "${bold}${cyan}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${reset}"
    echo "${bold}${cyan}  $1${reset}"
    echo "${bold}${cyan}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${reset}"
    echo
}

step() {
    echo "${bold}${green}▸ $1${reset}"
}

info() {
    echo "${dim}  $1${reset}"
}

pause() {
    echo
    echo "${yellow}  Press Enter to continue...${reset}"
    read -r
}

# ─── Check server is running ─────────────────────────────────────────────────

banner "freeq Policy Framework Demo"

echo "Checking server at ${SERVER}:${WEB_PORT}..."
if ! curl -sf "http://${SERVER}:${WEB_PORT}/api/v1/health" > /dev/null 2>&1; then
    echo "${red}${bold}Server not running!${reset}"
    echo
    echo "Start it in another terminal:"
    echo "  cargo run --release --bin freeq-server -- \\"
    echo "    --listen-addr ${SERVER}:${IRC_PORT} --web-addr ${SERVER}:${WEB_PORT}"
    exit 1
fi
echo "${green}✓ Server is running${reset}"

# ─── Scene 1: Normal IRC ─────────────────────────────────────────────────────

banner "Scene 1: Normal IRC — Everything Works"

step "Any IRC client can connect. No auth required."
info "Open your favorite IRC client and connect to ${SERVER}:${IRC_PORT}"
info "Or use netcat to prove it:"
echo
echo "  ${dim}echo -e 'NICK guest42\\r\\nUSER guest 0 * :Guest\\r\\nJOIN #general\\r\\n' | nc ${SERVER} ${IRC_PORT}${reset}"
echo
step "Channels without policies are open. This is standard IRC."

pause

# ─── Scene 2: Set a Policy ───────────────────────────────────────────────────

banner "Scene 2: Channel Op Sets a Policy"

step "Connect as an authenticated user (you need a Bluesky account)."
info "Use the web client at http://127.0.0.1:8080 or the TUI."
echo
step "Create ${CHANNEL} and set a policy:"
echo
echo "  ${bold}/join ${CHANNEL}${reset}"
echo "  ${bold}/POLICY ${CHANNEL} SET By joining you agree to the freeq Code of Conduct: be respectful, no spam, no harassment.${reset}"
echo
info "The server will respond with:"
info "  Policy set for ${CHANNEL} (version 1, rules_hash=a1b2c3..., policy_id=d4e5f6...)"
info "  You (the op) are auto-attested — you can always rejoin."

pause

# ─── Scene 3: Guest Gets Rejected ────────────────────────────────────────────

banner "Scene 3: Guest Tries to Join — Rejected"

step "A guest (unauthenticated) user tries to join ${CHANNEL}:"
echo
echo "  ${dim}NICK guest99${reset}"
echo "  ${dim}USER guest 0 * :Guest${reset}"
echo "  ${dim}JOIN ${CHANNEL}${reset}"
echo
info "Server responds:"
echo "  ${red}477 guest99 ${CHANNEL} :This channel requires authentication — sign in to join${reset}"
echo
step "This is the key: standard IRC clients still work for open channels."
step "Policy-gated channels require identity. No workarounds."

pause

# ─── Scene 4: Authenticated User Accepts Policy ──────────────────────────────

banner "Scene 4: Authenticated User Joins via Policy"

step "A second user signs in with Bluesky (web client or TUI)."
echo
step "They check the policy:"
echo "  ${bold}/POLICY ${CHANNEL} INFO${reset}"
echo
info "  Policy for ${CHANNEL}:"
info "    Version: 1"
info "    Requirement: ACCEPT(a1b2c3d4e5f6...)"
info "    Validity: JoinTime"
echo
step "They accept:"
echo "  ${bold}/POLICY ${CHANNEL} ACCEPT${reset}"
echo
info "  Policy accepted for ${CHANNEL} — role: member. You may now JOIN."
echo
step "Now they can join:"
echo "  ${bold}/join ${CHANNEL}${reset}"
echo
info "  ✓ Joined with verified badge (Bluesky identity)"

pause

# ─── Scene 5: The Audit Trail ────────────────────────────────────────────────

banner "Scene 5: Cryptographic Audit Trail"

step "Every join is logged. No DIDs in the log (privacy-preserving)."
echo
echo "  ${bold}curl -s http://${SERVER}:${WEB_PORT}/api/v1/policy/${ENCODED_CHANNEL}/transparency | python3 -m json.tool${reset}"
echo

# Actually run it if server is up
if curl -sf "http://${SERVER}:${WEB_PORT}/api/v1/policy/${ENCODED_CHANNEL}/transparency" > /dev/null 2>&1; then
    curl -s "http://${SERVER}:${WEB_PORT}/api/v1/policy/${ENCODED_CHANNEL}/transparency" | python3 -m json.tool 2>/dev/null || echo "  (no entries yet — set a policy first)"
else
    info "(run the curl command after setting a policy)"
fi

echo
step "Check current policy:"
echo
echo "  ${bold}curl -s http://${SERVER}:${WEB_PORT}/api/v1/policy/${ENCODED_CHANNEL} | python3 -m json.tool${reset}"
echo

if curl -sf "http://${SERVER}:${WEB_PORT}/api/v1/policy/${ENCODED_CHANNEL}" > /dev/null 2>&1; then
    curl -s "http://${SERVER}:${WEB_PORT}/api/v1/policy/${ENCODED_CHANNEL}" | python3 -m json.tool 2>/dev/null || echo "  (no policy set yet)"
else
    info "(run the curl command after setting a policy)"
fi

echo
step "Check a specific user's membership:"
echo
echo "  ${bold}curl -s http://${SERVER}:${WEB_PORT}/api/v1/policy/${ENCODED_CHANNEL}/membership/did:plc:... | python3 -m json.tool${reset}"

pause

# ─── Scene 6: Role Escalation ────────────────────────────────────────────────

banner "Scene 6: Role Escalation (The Big Idea)"

step "The policy DSL is composable. Imagine:"
echo
cat << 'POLICY'
  POLICY #myproject SET {
    "type": "ALL",
    "requirements": [
      { "type": "ACCEPT", "hash": "<code-of-conduct-hash>" },
    ]
  }

  With role escalation:
    "op" role requires:
      ALL(
        ACCEPT(code-of-conduct),
        PRESENT(github_membership, issuer=github)
      )
POLICY
echo
step "Regular users who accept the CoC get 'member' role."
step "GitHub org members who accept the CoC get 'op' role → auto +o on join."
echo
info "This is the same POLICY SET command. No code changes."
info "The requirement DSL supports: ACCEPT, PRESENT, PROVE, ALL, ANY, NOT"
info "Verifiers are pluggable — GitHub, email domain, NFT, anything."

pause

# ─── Wrap Up ──────────────────────────────────────────────────────────────────

banner "What Just Happened"

echo "  ${bold}1.${reset} IRC server runs normally — no protocol breakage"
echo "  ${bold}2.${reset} Channel ops can gate channels with cryptographic policies"
echo "  ${bold}3.${reset} Users authenticate via AT Protocol (Bluesky) — real identity"
echo "  ${bold}4.${reset} Policy acceptance is recorded with signed attestations"
echo "  ${bold}5.${reset} Privacy-preserving transparency log (no DIDs leaked)"
echo "  ${bold}6.${reset} Role escalation maps attestation roles → IRC modes"
echo "  ${bold}7.${reset} Policies federate to S2S peers automatically"
echo "  ${bold}8.${reset} Everything is auditable via REST API"
echo
echo "${bold}${cyan}HTTP API Endpoints:${reset}"
echo "  GET  /api/v1/policy/{channel}                — current policy"
echo "  GET  /api/v1/policy/{channel}/history         — full version chain"
echo "  POST /api/v1/policy/{channel}/join            — submit evidence"
echo "  GET  /api/v1/policy/{channel}/membership/{did} — check membership"
echo "  GET  /api/v1/policy/{channel}/transparency    — audit log"
echo "  GET  /api/v1/authority/{hash}                 — authority set"
echo
echo "${bold}${cyan}IRC Commands:${reset}"
echo "  POLICY #channel SET <rules text>   — create/update policy (ops)"
echo "  POLICY #channel INFO               — view current policy"
echo "  POLICY #channel ACCEPT             — accept and get attestation"
echo "  POLICY #channel CLEAR              — remove policy (ops)"
echo
echo "${bold}Source: ${reset}https://github.com/chad/freeq"
echo "${bold}Spec:   ${reset}https://gist.github.com/chad/9569f5265bfc3b6f5764d404118038b8"
echo
