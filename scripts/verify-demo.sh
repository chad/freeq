#!/bin/bash
# Demo: Verify message signatures on freeq
#
# Usage:
#   ./scripts/verify-demo.sh                    # verify most recent signed message
#   ./scripts/verify-demo.sh <msgid>            # verify specific message
#   ./scripts/verify-demo.sh --channel '#demo'  # check a different channel

set -euo pipefail

SERVER="https://irc.freeq.at"
CHANNEL="${2:-#freeq}"
CHANNEL_ENC=$(python3 -c "import urllib.parse; print(urllib.parse.quote('$CHANNEL'))")

if [ "${1:-}" = "--channel" ]; then
  CHANNEL="${2:-#freeq}"
  CHANNEL_ENC=$(python3 -c "import urllib.parse; print(urllib.parse.quote('$CHANNEL'))")
  MSGID=""
elif [ -n "${1:-}" ]; then
  MSGID="$1"
else
  MSGID=""
fi

echo "╔══════════════════════════════════════════════════════════╗"
echo "║          freeq message signature verification           ║"
echo "╚══════════════════════════════════════════════════════════╝"
echo

# If no msgid given, find the most recent signed message
if [ -z "$MSGID" ]; then
  echo "→ Finding most recent signed message in $CHANNEL..."
  MSGID=$(curl -s "$SERVER/api/v1/channels/$CHANNEL_ENC/history?limit=20" | \
    python3 -c "
import json, sys
msgs = json.load(sys.stdin)
for m in reversed(msgs):
    if '+freeq.at/sig' in m.get('tags', {}) and m.get('msgid'):
        print(m['msgid'])
        break
" 2>/dev/null)

  if [ -z "$MSGID" ]; then
    echo "  ✗ No signed messages found in $CHANNEL"
    exit 1
  fi
  echo "  Found: $MSGID"
  echo
fi

# Fetch verification
echo "→ Fetching verification for $MSGID..."
RESULT=$(curl -s "$SERVER/api/v1/verify/$MSGID")

if echo "$RESULT" | python3 -c "import json,sys; json.load(sys.stdin)" 2>/dev/null; then
  :
else
  echo "  ✗ Message not found (may have rolled out of history)"
  exit 1
fi

echo
echo "── Message ──────────────────────────────────────────────"
echo "$RESULT" | python3 -c "
import json, sys
r = json.load(sys.stdin)
print(f\"  Channel:   {r['channel']}\")
print(f\"  From:      {r['from']}\")
print(f\"  DID:       {r.get('sender_did', 'unknown')}\")
print(f\"  Text:      {r['text']}\")
print(f\"  Timestamp: {r['timestamp']}\")
print(f\"  MsgID:     {r['msgid']}\")
"

echo
echo "── Signature ────────────────────────────────────────────"
echo "$RESULT" | python3 -c "
import json, sys
r = json.load(sys.stdin)
sig = r.get('signature', 'none')
print(f'  Signature (base64url):')
print(f'  {sig}')
print()
print(f'  Canonical form (what was signed):')
canonical = r.get('canonical_form', '')
# Show with visible null bytes
display = canonical.replace(chr(0), ' \\\\0 ')
print(f'  {display}')
print()
v = r.get('verification', {})
print(f'  Server public key:  {v.get(\"server_public_key\", \"n/a\")}')
print(f'  Client public key:  {v.get(\"client_public_key\", \"n/a\")}')
"

echo
echo "── Verification ─────────────────────────────────────────"
echo "$RESULT" | python3 -c "
import json, sys
r = json.load(sys.stdin)
v = r.get('verification', {})
valid = v.get('valid', False)
by = v.get('verified_by', 'none')
if valid:
    if by == 'client-session-key':
        print('  ✓ VALID — signed by client session key (true non-repudiation)')
        print('  The server CANNOT have forged this signature.')
    elif by == 'server-key':
        print('  ✓ VALID — signed by server attestation key')
        print('  Server attests this DID sent this message.')
    else:
        print(f'  ✓ VALID — verified by: {by}')
else:
    print('  ✗ NOT VERIFIED against current keys')
    print('  (Session key may have rotated since this message was sent)')
"

echo
echo "── Verify it yourself ─────────────────────────────────────"
echo "$RESULT" | python3 -c "
import json, sys, base64
r = json.load(sys.stdin)
sig = r.get('signature', '')
canonical = r.get('canonical_form', '')
v = r.get('verification', {})
pk = v.get('client_public_key') or v.get('server_public_key', '')
print('  # Python one-liner to verify independently:')
print(f'  python3 -c \"')
print(f'  import base64')
print(f'  from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PublicKey')
print(f'  pk = Ed25519PublicKey.from_public_bytes(base64.urlsafe_b64decode(\\\"{pk}=\\\"))')
print(f'  sig = base64.urlsafe_b64decode(\\\"{sig}=\\\")')
# Show canonical with escaped nulls
canon_escaped = canonical.replace(chr(0), '\\\\x00')
print(f'  pk.verify(sig, b\\\"{canon_escaped}\\\")')
print(f'  print(\\\"✓ Signature valid\\\")\"')
"

echo
echo "  # Raw JSON:"
echo "  curl -s $SERVER/api/v1/verify/$MSGID | jq"
echo
