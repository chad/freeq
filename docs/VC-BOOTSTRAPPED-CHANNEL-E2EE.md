# VC-Bootstrapped End-to-End Encrypted Channels (no passphrase)

**Goal:** a company channel that is end-to-end encrypted — the freeq host never
sees plaintext — where **membership and key access are granted by an external
identity check** (Google Workspace / SAML / OIDC, or GitHub), *not* by a shared
passphrase. Offboarding someone in the IdP removes their ability to read new
messages.

This document explains why the naive approaches fail, specifies the protocol,
and maps each piece to code — including the two prototype components already
landed in this branch.

---

## Why not the existing schemes

freeq ships three encryption paths today (see `docs/ENCRYPTION.md`):

| Scheme | Where | Key source | Verdict for this goal |
|---|---|---|---|
| **ENC1** channel | `freeq-sdk-js/src/e2ee.ts:217` | `HKDF(passphrase, salt=channel)` | Works, but the passphrase is a **static shared secret** distributed by hand — no rotation, no revocation. |
| **ENC2** group | `freeq-sdk/src/e2ee_did.rs:77` | `HKDF(salt=SHA256(channel), ikm=sorted member DIDs)` | **Broken.** Every input is *public*. The server (and anyone who can list members) can recompute the key. Non-passphrase, but also non-secret. |
| **ENC3** DM | `freeq-sdk-js/src/e2ee.ts:494` | X3DH + Double Ratchet over published pre-keys | Genuinely E2EE and server-blind — but pairwise, not group. |

The insight: **ENC3's key-transport machinery is exactly what group channels
need.** Every user already publishes an authenticated X25519 pre-key bundle
(`POST /api/v1/keys`, owner-authenticated per CTF-19). We reuse that to deliver
a *real, random* group key to each member without the server ever seeing it.

### A credential is not a key

A `VerifiableCredential` (`freeq-server/src/policy/types.rs`) proves
*authorization* — "this DID is an ACME employee per Google." It is **not
secret**: the server issues it, stores it in `irc-policy.db`, and checks it. So
any key *derived from* a VC is knowable by the server (the ENC2 mistake). The VC
must therefore gate **who may receive** the key; the key itself must travel over
asymmetric crypto the server can't read.

> **Policy framework = admission control. Pre-key seal = key transport. They
> share one source of truth: the VC.**

---

## Roles

- **Member** — any user. Holds a long-lived X25519 identity key (already
  generated on login for DMs). Publishes the public half in their pre-key
  bundle.
- **Key steward** — one op client per channel, or a small company-run bot. Holds
  the current group secret and seals it to each admitted member. **No server
  custody of the key.** (A bot is convenient because it's always online to
  re-key on membership changes; an op's client works too.)
- **Verifier** — the SSO service (`/verify/oidc/*`) that turns a Google/SAML
  login into a signed VC. Can run on the freeq host or, for maximum host
  distrust, on company infrastructure (`freeq-server/src/bin/credential-issuer.rs`).

---

## End-to-end flow

```
                         ┌─────────────────────────────────────────┐
   (1) JOIN #eng         │  freeq server (relays blobs, sees no key)│
 user ───────────────────▶  policy: REQUIRE oidc_domain @acme.com   │
                         └───────────────┬─────────────────────────┘
   (2) gate → SSO                        │ 403 + verify URL
 user ──▶ /verify/oidc/start ──▶ Google login ──▶ /verify/oidc/callback
                         (3) checks hd=acme.com, signs VC, POSTs it back
                                          │
   (4) VC accepted → member added to channel; server broadcasts join
                                          │
   (5) steward sees join + valid VC, fetches member's pre-key bundle,
       seals current group secret to it:
 steward ──▶ TAGMSG #eng  @+freeq.at/groupkey  :EGK1:#eng:7:<eph>:<nonce>:<ct>
                                          │ (server relays, cannot open)
   (6) member opens EGK1 with its X25519 secret → holds epoch-7 group key
                                          │
   (7) traffic:  PRIVMSG #eng :EG1:7:<nonce>:<ct>   (server stores ciphertext)
```

On membership change (leave / kick / **VC expiry after offboarding**):

```
   steward.rotate() → epoch 8, fresh random secret
   re-seal EGK1 to REMAINING members only
   → departed member never gets epoch 8; new PRIVMSGs are EG1:8:… → unreadable
```

---

## Wire protocol

Two new message shapes. Both are opaque to the server.

### Channel message — `EG1`
Rides inside a normal `+encrypted` PRIVMSG (same envelope as ENC1/ENC2):
```
EG1:<epoch>:<nonce-b64url>:<ciphertext-b64url>
```
- `epoch` — which group secret encrypted this (lets a member detect it needs a
  newer sealed key).
- AES-256-GCM; message key = `HKDF(secret, salt=SHA256(channel), info="freeq-group-msg-v1-<epoch>")`.

### Sealed key-wrap — `EGK1` (control message)
Sent as a **TAGMSG** (or PRIVMSG) carrying a `+freeq.at/groupkey` client tag,
addressed to the channel; each member picks out the one sealed to them, or the
steward sends one targeted message per member:
```
EGK1:<channel>:<epoch>:<ephemeral-pub-b64url>:<nonce-b64url>:<ciphertext-b64url>
```
- Ephemeral-static ECIES: fresh X25519 ephemeral per seal, `shared =
  ECDH(ephemeral, member_pub)`, `wrap_key = HKDF(shared, salt=SHA256(channel),
  info="freeq-group-keywrap-v1-<epoch>")`, AES-256-GCM over the 32-byte secret.
- The ephemeral public key is in the clear (it's public); only the member's
  static secret rederives `wrap_key`. Binding to channel+epoch stops a sealed
  blob being replayed onto another channel/epoch.

Both formats, the seal/open, message encrypt/decrypt, and epoch rotation are
**implemented and unit-tested** in `freeq-sdk/src/e2ee_group.rs` (7 tests,
including "wrong member can't open" and "rotation revokes the departed member").

---

## SSO admission (Google / SAML / OIDC)

Implemented as a verifier in `freeq-server/src/verifiers/oidc.rs`, matching the
existing GitHub/Bluesky verifier pattern:

1. `GET /verify/oidc/start?subject_did=…&callback=…` → redirect to the IdP
   (`scope=openid email`, `hd=<domain>` hint for Google Workspace).
2. `GET /verify/oidc/callback` → exchange the code, read the ID token, require
   `email_verified` **and** `hd`/email-domain == the configured company domain.
3. Issue a signed `oidc_domain` credential: `{subject: userDID, claims:{email,
   domain}, exp: +12h}` and POST it to the join callback.

Config (env, read in `verifiers::router`):
```
OIDC_CLIENT_ID, OIDC_CLIENT_SECRET, OIDC_ALLOWED_DOMAIN=acme.com
OIDC_REDIRECT_URL=https://irc.acme.com/verify/oidc/callback
# OIDC_AUTH_URL / OIDC_TOKEN_URL default to Google; override for Okta/Entra/Auth0
```

Channel policy that uses it (existing `POLICY` command / policy framework):
```
POLICY #eng REQUIRE oidc_domain issuer=did:web:irc.acme.com:verify
```

The **short 12h TTL** is deliberate: re-auth through SSO re-checks Google
group/domain membership, so an offboarded employee's credential lapses within a
day and they are not re-sealed the next epoch. Tighten as needed.

> **Two jobs, one VC.** The `oidc_domain` credential gates JOIN (works today) and
> is the signal the steward checks before sealing the group key (new). SSO
> admission and E2E key access can never drift apart.

---

## What's built vs. what remains

**Landed in this branch (compiles; SDK tests green):**
- ✅ `freeq-sdk/src/e2ee_group.rs` — the full sender-keys crypto: random group
  secret, `seal_for`/`open` (ECIES key-wrap), `encrypt`/`decrypt` (`EG1`),
  `rotate` (epoch bump), `SealedGroupKey` wire codec. 7 passing unit tests.
- ✅ `freeq-server/src/verifiers/oidc.rs` + wiring in `verifiers/mod.rs` — the
  Google/SAML/OIDC → signed-VC verifier, config-gated by env.

**Remaining to ship the feature (ordered):**
1. **Steward orchestration** (SDK/bot): watch JOIN/PART, verify the member's VC,
   `seal_for` on join, `rotate` + re-seal on leave/kick/VC-expiry. New helper in
   `freeq-sdk` (or a `freeq-bots` steward bot).
2. **Server relay of `EGK1`**: allow the `+freeq.at/groupkey` TAGMSG through
   without interpreting it, and (optionally) persist the latest sealed key per
   member so a member reconnecting can re-fetch it (a new `GET
   /api/v1/channels/{name}/groupkey` returning *their* sealed blob — still
   server-blind).
3. **`+E` accepts `EG1`**: today the `+E` enforcement requires an `ENC1:` body
   (`freeq-server/src/connection/messaging.rs:709-711`). Extend the check to
   accept `EG1:` as well (both are opaque ciphertext). ~2 lines.
4. **Client wiring**: on receiving an `EGK1` for the current DID, `open` it and
   install the `GroupState`; replace the passphrase prompt
   (`setChannelKey(passphrase)` in `e2ee.ts`) with key-from-seal. Port
   `e2ee_group` to TS (or expose via the FFI already used for DM E2EE).
5. **Deprecate ENC2** (`e2ee_did.rs` `GroupKey`) — it advertises confidentiality
   it does not provide.

---

## Threat model

**Protects against:**
- **Host reading channel content** — the server only ever holds `EG1`/`EGK1`
  ciphertext and the (non-secret) VC. It cannot derive the group key.
- **Outsiders / non-employees** — no valid SSO VC → never admitted → never
  sealed to.
- **Ex-employees** — VC expiry + epoch rotation cut off future messages;
  no manual passphrase rotation, no "everyone learns the new pass" scramble.

**Does not protect against (be explicit):**
- **Content a member already saw** — inherent to any group scheme without
  per-message ratcheting. Rotation protects *future* epochs only. The MLS upgrade
  path (below) adds post-compromise security.
- **A malicious host substituting a pre-key bundle.** Uploads are owner-auth'd
  to a *session* DID (CTF-19), but the pre-key signing key isn't cryptographically
  bound to the member's AT Protocol DID document — so a hostile *server* could in
  principle offer a bundle it controls and MITM the seal. **Mitigation for the
  host-distrust model:** have the steward verify the member's pre-key is signed by
  a key in the member's DID document, or deliver the first seal alongside the VC
  from a company-run issuer. Required for Option-B (shared-host) deployments;
  moot when you self-host (you *are* the server).
- **Metadata** — membership, timing, channel names remain visible to the host
  (see `docs/COMPANY-DEPLOYMENT-SECURITY-ANALYSIS.md` C6).

---

## Upgrade path: MLS

`GroupState` here is "sender keys" — one shared secret per epoch, O(n) re-seals
on membership change. For large or high-assurance groups, migrate to **MLS
(RFC 9420)**: TreeKEM gives O(log n) rekey, forward secrecy, and post-compromise
security. The VC layer is unchanged — it just gates MLS `Add` proposals instead
of `seal_for` calls. This is the `docs/ENCRYPTION.md` Phase 3 target;
`e2ee_group` is the pragmatic step that ships now and proves the VC-gated,
server-blind key-distribution model.

---

## Try the crypto

```bash
cargo test -p freeq-sdk e2ee_group
# 7 passed — including rotation_revokes_the_departed_member,
# wrong_member_cannot_open_the_seal, secret_is_not_derivable_from_public_data
```
