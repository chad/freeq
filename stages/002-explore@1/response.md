I have everything I need. Global middleware applies `nosniff` + CSP to all responses (including `/api/v1/media`), and there's a global 12MB `DefaultBodyLimit`. CORS allows credentials from fixed origins. Now I'll write the findings brief.

The CORS `allow_credentials(true)` with `X-Upload-Token` not in `allow_headers` and the upload using cookie-implicit auth is notable. Let me compile the complete brief.

---

# Findings Brief: Harden & Enhance File Upload (on-freeq-server storage as default)

## 1. Current architecture (the surface)

freeq already has a **private-by-default, on-server media store**. The feature is largely about *hardening + completing* it, not greenfield.

**Server (Rust, `freeq-server/`)**
- `src/media_store.rs` (302 lines, 27 fns/tests) — disk-backed encrypted store. `MediaStore { dir, enc_key, cap_key }`. Keys derived from the server msg-signing seed via HMAC (`derive_enc_key` = `freeq-media-encryption-v1`, `derive_cap_key` = `freeq-media-cap-v1`). Bytes encrypted at rest with **AES-256-GCM** (`encrypt`/`decrypt`), sharded on disk by first 2 id chars, `0o700`/`0o600` perms (unix). Access is via **non-expiring HMAC capability URLs**: `/api/v1/media/{id}/{sig}/{filename}`, `sign`/`verify` (constant-time). `sanitize_filename` strips path traversal.
- `src/web.rs` (gamma 275 hotspot, ~4400 lines):
  - `api_upload` (3113–3392) — `POST /api/v1/upload`, multipart. Fields: `file, did, alt, channel, share_pds, share_bluesky|cross_post`. **Always** stores privately + mints a capability URL; PDS/Bluesky share is opt-in and best-effort. Auth: `X-Upload-Token` (HMAC, 5-min TTL in `state.upload_tokens`) **or** an active WebSocket session for that DID (`state.session_dids`). Size cap: **10 MB** in-handler (3155) + global `DefaultBodyLimit::max(12 MB)` (web.rs:245). Returns `{url, content_type, size, private}`.
  - `api_media_serve` (3791–3862) — `GET /api/v1/media/{id}/{sig}/{filename}`. Verifies capability sig **first** (403 on bad), then DB metadata lookup (non-deleted), then decrypts + serves with single-range support (`parse_single_range`), `Cache-Control: private, immutable`. Content-Type comes from stored `row.mime` (client-supplied at upload).
  - `api_blob_proxy` (3674) — `GET /api/v1/blob?url=` PDS/CDN proxy with strict host allowlist (SSRF guard), strips `Content-Disposition: attachment`, Range support.
  - `api_og_preview` (3670 region) — OG metadata proxy with SSRF checks.
  - `pick_media_filename` (3396) — extension by MIME for the URL tail.
  - `derive_web_origin` (1949) — **trusts the `Host` header** to build capability URL origin.
  - `security_headers` middleware (4180) — global `nosniff`, `X-Frame-Options: DENY`, HSTS, CSP (applies to `/api/v1/media` too). CORS (246–267): fixed origin allowlist, `allow_credentials(true)`, allowed headers = `content-type, authorization, X-Broker-Signature` (**not** `X-Upload-Token`).
- `src/db.rs` — `media` table (id, uploader_did, scope, mime, size, alt, filename, created_at, deleted_at), `idx_media_scope`. `insert_media` / `get_media` (filters deleted) / `soft_delete_media`. **No per-DID/quota query exists.**
- `src/server.rs` (1283–1301) — wires `MediaStore` only when a DB is configured; dir = `{data_dir}/media`, keys from `msg_signing_key`. `SharedState.media_store: Option<...>` (734).
- Tests: `tests/upload.rs` (24 tests) — auth (401/400), oversize (413), private roundtrip, tampered-sig 403, Range 206, step-up, wrong-DID, blob SSRF, OG SSRF, CSP. Plus 6 `media_store.rs` unit tests.

**SDK / clients**
- `freeq-sdk/src/media.rs` — `upload_media_to_pds` (PDS XRPC `uploadBlob` + `blue.irc.media` pin record + optional `app.bsky.feed.post`), `MediaAttachment` tags, `fetch_link_preview` (SSRF-guarded via `ssrf.rs`). This is the **PDS path** the server calls for opt-in share; **not** the on-server path.
- `freeq-tui` — uploads via the SDK **PDS path directly** (`/media|/img|/upload|/crosspost`, `main.rs:2351`), does *not* use `/api/v1/upload`. Renders inline images (feature-gated, 10 MB cap). Guest users blocked from upload (`main.rs:2362`).
- `freeq-sdk-js` — `sendMedia` emits media tags only; **no byte upload, no FormData**.
- `freeq-sdk-ffi` — **no media/upload bindings** (iOS/macOS/Android can't upload via FFI today).
- `freeq-app` (web, React) — `ComposeBox.tsx` `doUpload()`/`buildForm()` posts FormData to `/api/v1/upload` with **same-origin cookie auth only** (no `X-Upload-Token`, no CSRF token). Client caps 10 MB (`MAX_FILE_SIZE`) + MIME allowlist with an `image/*` escape hatch. `MessageList.tsx` renders media by **URL/extension regex**; `isTrustedImageUrl` auto-trusts same-origin `/api/v1/media/`. `<video>`/`<audio>` are **not** gated by `loadExternalMedia` (only images are). Capability URL is sent inline in the PRIVMSG body.

## 2. End-to-end paths

- **Web upload (the default):** ComposeBox → `POST /api/v1/upload` (cookie auth) → `api_upload` verifies session/token → `media_store.put` (AES-GCM) → `db.insert_media` → capability URL → URL sent as PRIVMSG text → recipients' `MessageList.tsx` regex-matches `/api/v1/media/...` → `GET api_media_serve` (sig verify → decrypt → serve, Range-capable).
- **Opt-in share:** same, plus `upload_media_to_pds` (best-effort, failure doesn't fail upload).
- **TUI:** disk file → SDK `upload_media_to_pds` → PDS/CDN URL → `send_media` tags (bypasses on-server store entirely).

## 3. Constraints

- **CI gate** (`.fabro/verify.sh` ≈ `ci.yml`): `cargo fmt --check`, `cargo check`, `clippy -D warnings` (warnings = errors), `cargo test` — all `--workspace` excluding AV crates (`freeq-av`, `freeq-av-client`, `freeq-av-image`, `freeq-eliza`). If `freeq-app/` touched → `npm ci` + `vitest run` (Node 20). `cargo audit` runs in CI.
- **Hotspots / philosophy** (`CLAUDE.md`): `web.rs` (gamma 275) and `MessageList.tsx` (gamma 103, **undertested**) are both in this surface — **write tests FIRST**. Philosophy: IRC-as-infrastructure, no centralization/UX-regression/protocol-breakage; "if it feels too clever, it's wrong." Backward-compat: non-SASL/standard IRC clients must keep working.
- **Protocol/docs are stale:** `docs/api-reference.md:91-99` still says upload goes "to the user's PDS" with `Bearer {web-token}` — contradicts the private-by-default reality. `docs/PROTOCOL.md` barely mentions media. No documented spec for capability URLs / on-server storage. Plan must update these.
- **Storage gating:** media store only exists when a DB + `data_dir` are configured (ephemeral servers return 500 on upload).

## 4. Threat model / failure modes

| Risk | Current state |
|---|---|
| **Stored XSS via SVG/HTML** | MIME is client-supplied and echoed back as `Content-Type`; no server-side sniff/validation. Global `nosniff` + CSP mitigate, but `/api/v1/media` sets **no `Content-Disposition`**, and web allowlist has an `image/*` escape (lets `image/svg+xml` through). **Highest-priority gap.** |
| **Size / DoS** | 10 MB in-handler + 12 MB global body limit. Whole file buffered in memory; whole blob decrypted in memory to serve (even for Range). **No per-DID quota, no rate-specific upload limit** (shares general `rest_rate_limiter`). |
| **Path traversal** | Mitigated: `sanitize_filename` + opaque random `id` is the actual disk key (filename is cosmetic URL tail). |
| **Capability-URL leakage** | URLs are non-expiring, unguessable (128-bit id + HMAC sig), but **bearer-style**: anyone with the URL gets the bytes; no per-viewer authz tie to channel membership. Leaked/forwarded URL = persistent access. No revocation beyond soft-delete. |
| **Content-type spoofing** | No validation that bytes match declared MIME; renderer trusts extension only. |
| **Auth/CSRF** | Web upload uses implicit same-origin cookies, no CSRF token; `X-Upload-Token` defined server-side but **not sent by web client**. CORS `allow_credentials(true)`. Need to confirm SameSite/origin enforcement. |
| **Host-header injection** | `derive_web_origin` trusts `Host`; a spoofed Host could mint capability URLs pointing at an attacker origin (matters if URLs are consumed out of band). |
| **Ungated media autoload (web)** | `<video>`/`<audio>` auto-attach `src` for any matched URL incl. third-party hosts → IP leak/bandwidth. |
| **Orphan/rollback** | Upload rolls back blob on DB insert failure (good); soft-delete leaves bytes on disk (no GC). |

## 5. Native clients (note only — not edited here)

- **freeq-ios / freeq-macos / Freeq.WinUI / freeq-windows-app**: consume `freeq-sdk-ffi`, which exposes **no upload/media API** today. To use on-server-default upload they'll eventually need: FFI bindings for an upload call (multipart `POST /api/v1/upload` with `X-Upload-Token`/session) + `MediaAttachment`/result types, and capability-URL rendering. This executor **cannot compile Swift/WinUI**, so these are out of scope for implementation/verification.

## 6. Open design questions the plan must answer

1. **Content-type safety:** sniff bytes server-side (e.g. `infer`/magic) and reject mismatches? Force `Content-Disposition: attachment` for non-inline-safe types, or serve a normalized/neutralized `Content-Type` (e.g. never echo `image/svg+xml` as inline)? Tighten the web `image/*` allowlist escape.
2. **On-server as the *default* across clients:** should `freeq-sdk`/TUI/JS gain a generic `upload_to_server` (multipart to `/api/v1/upload`) so the default no longer requires a PDS/Bluesky account (esp. guests, TUI guest-block)? How does auth work for non-WebSocket SDK callers (mint `X-Upload-Token` over IRC?)?
3. **Capability model:** keep bearer capability URLs (simple, replay-survivable for CHATHISTORY) or add per-viewer authorization / expiry / revocation? Trade-off vs. non-repudiation and history replay (URLs must keep resolving).
4. **Quota & abuse:** per-DID storage quota + dedicated upload rate limit + total-store cap? New DB queries needed (none exist).
5. **Streaming vs. whole-file:** keep whole-file decrypt-in-memory (current 10 MB cap rationale) or move to streaming cipher to raise limits? Any size-limit change?
6. **Host-header trust:** make capability-URL origin come from config, not the `Host` header.
7. **CSRF/auth for web:** add a CSRF token or strict origin check; decide whether web should also send `X-Upload-Token` (and add it to CORS `allow_headers`).
8. **Web rendering hardening:** gate `<video>`/`<audio>` like images; trust only same-origin `/api/v1/media/` for inline; normalize raw server-error text surfaced to users.
9. **Docs/protocol:** correct `api-reference.md`, add a capability-URL/on-server-storage spec section to `PROTOCOL.md`, update `KNOWN-LIMITATIONS.md`.
10. **Test-first targets (high-gamma):** new `web.rs` tests (content-type sniffing, disposition, quota, host-origin) and `MessageList.tsx` vitest (SVG/ungated media) before editing.

**Key files to touch (next step):** `freeq-server/src/web.rs` (`api_upload`, `api_media_serve`, `derive_web_origin`, security headers), `freeq-server/src/media_store.rs`, `freeq-server/src/db.rs` (quota queries), `freeq-server/tests/upload.rs`; `freeq-app/src/components/ComposeBox.tsx` + `MessageList.tsx` (+ tests); optionally `freeq-sdk/src/media.rs` for a server-upload path; docs `api-reference.md`, `PROTOCOL.md`, `KNOWN-LIMITATIONS.md`. Native FFI bindings are follow-on (not Linux-verifiable here).

This is a read-only investigation — no files were modified.