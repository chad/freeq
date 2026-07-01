# freeq for Company Chat — Security Gap Analysis & Remediation Plan

**Question:** A company wants to move its internal chat to freeq. Two options:
- **Option A — Self-host** their own instance end-to-end.
- **Option B — Use a shared/hosted freeq** and rely on *encryption features + private channels + policies* so only company people can join and read.

This document assesses what is **not safe or good enough today** for each option, then gives a plan to close the gaps. Findings are grounded in the current code (paths cited), not the marketing docs.

---

## TL;DR / Recommendation

**Recommend Option A (self-host), with federation OFF and the whole surface behind the company network boundary.** Self-hosting removes the single biggest problem in Option B — *the host operator can read everything* — and turns the remaining issues into things the company controls.

**Option B is not adequately safe today** if the requirement is "the host cannot read our messages." The only thing that hides content from the host is `+E` channel encryption, which is a **static shared-passphrase** scheme with no forward secrecy, no key rotation, no per-member revocation, and it still **leaks all metadata** (who talked to whom, when, channel names, membership). That is defense-in-depth, not a trust boundary you'd bet a company on.

Even self-hosted, there are **four must-fix gaps before go-live** (all have deploy-time workarounds):
1. The server accepts **any** AT identity — there is no "only my company can connect" control.
2. REST read APIs are **unauthenticated**; privacy depends entirely on each channel being `+i`/`+k`, and **new channels aren't** (`+nt` default).
3. **Federation, if enabled, trusts peer-vouched identity** — a peer can inject messages spoofing your users' DIDs.
4. **Metadata and the policy DB are plaintext**; the plaintext IRC port and secrets handling need locking down.

---

## Threat models differ by option

| | **Option A: Self-host** | **Option B: Shared host + encryption** |
|---|---|---|
| Host operator can read plaintext | You *are* the operator — not a threat | **YES**, unless every channel uses `+E`; metadata always visible |
| External attacker on the internet | Yes — depends on your hardening | Yes — depends on freeq's hardening |
| Malicious federated peer | Only if you enable federation (don't) | Depends on host config (out of your control) |
| Data-at-rest / backup theft | Your responsibility | Host's responsibility; metadata + policy DB plaintext |
| "Only my company" enforcement | You can add allowlists / network boundary | Per-channel `+i`/`+k`/policy only — no org-level gate |

The rest of this doc is written primarily for **Option A**, with Option-B-specific caveats called out.

---

## Findings (grounded in code)

Severity: **Critical** = likely message/metadata exposure in a realistic config; **High** = exposure under common misconfig; **Medium** = weakens the model; **Low** = hygiene.

### C1 — No connect-time identity restriction *(Critical for "only my company")*
There is **no `--allowed-dids` / domain allowlist / SSO gate**. Anyone with any AT Protocol DID — or a guest — can connect to the server (`config.rs`/`main.rs` have no such flag; guest fallback at `server.rs:899,935`). "Only people in my company" is currently expressible **only** per-channel via `+i`/`+k`/policy, not at the server door. A stranger can connect, sit on the server, enumerate channels (see C4), and probe every unauthenticated REST path (C2).

### C2 — REST read endpoints are unauthenticated; privacy hinges on `+i`/`+k` *(Critical)*
`/api/v1/channels/{name}/history`, `/api/v1/search`, `/api/v1/channels/{name}/export`, and `/api/v1/messages/{msgid}` perform **no caller authentication**. The *only* gate is: if the channel is `+i` (invite-only) **or** `+k` (keyed), return `403` to everyone; otherwise serve full content to anyone on the internet (`web.rs:1233-1242`, `rest_readable_channel` at `web.rs:1299-1314`, search at `web.rs:1477-1510`).
- **New channels default to `+nt`**, i.e. *not* `+i`/`+k` → **world-readable via REST by default** until an op remembers to lock them.
- REST is all-or-nothing: there is no "member can read, non-member cannot." Members read via IRC `CHATHISTORY`; REST is either fully public or fully 403.

### C3 — REST gate depends on in-memory channel state *(High — verify)*
The `+i`/`+k` check reads the **in-memory** channel object: `if let Some(ch) = channels.get(...) && (ch.invite_only || ch.key.is_some())` (`web.rs:1236-1241`). If a channel's mode state isn't loaded in memory at request time, the gate does not fire and DB-backed history is still served. Needs a fail-closed rewrite that loads modes from the DB. (Flagged for verification — confirm channels are always resident with modes set.)

### C4 — No secret/private channel mode (`+s`/`+p`) *(High)*
Not implemented — channels **always appear in LIST** and via `/api/v1/channels` (`KNOWN-LIMITATIONS.md`). Even a properly `+i`/`+k`/`+E` channel leaks its **name, topic, and existence** to anyone connected or hitting REST. For a company, "Project-Falcon-acquisition" as a visible channel name is itself a leak.

### C5 — Channel encryption (`+E`) is a static shared passphrase *(Critical for Option B; High as defense-in-depth)*
`+E` is real and enforced: the server requires messages carry the `+encrypted` tag **and** an `ENC1:` ciphertext body, rejecting plaintext (`connection/messaging.rs:701-729`, CTF-21 hardening), and only stores/relays ciphertext — so with `+E`, the **server never sees plaintext**. Good. But the key management is weak (`KNOWN-LIMITATIONS.md` E2EE):
- **Static passphrase**, shared **out-of-band by hand**. No key-exchange protocol for channels.
- **No forward secrecy, no ratcheting, no key rotation.** One passphrase compromise (ever) decrypts **all past and future** messages.
- **No per-member revocation.** An offboarded employee who ever had the passphrase keeps the ability to decrypt everything, forever.
- (DMs are much stronger — X3DH + Double Ratchet, `ENC3:`, server sees ciphertext. The weakness is *channels*.)

### C6 — Metadata and policy DB are never encrypted *(High)*
At-rest encryption covers **message text only** (AES-256-GCM per row, `db-encryption-key.secret`). **Not** encrypted: senders, timestamps, msgids, membership, channel names, and the **entire policy DB** (`irc-policy.db`, which holds policies + issued credentials) — plaintext on disk (`ENCRYPTION.md` scorecard). Disk/backup theft, or the host operator in Option B, learns the full social graph and channel structure even when message bodies are `+E`-encrypted.

### C7 — Federation trusts peer-vouched identity *(Critical if federation enabled)*
Relayed S2S messages carry the sender's DID but the receiving server **does not verify it** — it trusts the origin peer, and `+freeq.at/sig` is **not verifiable across servers** (`KNOWN-LIMITATIONS.md` S2S). A malicious or compromised federated peer can **inject messages spoofing a company user's DID**. Additionally, `--s2s-allowed-peers` only checks *incoming* connections; mutual auth is an open item (`KNOWN-LIMITATIONS.md`; the server only *warns* if iroh is enabled without an allowlist, `server.rs:1586`). **Mitigation: do not enable federation for a private company instance.**

### C8 — Plaintext IRC port and transport exposure *(Medium)*
Port `6667` is plaintext TCP (`ENCRYPTION.md`). If bound to a public interface it exposes messages/registration on the wire. Only the TLS listener (`6697`) / `wss` should be reachable.

### C9 — Session lifecycle vs. offboarding *(Medium)*
DID key rotation/deactivation does **not** invalidate existing sessions; the server doesn't poll for key changes; handles aren't periodically re-verified (`KNOWN-LIMITATIONS.md` Auth). Deactivating an ex-employee's identity upstream does **not** immediately cut their live freeq session.

### C10 — Secrets & committed artifacts *(Medium)*
`*.secret` are gitignored (good) and keys auto-generate on first run. **But `irc-policy.db` is tracked in git** (`git ls-files` shows it) — it holds channel policies and issued credentials. The verifier signing key is stored as plaintext on disk. No HSM/keystore (`ENCRYPTION.md` Phase 5, still future).

### C11 — Media capability URLs are unguessable-but-public *(Medium)*
`/api/v1/media/{id}/{sig}/{filename}` are signed capability URLs; blobs are encrypted at rest but served **decrypted to anyone holding the URL** — no per-request membership check. A leaked/forwarded URL = a leaked file.

### C12 — Retention & compliance controls *(Low)*
Pruning is count-based only (`--max-messages-per-channel`); there is no retention-by-age or compliance deletion/export tooling (`KNOWN-LIMITATIONS.md`).

### C13 — Legacy-client server-side signing *(Low)*
Modern clients sign messages themselves (server can't forge). Legacy IRC clients fall back to server-attested signatures (server holds the key). Low risk if you mandate the official clients.

---

## Remediation plan

Ordered so a company can go live safely **before** any code lands (Phase 0), then close gaps properly.

### Phase 0 — Deploy-time hardening (no code; do before go-live)
These compensating controls neutralize the Critical/High findings for a self-hosted instance:
- **Self-host.** Operator = the company; C-level "host can read" (Option B) disappears.
- **Federation OFF.** Do **not** pass `--iroh`. (Closes C7.) If ever needed, set *both* `--s2s-peers` and `--s2s-allowed-peers` to the exact, mutually-trusted peer set.
- **Put the entire web/REST/WS surface behind the company boundary** — VPN, mTLS at nginx, or an IP allowlist. This is the key compensating control for unauthenticated REST (C2/C3/C11): if only employees can reach the server, "unauthenticated REST" stops being an internet-facing exposure. **Do this even though it's a band-aid.**
- **TLS only.** Bind `6667` to `127.0.0.1` (or don't expose it); publish only `6697`/`wss` via nginx. Firewall the rest. (C8)
- **Channel runbook:** every company channel gets `+i` **and** a channel key `+k`; use `+E` for channels that must be unreadable to disk/backup/host. Make an ops checklist so no channel ships as `+nt`. (Mitigates C2 for existing channels.)
- **Secrets:** `git rm --cached irc-policy.db` and add it to `.gitignore`; back up `db-encryption-key.secret` (losing it = unreadable history); `chmod 600` all `*.secret`; restrict `--oper-dids` to named admins with a strong `--oper-password`. (C10)

### Phase 1 — Org-level access control *(closes C1)*
Add a first-class allowlist enforced at SASL success / registration:
- `--allowed-dids <list>` and/or `--allowed-did-domains <handle-domains>` (e.g. only `*.yourco.com` handles), plus `--no-guest` to disable guest auth entirely.
- Reject connections whose verified DID isn't on the allowlist **before** binding identity. This makes "only my company can connect" a server control instead of a pile of per-channel band-aids.

### Phase 2 — Real REST authorization + private channels *(closes C2, C3, C4)*
- Require an authenticated, DID-scoped token/session on REST read endpoints and check **channel membership** — replace the all-or-nothing `+i`/`+k` gate with default-deny + member scoping. Load channel modes from the DB and **fail closed** when unknown (fixes C3).
- Implement `+s`/`+p` secret/private mode; exclude such channels from `LIST` and `/api/v1/channels`. (C4)
- Apply the same member-scoped check + short-TTL tokens to media cap URLs. (C11)

### Phase 3 — Strong channel encryption + metadata protection *(closes C5, C6)*
- Replace static-passphrase channel keys with **DID-based group key exchange** (the `ENCRYPTION.md` Phase 3 direction — MLS or Signal-style sender keys). Delivers forward secrecy, key rotation, and **per-member revocation** (offboarding actually removes access). This is the real fix that would make Option B trustworthy.
- **Encrypt the metadata + policy DBs at rest** (SQLCipher full-DB encryption), so disk/backup/host compromise doesn't reveal the social graph even when bodies are encrypted. (C6)

### Phase 4 — Lifecycle & compliance *(closes C9, C12, and finishes C10)*
- Invalidate sessions on DID key rotation/deactivation (poll PDS or webhook) so offboarding cuts access immediately. (C9)
- Retention-by-age + compliance export/deletion. (C12)
- Move TLS/signing/verifier/iroh keys into an encrypted keystore or HSM (`ENCRYPTION.md` Phase 5). (C10)

---

## If the company insists on Option B (shared host) today

It can be *acceptable for low-sensitivity chat* only if **all** of these hold, and the company accepts that **metadata is visible to the host regardless**:
1. **Every** channel is `+E` (content encrypted client-side; host sees only ciphertext).
2. The channel passphrase is generated well, shared strictly out-of-band, and **rotated on every membership change** (manually — there's no protocol for it, and old members retain old keys).
3. They accept that the host sees **all metadata**: membership, timing, channel names, who-DMs-whom.
4. They accept **no forward secrecy** — one passphrase leak, ever, exposes all history.
5. Sensitive 1:1s use **DMs** (X3DH + Double Ratchet), which are genuinely E2EE, not channels.

For anything the company would be unhappy to see in a subpoena to the host, Option B is not sufficient. **Self-host.**

---

## What's already solid (so this reads fairly)
- TLS 1.3 on production transports; SASL ATPROTO-CHALLENGE is sound (nonce, 60s window, single-use, keys never leave the client).
- DM E2EE (X3DH + Double Ratchet, `ENC3:`, SPK signatures verified, safety numbers) is genuinely strong.
- `+E` enforcement is hardened (requires tag **and** ciphertext body — CTF-21).
- S2S *authorization* (mode/kick/topic/ban/join) is verified server-side and rate-limited; the remaining S2S gap is *identity vouching* (C7), which disabling federation sidesteps entirely.
- Message text is AES-256-GCM at rest with a key independent of the signing key.
- Per-IP connection limits, command rate limiting, hostname cloaking.

*Analysis grounded in: `docs/ENCRYPTION.md`, `docs/KNOWN-LIMITATIONS.md`, `docs/POLICY.md`, `docs/self-hosting.md`, and code in `freeq-server/src/{web.rs,server.rs,config.rs,connection/messaging.rs,connection/channel.rs}`.*
