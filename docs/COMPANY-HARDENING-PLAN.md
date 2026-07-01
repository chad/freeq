# Hardening Public freeq for Company Use — Remediation Plan

**Scope.** Make a **public, multi-tenant** freeq instance safe for a company to
run private channels on — i.e. their channel's existence, membership, history,
and files never leak to *other tenants* or *the internet*, and host exposure is
minimized. Message **content** is already handled by VC-bootstrapped E2E channels
(`+E` / EG1) + SSO gating (OIDC verifier); this plan covers the *other* gaps from
the internal security analysis.

**Inherent limit (be honest):** on a shared public host, the operator can always
see *metadata* (who's in a channel, when, channel names) unless that channel is
`+E` *and* the metadata store is encrypted (Phase 3) — and even then timing/size
side-channels remain. A company wanting zero host visibility should self-host.
This plan closes everything an *outsider or other tenant* can reach, and shrinks
what the host sees.

Ordered **easiest / least-intrusive first**. Effort S/M/L, Intrusiveness Low/Med/High.

---

## Phase 0 — Hygiene (minutes, zero protocol risk)

| # | Gap | Fix | Files | Effort | Intr. |
|---|---|---|---|---|---|
| 0.1 | Committed policy DB (C10) | `git rm --cached irc-policy.db`; add to `.gitignore`; scrub from image build | repo, `.gitignore`, `Dockerfile` | S | Low |
| 0.2 | Key file perms (C10) | Ensure `chmod 600` on all `*.secret` at startup (verify `secrets.rs` already does) | `secrets.rs` | S | Low |
| 0.3 | Plaintext IRC port (C8) | Default `--bind` (6667) to `127.0.0.1`; require explicit `--enable-plaintext-irc` to expose publicly; keep TLS `--tls-bind` public | `config.rs`, `main.rs` | S | Low |

Impact: removes a credential-leak vector and the one plaintext-on-the-wire path,
with no behavior change for TLS/web clients.

---

## Phase 1 — Stop private-channel enumeration (small, additive, HIGH value)

The single biggest cheap win for a multi-tenant host: **don't advertise private
channels.** Today `LIST` and the unauthenticated `GET /api/v1/channels` expose
every channel name + topic + count.

| # | Gap | Fix | Files | Effort | Intr. |
|---|---|---|---|---|---|
| 1.1 | LIST leaks private chans (C4) | In `handle_list`, skip channels that are `+i` or `+k` (and `+s` once added) unless the requester is a member | `connection/channel.rs:2033` | S | Low |
| 1.2 | REST `/channels` leaks private (C2/C4) | In `api_channels`, filter out `+i`/`+k` channels entirely (unauth endpoint → only public channels) | `web.rs:1205` | S | Low |
| 1.3 | REST read gate is TOCTOU (C3) | `api_channel_history` / `rest_readable_channel` / `api_search` load modes from DB and **fail closed** when the channel isn't resident in memory (today a not-loaded channel bypasses the `+i`/`+k` check) | `web.rs:1233,1299,1477` | S | Low |

Impact: a company's `+i`/`+k` channel becomes invisible and unreadable to
non-members via IRC LIST and REST, closing the easiest leakage paths. Additive
filters, minimal regression risk.

---

## Phase 2 — Real secret mode + member-scoped reads (moderate)

| # | Gap | Fix | Files | Effort | Intr. |
|---|---|---|---|---|---|
| 2.1 | No `+s` secret mode (C4) | Add `+s` channel mode: excluded from LIST, `/api/v1/channels`, and others' WHOIS channel lists; only members see it exists | `connection/channel.rs` (mode parse + list), `web.rs` | M | Med |
| 2.2 | Media URLs not member-scoped (C11) | `api_media_serve`: in addition to the capability sig, require an authenticated Bearer session whose DID is a member of the owning channel; short-TTL the cap sig | `web.rs` (`api_media_serve`), `media_store.rs` | M | Med |
| 2.3 | REST reads all-or-nothing (C2) | Give `history`/`search`/`export` a **member-scoped** path: authenticated Bearer session (reuse `session_dids`) + channel-membership check, replacing the binary public/403 gate. Default-deny | `web.rs` | M | Med |

Impact: private history, search, and uploads become readable only by
authenticated members over REST — matching the IRC `CHATHISTORY` guarantee.
Reuses the existing `session_dids` Bearer mechanism (already used by `/keys`,
`/groupkeys`).

---

## Phase 3 — Lifecycle & at-rest metadata (moderate–high)

| # | Gap | Fix | Files | Effort | Intr. |
|---|---|---|---|---|---|
| 3.1 | Offboarding doesn't cut sessions (C9) | On DID key rotation / handle change, invalidate live sessions: poll the PDS or subscribe to firehose; drop bound connections whose DID no longer verifies | `server.rs` (session mgmt), auth path | M | Med |
| 3.2 | Server-level connect gating (C1) | `--allowed-did-domains` / `--allowed-dids` + `--no-guest`, enforced at SASL success. (Less critical on a *public* instance — per-channel OIDC policy already gates company channels — but useful for a company sub-instance) | `config.rs`, `server.rs` | M | Med |
| 3.3 | Metadata + policy DB plaintext (C6) | Full-DB encryption at rest via SQLCipher for `irc.db` metadata tables + `irc-policy.db`; operator-held key | `db.rs`, `secrets.rs` | L | High |

Impact: offboarding actually revokes access; disk/backup theft no longer reveals
the social graph. 3.3 is the heaviest lift and mostly matters for host-disk
compromise (not other tenants).

---

## Phase 4 — Federation & compliance (highest, often optional for public)

| # | Gap | Fix | Files | Effort | Intr. |
|---|---|---|---|---|---|
| 4.1 | S2S peer-vouched identity (C7) | For the public instance, keep federation **off** (don't pass `--iroh`) — closes it entirely. If federation is needed, verify `+freeq.at/sig` across servers (make its canonical inputs survive S2S) + enforce mutual `--s2s-allowed-peers`/`--s2s-peers` | `s2s.rs`, `server.rs` | L | High |
| 4.2 | Retention / compliance (C12) | `--message-retention-days`; compliance export + hard-delete tooling | `db.rs`, `server.rs` | M | Med |

---

## Recommended execution order

1. **Phase 0 + Phase 1 first** — a day's work, low risk, and they close the
   outsider/other-tenant leakage that matters most on a public host.
2. **Phase 2** — the real member-scoped access control; the substantive win.
3. **Phase 3.1 (offboarding)** next — high operational value for companies.
4. **Phase 3.3 / Phase 4** — as needed by the company's compliance/threat bar;
   4.1 is a config default (federation off) unless federation is required.

Each phase is independently shippable and testable; nothing here blocks the
already-live E2E channel + SSO features.
