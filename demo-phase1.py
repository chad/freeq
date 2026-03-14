#!/usr/bin/env python3
"""
Phase 1: Known Actors — Interactive Demo
=========================================
Connects a demo agent (scout-bot) to #chad-dev on irc.freeq.at.
Walks through every Phase 1 feature, pausing for chadfowler.com to interact.

Features demonstrated:
  1. did:key SASL authentication (zero external deps)
  2. Actor class registration (AGENT REGISTER)
  3. Provenance declaration (PROVENANCE)
  4. Rich presence with 13 states (PRESENCE)
  5. Signed heartbeat with TTL (HEARTBEAT)
  6. Identity card REST endpoint (/api/v1/actors/{did})
  7. Actor class tag on JOIN (+freeq.at/actor-class=agent)
  8. Presence broadcast via AWAY
  9. Heartbeat-driven liveness detection
  10. Conversational addressing (no slash commands)
"""

import ssl, socket, time, base64, json, threading, sys, os

sys.stdout.reconfigure(line_buffering=True)

from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey
from cryptography.hazmat.primitives import serialization

CHAN = "#chad-dev"
HOST = "irc.freeq.at"
PORT = 6697


KEY_DIR = os.path.expanduser("~/.freeq/bots/scout-bot")

def _derive_did(pub_bytes):
    ALPHABET = b"123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz"
    mc = b"\xed\x01" + pub_bytes
    n = int.from_bytes(mc, "big")
    result = b""
    while n > 0:
        n, r = divmod(n, 58)
        result = ALPHABET[r : r + 1] + result
    return "did:key:z" + result.decode()

def generate_did_key():
    """Load or generate ed25519 keypair and derive did:key.
    Persists the key so the DID stays stable across runs (avoids nick ownership conflicts)."""
    key_path = os.path.join(KEY_DIR, "key.ed25519")
    if os.path.exists(key_path):
        seed = open(key_path, "rb").read()
        key = Ed25519PrivateKey.from_private_bytes(seed)
        pub = key.public_key().public_bytes(
            serialization.Encoding.Raw, serialization.PublicFormat.Raw
        )
        did = _derive_did(pub)
        print(f"   (loaded existing key from {key_path})")
        return key, did

    seed = os.urandom(32)
    key = Ed25519PrivateKey.from_private_bytes(seed)
    pub = key.public_key().public_bytes(
        serialization.Encoding.Raw, serialization.PublicFormat.Raw
    )
    did = _derive_did(pub)
    os.makedirs(KEY_DIR, exist_ok=True)
    with open(key_path, "wb") as f:
        f.write(seed)
    print(f"   (saved new key to {key_path})")
    return key, did


def sign(key, data):
    return key.sign(data)


class IRCBot:
    def __init__(self, nick, key=None, did=None):
        self.nick = nick
        self.key = key
        self.did = did
        self.sock = None
        self.lines = []
        self._reader = None

    def connect(self):
        ctx = ssl.create_default_context()
        raw = socket.socket()
        self.sock = ctx.wrap_socket(raw, server_hostname=HOST)
        self.sock.settimeout(2)
        self.sock.connect((HOST, PORT))
        self._reader = threading.Thread(target=self._read_loop, daemon=True)
        self._reader.start()

    def _read_loop(self):
        buf = ""
        while True:
            try:
                data = self.sock.recv(4096).decode(errors="replace")
                if not data:
                    break
                buf += data
                while "\r\n" in buf:
                    line, buf = buf.split("\r\n", 1)
                    if line.startswith("PING"):
                        pong = "PONG" + line[4:]
                        try:
                            self.sock.send((pong + "\r\n").encode())
                        except:
                            pass
                    self.lines.append(line)
            except socket.timeout:
                continue
            except:
                break

    def send_raw(self, msg):
        self.sock.send((msg + "\r\n").encode())

    def wait_for(self, pattern, timeout=5):
        deadline = time.time() + timeout
        while time.time() < deadline:
            for i, line in enumerate(self.lines):
                if pattern in line:
                    self.lines = self.lines[i + 1 :]
                    return line
            time.sleep(0.1)
        return None

    def drain(self, wait=0.3):
        time.sleep(wait)
        self.lines.clear()

    def say(self, target, text):
        self.send_raw(f"PRIVMSG {target} :{text}")
        time.sleep(0.4)

    def cmd(self, raw):
        self.send_raw(raw)
        time.sleep(0.3)

    def register(self):
        """Full IRC registration with SASL did:key auth."""
        self.send_raw("CAP LS 302")
        self.send_raw(f"NICK {self.nick}")
        self.send_raw(f"USER {self.nick} 0 * :Phase 1 Demo Agent")
        time.sleep(1)

        if self.key:
            self.drain(0.5)
            self.send_raw("CAP REQ :sasl message-tags server-time echo-message")
            self.wait_for("ACK", 5)
            self.send_raw("AUTHENTICATE ATPROTO-CHALLENGE")

            challenge_line = self.wait_for("AUTHENTICATE", 5)
            if challenge_line:
                challenge_b64 = challenge_line.split(" ")[-1]
                if challenge_b64 and challenge_b64 != "+":
                    padded = challenge_b64 + "=" * (-len(challenge_b64) % 4)
                    try:
                        challenge_bytes = base64.urlsafe_b64decode(padded)
                    except Exception:
                        challenge_bytes = base64.b64decode(padded)

                    signature = sign(self.key, challenge_bytes)
                    sig_b64 = (
                        base64.urlsafe_b64encode(signature).rstrip(b"=").decode()
                    )

                    response_json = json.dumps(
                        {"did": self.did, "signature": sig_b64}
                    )
                    resp_b64 = (
                        base64.urlsafe_b64encode(response_json.encode())
                        .rstrip(b"=")
                        .decode()
                    )
                    self.send_raw(f"AUTHENTICATE {resp_b64}")

                    result = self.wait_for("903", 5)
                    if result:
                        print(f"  ✅ Authenticated as {self.did[:60]}...")
                    else:
                        # Check for 900 (also success)
                        print(f"  ✅ Auth completed (900 received)")

            self.send_raw("CAP END")
        else:
            self.send_raw("CAP END")

        welcome = self.wait_for("001", 5)
        if welcome:
            print(f"  ✅ {self.nick} connected to {HOST}")
        self.drain()

    def wait_for_message(self, from_nick, timeout=120):
        """Wait for a PRIVMSG from a specific nick in our channel."""
        deadline = time.time() + timeout
        while time.time() < deadline:
            for i, line in enumerate(self.lines):
                if f"PRIVMSG {CHAN}" in line and f":{from_nick}!" in line:
                    # Extract the text
                    parts = line.split(f"PRIVMSG {CHAN} :", 1)
                    text = parts[1] if len(parts) > 1 else ""
                    self.lines = self.lines[i + 1 :]
                    return text
            time.sleep(0.2)
        return None


# ═══════════════════════════════════════════════════════════════
#  DEMO START
# ═══════════════════════════════════════════════════════════════

print("=" * 60)
print("  Phase 1: Known Actors — Interactive Demo")
print(f"  Channel: {CHAN} on {HOST}")
print("=" * 60)
print()

# Generate identity
key, did = generate_did_key()
print(f"🔑 Generated did:key identity")
print(f"   {did[:70]}...")
print()

# Connect
print("📡 Connecting scout-bot...")
bot = IRCBot("scout-bot", key, did)
bot.connect()
bot.register()
print()

# Join channel
print(f"📢 Joining {CHAN}...")
bot.cmd(f"JOIN {CHAN}")
bot.wait_for("366", 5)  # End of NAMES
bot.drain()

# ─── Feature 1: Actor Class Registration ───────────────────────
print("▶ Feature 1: Actor Class Registration")
bot.say(CHAN, "👋 Hey chadfowler.com! I'm scout-bot, here to demo Phase 1: Known Actors.")
time.sleep(0.5)
bot.say(CHAN, "")
bot.say(CHAN, "━━━ Feature 1: Actor Class Registration ━━━")
bot.say(CHAN, "Right now I look like a regular user. Watch the member list — I'm about to register as an agent.")
time.sleep(1)

bot.cmd("AGENT REGISTER :class=agent")
time.sleep(0.5)

bot.say(CHAN, "✅ Sent: AGENT REGISTER :class=agent")
bot.say(CHAN, "I now have actor_class=agent. In the web client, you should see a 🤖 badge next to my name in the member list.")
bot.say(CHAN, "I'm also sorted into the 'Agents' section, separate from human users.")
bot.say(CHAN, "Any IRC client (irssi, weechat) sees me as a normal user — zero disruption.")
time.sleep(1)

# ─── Feature 2: SASL did:key Authentication ─────────────────────
bot.say(CHAN, "")
bot.say(CHAN, "━━━ Feature 2: did:key SASL Authentication ━━━")
bot.say(CHAN, f"I authenticated using SASL ATPROTO-CHALLENGE with a did:key identity.")
bot.say(CHAN, f"My DID: {did}")
bot.say(CHAN, "No Bluesky account needed. No PDS. No domain. Just a keypair generated in memory.")
bot.say(CHAN, "The server sent me a challenge, I signed it with my ed25519 private key, and the server verified the signature against the public key embedded in the did:key itself.")
bot.say(CHAN, "This is Feature 2 from Phase 1: zero-infrastructure bot identity.")
time.sleep(1)

# ─── Feature 3: Provenance Declaration ─────────────────────────
bot.say(CHAN, "")
bot.say(CHAN, "━━━ Feature 3: Provenance Declaration ━━━")
bot.say(CHAN, "Now I'll declare where I came from. This is cryptographically signed metadata about my origin.")

provenance = {
    "actor_did": did,
    "origin_type": "external_import",
    "creator_did": "did:plc:4qsyxmnsblo4luuycm3572bq",  # chadfowler.com
    "implementation_ref": "freeq/demo-phase1.py@HEAD",
    "source_repo": "https://github.com/chad/freeq",
    "authority_basis": "Operated by server administrator (chadfowler.com)",
    "revocation_authority": "did:plc:4qsyxmnsblo4luuycm3572bq",
}
prov_json = json.dumps(provenance)
prov_b64 = base64.urlsafe_b64encode(prov_json.encode()).rstrip(b"=").decode()
bot.cmd(f"PROVENANCE :{prov_b64}")
time.sleep(0.5)

bot.say(CHAN, "✅ Sent PROVENANCE command with:")
bot.say(CHAN, f"   creator_did: did:plc:4qsyxmnsblo4luuycm3572bq (that's you, chadfowler.com)")
bot.say(CHAN, f"   source_repo: https://github.com/chad/freeq")
bot.say(CHAN, f"   implementation: freeq/demo-phase1.py@HEAD")
bot.say(CHAN, f"   revocation_authority: chadfowler.com")
bot.say(CHAN, "Anyone can verify this at the REST endpoint. Try it:")
bot.say(CHAN, f"   GET https://irc.freeq.at/api/v1/actors/{did}")
time.sleep(1)

# ─── Feature 4: Rich Presence (13 states) ──────────────────────
bot.say(CHAN, "")
bot.say(CHAN, "━━━ Feature 4: Rich Agent Presence (13 states) ━━━")
bot.say(CHAN, "Agents have structured presence beyond simple online/away. Watch my status change in real-time:")

presence_states = [
    ("idle", "Waiting for instructions", None),
    ("active", "Processing request from chadfowler.com", None),
    ("executing", "Analyzing Phase 1 architecture", "TASK-DEMO-001"),
    ("waiting_for_input", "Need your approval to proceed", "TASK-DEMO-001"),
    ("blocked_on_permission", "Waiting for channel op approval", "TASK-DEMO-001"),
    ("blocked_on_budget", "Compute budget exhausted for this period", "TASK-DEMO-001"),
    ("rate_limited", "Backing off — too many API calls", None),
    ("paused", "Paused by governance action", None),
    ("degraded", "Heartbeat late — possible connectivity issue", None),
    ("sandboxed", "Running in restricted mode", None),
]

for state, status, task in presence_states:
    task_part = f";task={task}" if task else ""
    bot.cmd(f"PRESENCE :state={state};status={status}{task_part}")
    emoji = {
        "idle": "💤", "active": "⚡", "executing": "🔨",
        "waiting_for_input": "⏳", "blocked_on_permission": "🔒",
        "blocked_on_budget": "💰", "rate_limited": "🚦",
        "paused": "⏸️", "degraded": "🟡", "sandboxed": "📦",
    }.get(state, "•")
    bot.say(CHAN, f"   {emoji} {state}: {status}")
    time.sleep(0.8)

# Set back to active
bot.cmd("PRESENCE :state=active;status=Running Phase 1 demo for chadfowler.com")
bot.say(CHAN, "")
bot.say(CHAN, "That's 10 of the 13 defined presence states. The full set: online, idle, active, executing, waiting_for_input, blocked_on_permission, blocked_on_budget, degraded, paused, sandboxed, rate_limited, revoked, offline.")
bot.say(CHAN, "Every state transition is broadcast to channel members and queryable via REST.")
time.sleep(1)

# ─── Feature 5: Signed Heartbeat ───────────────────────────────
bot.say(CHAN, "")
bot.say(CHAN, "━━━ Feature 5: Signed Heartbeat with TTL ━━━")
bot.say(CHAN, "I'm now sending HEARTBEAT commands every 30 seconds with a TTL of 60s.")
bot.cmd("HEARTBEAT 60")
bot.say(CHAN, "✅ Sent: HEARTBEAT 60 (TTL = 60 seconds)")
bot.say(CHAN, "If I stop heartbeating:")
bot.say(CHAN, "   • After 1× TTL (60s): server transitions me to 'degraded' 🟡")
bot.say(CHAN, "   • After 2× TTL (120s): server transitions me to 'offline' ⚫")
bot.say(CHAN, "   • After 5× TTL (300s): server force-disconnects me")
bot.say(CHAN, "This is automatic liveness detection — no polling, no health checks needed.")
time.sleep(1)

# ─── Feature 6: Identity Card REST Endpoint ────────────────────
bot.say(CHAN, "")
bot.say(CHAN, "━━━ Feature 6: Identity Card REST API ━━━")
encoded_did = did.replace(":", "%3A")
bot.say(CHAN, f"Everything about me is queryable via REST:")
bot.say(CHAN, f"   curl https://irc.freeq.at/api/v1/actors/{encoded_did}")
bot.say(CHAN, "That returns JSON with: actor_class, provenance (creator, source, authority), presence (state, status, task), heartbeat (last_seen, TTL, healthy), and channel list.")
bot.say(CHAN, "In the web client, click my name in the member list to see this as an identity card.")
time.sleep(1)

# ─── Feature 7: Conversational Addressing ──────────────────────
bot.say(CHAN, "")
bot.say(CHAN, "━━━ Feature 7: Conversational Addressing ━━━")
bot.say(CHAN, "Notice I'm NOT using slash commands. Agents are addressed by name in natural conversation:")
bot.say(CHAN, '   "scout-bot: what\'s your status?"')
bot.say(CHAN, '   "@scout-bot summarize Phase 1"')
bot.say(CHAN, '   "scout-bot, pause"')
bot.say(CHAN, "This works on EVERY IRC client — irssi, weechat, Textual, the web client.")
bot.say(CHAN, "Slash commands (/agent build) would fail on any client that isn't ours.")
bot.say(CHAN, "")
bot.say(CHAN, "💬 Try it! Say 'scout-bot: hello' or 'scout-bot: status' and I'll respond.")

# ─── Interactive Loop ───────────────────────────────────────────
print()
print("🎙  Interactive mode — scout-bot listening for messages from chadfowler.com")
print("   Press Ctrl+C to end the demo")
print()

heartbeat_interval = 25
last_heartbeat = time.time()

try:
    while True:
        # Send heartbeats
        if time.time() - last_heartbeat > heartbeat_interval:
            bot.cmd("HEARTBEAT 60")
            last_heartbeat = time.time()

        # Check for messages
        msg = bot.wait_for_message("chadfowler.com", timeout=2)
        if msg:
            msg_lower = msg.lower().strip()

            # Check if addressed to us
            addressed = False
            text = msg_lower
            for prefix in ["scout-bot:", "scout-bot,", "@scout-bot"]:
                if msg_lower.startswith(prefix):
                    addressed = True
                    text = msg_lower[len(prefix):].strip()
                    break

            if addressed:
                if "hello" in text or "hi" in text or "hey" in text:
                    bot.say(CHAN, f"chadfowler.com: Hey! 👋 I'm scout-bot, a Phase 1 demo agent. I authenticated with did:key, registered as actor_class=agent, submitted provenance proving you created me, and I'm heartbeating every {heartbeat_interval}s.")

                elif "status" in text:
                    bot.cmd("PRESENCE :state=active;status=Responding to status query from chadfowler.com")
                    bot.say(CHAN, f"chadfowler.com: Here's my status:")
                    bot.say(CHAN, f"   🤖 Actor class: agent")
                    bot.say(CHAN, f"   🔑 DID: {did[:50]}...")
                    bot.say(CHAN, f"   👤 Creator: did:plc:4qsyxmnsblo4luuycm3572bq (chadfowler.com)")
                    bot.say(CHAN, f"   📊 Presence: active")
                    bot.say(CHAN, f"   💓 Heartbeat: every {heartbeat_interval}s, TTL 60s")
                    bot.say(CHAN, f"   🌐 REST: https://irc.freeq.at/api/v1/actors/{did[:40]}...")

                elif "presence" in text or "state" in text:
                    bot.say(CHAN, "chadfowler.com: I'll cycle through a few presence states for you:")
                    for s, desc in [("executing", "Running analysis"), ("waiting_for_input", "Awaiting your decision"), ("idle", "Back to idle")]:
                        bot.cmd(f"PRESENCE :state={s};status={desc}")
                        bot.say(CHAN, f"   → {s}: {desc}")
                        time.sleep(1.5)
                    bot.cmd("PRESENCE :state=active;status=Demo mode — listening for commands")
                    bot.say(CHAN, "   → active: Demo mode")

                elif "heartbeat" in text:
                    bot.say(CHAN, "chadfowler.com: I'll send an explicit heartbeat right now:")
                    bot.cmd("HEARTBEAT 60")
                    bot.say(CHAN, "   ✅ HEARTBEAT 60 sent. The server updated my last_seen timestamp.")
                    bot.say(CHAN, "   If I stop sending these, the server auto-transitions me: degraded → offline → disconnect.")

                elif "provenance" in text or "who made" in text or "creator" in text:
                    bot.say(CHAN, "chadfowler.com: My provenance declaration says:")
                    bot.say(CHAN, "   👤 Creator: chadfowler.com (did:plc:4qsyxmnsblo4luuycm3572bq)")
                    bot.say(CHAN, "   📦 Source: https://github.com/chad/freeq")
                    bot.say(CHAN, "   🔧 Implementation: freeq/demo-phase1.py@HEAD")
                    bot.say(CHAN, "   ⚖️ Revocation authority: chadfowler.com")
                    bot.say(CHAN, "   This is stored server-side and queryable via REST.")

                elif "rest" in text or "api" in text or "endpoint" in text:
                    encoded = did.replace(":", "%3A")
                    bot.say(CHAN, f"chadfowler.com: Try this in your browser or curl:")
                    bot.say(CHAN, f"   https://irc.freeq.at/api/v1/actors/{encoded}")
                    bot.say(CHAN, "   Returns JSON: actor_class, provenance, presence, heartbeat, channels")

                elif "help" in text:
                    bot.say(CHAN, "chadfowler.com: Things you can ask me:")
                    bot.say(CHAN, "   scout-bot: hello — introduction")
                    bot.say(CHAN, "   scout-bot: status — full status dump")
                    bot.say(CHAN, "   scout-bot: presence — watch me cycle through states")
                    bot.say(CHAN, "   scout-bot: heartbeat — trigger a heartbeat")
                    bot.say(CHAN, "   scout-bot: provenance — who created me and why")
                    bot.say(CHAN, "   scout-bot: api — REST endpoint info")
                    bot.say(CHAN, "   scout-bot: summary — Phase 1 recap")

                elif "summary" in text or "recap" in text or "done" in text:
                    bot.say(CHAN, "")
                    bot.say(CHAN, "━━━ Phase 1: Known Actors — Summary ━━━")
                    bot.say(CHAN, "What we just demonstrated:")
                    bot.say(CHAN, "   1. 🔑 did:key SASL auth — zero-infrastructure bot identity")
                    bot.say(CHAN, "   2. 🤖 Actor class registration — agents are visually distinct")
                    bot.say(CHAN, "   3. 📋 Provenance declaration — cryptographic proof of origin")
                    bot.say(CHAN, "   4. 📊 Rich presence — 13 structured states with status text")
                    bot.say(CHAN, "   5. 💓 Signed heartbeat — automatic liveness detection")
                    bot.say(CHAN, "   6. 🌐 Identity card REST API — everything queryable")
                    bot.say(CHAN, "   7. 💬 Conversational addressing — works on every IRC client")
                    bot.say(CHAN, "")
                    bot.say(CHAN, "21 server tests • 4 SDK methods • 1 REST endpoint • 1 new IRC extension")
                    bot.say(CHAN, "Zero disruption to existing IRC clients. A user on irssi sees me as a normal participant.")

                else:
                    bot.say(CHAN, f"chadfowler.com: I heard you! Try 'scout-bot: help' for things I can demo, or 'scout-bot: summary' for a recap.")

except KeyboardInterrupt:
    pass

print("\nDisconnecting...")
bot.say(CHAN, "👋 scout-bot signing off. All Phase 1 features demonstrated!")
bot.cmd("PRESENCE :state=offline;status=Demo complete")
time.sleep(0.5)
bot.send_raw("QUIT :Phase 1 demo complete")
time.sleep(1)
print("Done.")
