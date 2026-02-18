# What's Different About Freeq

Freeq is a fully compatible IRC server. Any IRC client can connect and use it
normally. But underneath, several things work differently — and if you care
about identity, encryption, or federation, they might matter to you.

---

## Your Identity Is Yours

Standard IRC has no real concept of identity. You pick a nick, maybe register
it with NickServ, and hope nobody impersonates you on another network.

Freeq uses [AT Protocol](https://atproto.com) (the protocol behind Bluesky)
for authentication. Your IRC identity is your DID — the same cryptographic
identifier behind your Bluesky account. When you authenticate:

- Your nick is **bound to your DID**. No one else can use it — not on this
  server, and not on any federated server.
- WHOIS shows your **verified Bluesky handle** (e.g. `chadfowler.com`).
- Bans, invites, and channel ops are tied to your DID, not your nick or IP.
  They survive reconnects, nick changes, and even work across servers.

Authentication happens via SASL with a custom mechanism (`ATPROTO-CHALLENGE`).
The server sends a cryptographic challenge; your client signs it with your
AT Protocol credentials. **Your private keys never leave your machine.**

You can authenticate with:
- **Browser-based OAuth** (like logging into any Bluesky app)
- **App passwords** (for headless/bot use)
- **Direct cryptographic signatures** (if you manage your own keys)

If you don't authenticate, everything still works — you're just a guest.

---

## Channels Have Owners, Not Just Ops

In standard IRC, channel ownership is fragile. If everyone leaves, the
channel resets. Ops are granted per-session and disappear on disconnect.

Freeq introduces **DID-based channel authority**:

- The first authenticated user to create a channel becomes its **founder**.
  Founder status is permanent — stored in a database, replicated via CRDT,
  and survives server restarts.
- Channel ops can be granted **by DID**. If you're opped and you reconnect
  tomorrow from a different IP with a different nick, you're still an op.
- The founder can never be de-opped.

Bans work the same way: `MODE #channel +b did:plc:abc123` bans an identity,
not a hostmask. No more whack-a-mole with IP changes and nick switches.

---

## Servers Federate Without a Central Authority

Traditional IRC federation (RFC 2813) uses a spanning tree of trusted
servers with a shared, fragile namespace. Netsplits are catastrophic.
Most modern networks just don't federate.

Freeq servers federate over [iroh](https://iroh.computer) QUIC connections.
Each server is independent and maintains its own state. Channel authority
(founders, DID-based ops, topics) converges using
[Automerge CRDTs](https://automerge.org) — no timestamps, no conflicts,
no split-brain.

What syncs across servers:
- Channel messages, membership, topics
- DID-based ops and founder status
- Nick ownership

What stays local:
- TCP connections and sessions
- Rate limiting and connection state

Servers discover each other by iroh endpoint ID (a public key). There's no
DNS, no hub server, no registration process. You point your server at a
peer's endpoint ID and they're linked.

```sh
# That's it. Two servers, federated.
freeq-server --iroh
freeq-server --iroh --s2s-peers <other-server-endpoint-id>
```

---

## NAT Traversal Built In

IRC has always struggled with NAT. DCC doesn't work. Server linking
requires public IPs and open ports.

Freeq's iroh transport gives you QUIC connections that punch through NAT
automatically, with relay fallback when direct connections aren't possible.
This works for both client↔server and server↔server links.

The server advertises its iroh endpoint in `CAP LS`. Clients that support
it auto-upgrade — no configuration needed.

---

## End-to-End Encrypted Channels

Standard IRC messages are plaintext on the server. Always have been.

Freeq supports client-side AES-256-GCM encryption per channel. The server
sees only ciphertext. Everyone in the channel shares a passphrase; key
derivation uses HKDF-SHA256 salted with the channel name.

```
/encrypt my-secret-passphrase
```

There's also a DID-based encryption mode where the group key is derived
from the sorted DIDs of channel members — no shared secret needed, and
the key rotates automatically when membership changes.

---

## Peer-to-Peer DMs That Skip the Server

Want to talk to someone without any server in the middle? Freeq clients
can open direct iroh QUIC connections to each other for encrypted DMs.

```
/p2p start
/p2p connect <their-endpoint-id>
/p2p msg <id> hello
```

The server is never involved. Messages don't touch IRC at all. Peer
endpoint IDs are discoverable via WHOIS.

---

## Four Transports, One Protocol

IRC has always been TCP. Freeq speaks IRC over four transports, and you
can mix them freely in the same channel:

| Transport | What it gives you |
|-----------|-------------------|
| **TCP** (:6667) | Standard IRC — works with every client ever made |
| **TLS** (:6697) | Encrypted standard IRC |
| **WebSocket** | Browser clients, no plugins needed |
| **iroh QUIC** | NAT traversal, E2E encryption, P2P federation |

All four feed into the same protocol handler. The server doesn't care
how you got there.

---

## Message History That Just Works

When you join a channel, the server replays recent messages as standard
PRIVMSGs. No special client support needed — your existing IRC client
shows them. Clients that negotiate `server-time` and `batch` get proper
timestamps and grouping.

History is stored in SQLite and survives restarts.

---

## Rich Media Over IRC

Freeq uses IRCv3 message tags to carry structured media metadata alongside
plain text. The same message has two representations:

- **Tags**: content type, URL, dimensions, alt text (for rich clients)
- **Body**: description + URL (for every other client)

In WeeChat you see a clickable link. In the Freeq TUI you see an inline
media badge. The server never handles media bytes — images and files are
hosted on your AT Protocol PDS (Bluesky's blob storage).

---

## What Didn't Change

Freeq is still IRC. Specifically:

- Every standard IRC client works unmodified (as a guest)
- Channel modes, ops, bans, invites, kicks — all standard
- NAMES, WHO, WHOIS, LIST, MOTD — all standard
- CTCP ACTION (`/me`) works
- No new wire protocol — it's IRC lines on a TCP socket
- If you don't want authentication, don't use it
- If you don't want encryption, don't use it
- If you don't want federation, run one server

The goal is to make IRC better without making it something else.
