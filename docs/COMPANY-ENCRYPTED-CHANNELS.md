# Private, End-to-End Encrypted Channels for Your Company

This tutorial shows how to run freeq so that a company gets **private channels
only its people can join, whose message content the server operator cannot
read** — with no shared passphrase. Membership is granted by your existing
identity provider (Google Workspace / Okta / Entra / any OIDC or SAML IdP), and
the channel's encryption key is delivered to each member sealed to their own
key. When someone leaves, a key rotation locks them out of future messages.

By the end you'll have:

1. A self-hosted freeq server, federation off, behind TLS.
2. An SSO gate so only `@yourco.com` identities can obtain channel access.
3. An end-to-end encrypted channel (`+E`) whose group key the server never sees.
4. A steward that seals the key to members on join and rotates it on departure.

> **Why self-host?** The strongest privacy guarantee — "the host cannot read our
> messages" — is only meaningful when *you* are the host. On a shared host, the
> `+E` group encryption below still hides message *content*, but you should read
> [Company Deployment Security Analysis](/docs/company-security/) for the full
> threat model (metadata is visible to the host regardless). This tutorial
> assumes you self-host; see [Self-Hosting End to End](/docs/self-hosting-e2e/).

---

## How it works (one diagram)

```
 (1) JOIN #eng ─▶ policy: REQUIRE oidc_domain @yourco.com ─▶ 403 + verify link
 (2) user logs into Google ─▶ /verify/oidc/callback checks hd=yourco.com
 (3) signed credential issued ─▶ member admitted to the channel
 (4) steward seals the group key to the member's X25519 key:
        POST /api/v1/channels/#eng/groupkeys   { epoch, keys: { did: EGK1... } }
 (5) member fetches + opens it:
        GET  /api/v1/channels/#eng/groupkeys  ─▶ EGK1 blob ─▶ open with own key
 (6) traffic:  PRIVMSG #eng :EG1:7:<nonce>:<ciphertext>     (server stores ct only)

 departure ─▶ steward rotate()s to epoch 8, re-seals to remaining members only
           ─▶ the person who left never gets epoch 8 → can't read new messages
```

The two ideas that make this safe:

- **The credential gates access, not the key.** A verifiable credential is not
  secret (the server issues and stores it). It decides *who may receive* the
  key. The key itself is a **random secret**, delivered sealed to each member's
  public key, so the server only ever relays ciphertext.
- **Rotation = revocation.** Each membership change mints a fresh key at a new
  "epoch." Old members keep old-epoch keys (so history stays readable to those
  who were present) but never receive new ones.

Full protocol: [VC-Bootstrapped E2E Channels](/docs/vc-e2e-channels/).

---

## Step 1 — Run the server (federation off, TLS on)

Follow [Self-Hosting End to End](/docs/self-hosting-e2e/) for the full install.
The privacy-relevant flags:

```bash
freeq-server \
  --bind 127.0.0.1:6667 \            # plaintext port: localhost only
  --tls-bind 0.0.0.0:6697 \          # TLS for IRC clients
  --tls-cert /etc/letsencrypt/live/irc.yourco.com/fullchain.pem \
  --tls-key  /etc/letsencrypt/live/irc.yourco.com/privkey.pem \
  --web-addr 127.0.0.1:8080 \        # behind nginx TLS
  --web-static-dir /opt/freeq/freeq-app/dist \
  --db-path  /opt/freeq/data/irc.db \
  --data-dir /opt/freeq/data \
  --server-name irc.yourco.com \
  --oper-dids did:plc:your-admin-did
  # NOTE: do NOT pass --iroh → federation stays off (no S2S surface).
```

Put nginx (TLS) in front of `:8080`, and — because REST read APIs are
gated per-channel, not per-user — keep the whole surface on your VPN or behind
an IP allowlist. See the hardening checklist in the self-hosting guide.

---

## Step 2 — Turn on the SSO gate (Google / OIDC)

The **OIDC verifier** turns a Google Workspace login into a signed credential
that proves the user is at your domain. Configure it via environment variables
(read at startup):

```bash
OIDC_CLIENT_ID=<your Google OAuth client id>
OIDC_CLIENT_SECRET=<secret>
OIDC_ALLOWED_DOMAIN=yourco.com
OIDC_REDIRECT_URL=https://irc.yourco.com/verify/oidc/callback
# Okta/Entra/Auth0 instead of Google? Override the endpoints:
# OIDC_AUTH_URL=...  OIDC_TOKEN_URL=...
```

Register `https://irc.yourco.com/verify/oidc/callback` as an authorized redirect
URI in your IdP. On restart you'll see `OIDC/SSO verifier configured` in the log,
and these routes go live:

```
GET /verify/oidc/start?subject_did=<user DID>&callback=<join callback>
GET /verify/oidc/callback
```

The verifier requires a **verified** email whose domain (or Google `hd` claim)
equals `OIDC_ALLOWED_DOMAIN`, then issues an `oidc_domain` credential with a
short 12-hour TTL — so re-authentication picks up offboarding within a day.

---

## Step 3 — Create the channel and require SSO

As a channel founder (or DID-op), create the channel, lock it down, and attach
the policy. From any IRC client or the web app:

```
/join #eng
/mode #eng +i            ; invite-only: no REST leak, must be admitted
/mode #eng +E            ; encrypted-only: server rejects any plaintext
/msg ChanServ POLICY #eng REQUIRE oidc_domain issuer=did:web:irc.yourco.com:verify
```

- `+i` makes the channel private (history/search/export return 403 over REST).
- `+E` enforces that **every** message is `ENC1`/`EG1` ciphertext — the server
  will reject anything else, so no plaintext can ever land in the channel.
- The `POLICY … REQUIRE` line makes JOIN conditional on a valid SSO credential.

Now a stranger who tries `/join #eng` is bounced to the Google login and only
gets in if they're `@yourco.com`.

---

## Step 4 — Run the key steward

The **steward** holds the current group key, seals it to each admitted member,
and rotates on departure. Run it as a small bot with a founder/DID-op identity.
Here's the core loop using the Rust SDK (`freeq_sdk::e2ee_group`):

```rust
use freeq_sdk::e2ee_group::GroupState;

// One-time: create the group for the channel.
let mut group = GroupState::create("#eng");

// On each admitted member (from JOIN + verified credential), fetch their
// published X25519 public key (pre-key bundle) and seal:
let members: Vec<(String, [u8; 32])> = current_members(); // (did, x25519_pub)
let sealed = group.seal_batch(&members);                  // [(did, "EGK1:...")]

// Push to the server (server stores opaque blobs, can't open them):
http.post(format!("{base}/api/v1/channels/%23eng/groupkeys"))
    .bearer_auth(&steward_session_id)
    .json(&json!({ "epoch": group.epoch, "keys": sealed_map(sealed) }))
    .send().await?;

// Post messages as ciphertext:
let wire = group.encrypt("Q3 board deck is in the drive")?; // "EG1:1:..."
client.privmsg("#eng", &wire).with_tag("+encrypted").await?;

// When someone LEAVES / is kicked / their credential expires:
group = group.rotate();                       // fresh secret, epoch += 1
let sealed = group.seal_batch(&remaining_members());
// ... POST again with the new epoch; the departed member is not included.
```

A member (web client) recovers the key and reads:

```ts
import { openBest, decryptGroup } from '@freeq/sdk';

// GET /api/v1/channels/#eng/groupkeys → { keys: [{epoch, sealed}] }
const state = await openBest(resp.keys.map(k => [k.epoch, k.sealed]), myX25519Key);
const text  = await decryptGroup(state!, incomingWire);  // decrypts EG1:...
```

The server relays and stores only `EG1`/`EGK1` blobs. It never holds a key it
can open — verifiable in the test suite:

```bash
cargo test -p freeq-server --test group_e2e   # full lifecycle + server-blind proof
cargo test -p freeq-sdk    e2ee_group          # crypto unit tests
```

---

## What this gives you, and what it doesn't

**You get:**
- Only `@yourco.com` identities can join (SSO-gated, not a shared password).
- The server operator cannot read message **content** (`+E` + sealed keys).
- Leaving the company removes future read access (rotation on departure).
- History stays readable to members who were present (old epochs retained).

**Be aware:**
- **Metadata** (who's in the channel, timing, channel names) is visible to the
  host. Self-hosting keeps that inside your walls.
- **Content a member already saw** stays readable to them; rotation protects
  *future* epochs. For post-compromise security at scale, the roadmap is MLS —
  see [VC-Bootstrapped E2E Channels](/docs/vc-e2e-channels/#upgrade-path-mls).
- On a **shared** host, verify pre-key authenticity before trusting a seal (a
  hostile host could substitute a bundle). Self-hosting sidesteps this.

For the complete threat model and hardening checklist, read
[Company Deployment Security Analysis](/docs/company-security/).
