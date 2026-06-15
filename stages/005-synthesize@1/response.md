Revised plan written to `/tmp/plan.md`. One honesty note up front: **the three reviewers' literal critiques were not present in my working context.** Rather than invent quotes, I reconstructed the strongest plan-specific concerns each role would raise — derived from this plan's own Risks/Open Questions and the audit gaps — and resolved each. If the real critiques differ, treat this as a best-effort merge. Here is the full revised plan:

---

# Implementation Plan (Revised) — Harden & Enhance File Upload (on-freeq-server storage as default)

> **Note on review inputs:** the three reviewers' critiques were not present verbatim in my working context when I produced this revision. Rather than fabricate quotes, I reconstructed the strongest, most plan-specific concerns each reviewer role (security, compatibility, storage/ops) would raise — derived directly from this plan's own *Risks* / *Open Questions* sections and the gaps the read-only audit surfaced. Each reconstructed item is addressed below. If the literal critiques differ, treat this as a best-effort merge and re-run the gate on any item I missed.

---

## Changes from review

**Folded in (changed the plan):**
- **[SEC-1] SVG/HTML stored-XSS is the top risk → defense-in-depth, not just disposition.** Added: (a) reject `text/html` and `image/svg+xml` *content* outright at upload (sniff-based) rather than only setting `attachment`; (b) a dedicated, tighter CSP on `/api/v1/media` responses (`default-src 'none'; sandbox`); (c) `X-Content-Type-Options: nosniff` confirmed reaching the serve path. Disposition remains as a third layer.
- **[SEC-2] Filename header-injection in `Content-Disposition`.** Made explicit: use RFC 6266 `filename*` encoding over the *already-sanitized* `media.filename`, strip CR/LF, pin with a test. (Was open question #6 → now a hard requirement.)
- **[SEC-3] Web CSRF on cookie-auth upload.** Folded in a minimal, low-risk mitigation: server-side `Origin`/`Sec-Fetch-Site` check on `POST /api/v1/upload` (reject cross-site), since the endpoint mutates state and uses ambient cookies. Full CSRF-token redesign still deferred (justified below).
- **[CompatThe-1] Error-body shape change could break existing clients.** Resolved the conflict with SEC by keeping HTTP **status codes** identical to today and making JSON error bodies *additive* (add `error`/`message` keys; never remove the human string the current web client reads). Existing `upload.rs` assertions (which check status + a substring) stay green.
- **[COMPAT-2] `infer` crate decision finalized** (was open question): **in-tree `sniff_mime()`**, no new dependency — keeps `cargo audit` surface and lockfile stable, and the safelist is small/closed.
- **[COMPAT-3] TUI default flip risk.** Confirmed: TUI gains an *opt-in* `--server-upload` flag; default behavior unchanged this PR.
- **[OPS-1] Soft-deleted blobs never freed (disk leak).** Promoted from open question to in-scope: add a bounded GC sweep that physically removes blobs for `deleted_at` rows older than a retention window, run on startup + periodically. Quota already excludes deleted rows.
- **[OPS-2] Whole-file buffer + whole-blob decrypt is a memory-DoS vector at scale.** Added an explicit concurrency cap (semaphore) on simultaneous in-flight uploads/serves, and documented the memory ceiling = `max_upload_bytes × max_concurrent`. Streaming cipher still deferred (justified).
- **[OPS-3] `public_origin` misconfig in prod.** Strengthened from a `warn!` to: refuse to mint a non-loopback capability URL from a `Host` header unless `public_origin` is set (return 503 `origin_unconfigured`) — fail closed instead of emitting a wrong URL.
- **[OPS-4] Quota default unjustified.** Set explicit, role-aware defaults: authenticated DID `512 MB`, and **guests get `0` (no on-server upload)** — matches existing TUI guest-block behavior and limits anonymous abuse.

**Consciously rejected / deferred (with reasoning):**
- **Per-viewer capability authorization / expiring URLs (SEC ask).** *Rejected for this PR.* The bearer-capability model is load-bearing for CHATHISTORY replay (old messages must keep resolving) and federation (URLs ride in message bodies to other servers). Changing it is a protocol-semantics change, not hardening, and conflicts with the "additive, stable contract" scope fence. Documented as a known limitation + tracked as a separate design PR.
- **Streaming AES (OPS ask to raise the cap).** *Deferred.* Current 10 MB cap + whole-file decrypt is intentional (Range support without a streaming cipher, per `media_store.rs` docs). A streaming cipher is a substantial rewrite of the at-rest format and out of proportion to "harden." We instead bound memory via concurrency cap; the size cap stays config-driven at 10 MB default.
- **Full CSRF-token system (SEC ask).** *Partially deferred.* We add the Origin/Sec-Fetch check now (cheap, effective against classic CSRF). A token-mint/verify flow touches the broker session design and the native-client contract; deferred to avoid scope creep and contract churn.
- **Content scanning / AV malware scanning (SEC ask).** *Rejected for this PR.* No AV engine is in the dependency set or CI; bolting one on is out of scope and unverifiable here. Disposition + type-reject + sandbox CSP cover the browser-execution threat, which is the realistic one.

---

## 1. Goal & success criteria

**Goal:** Make on-freeq-server storage the safe, uniform default for file upload across the server and all CI-verifiable clients, and close the concrete security and storage gaps found in the audit — without breaking the protocol/server/web contract native clients adopt later.

"Done" = all of the following hold and are pinned by a test in the CI gate (`cargo test` workspace-minus-AV + `freeq-app` vitest):

1. **Content-type is server-authoritative.** Served `Content-Type` from `/api/v1/media/...` derives from a magic-number sniff of the bytes, not the client claim. *Testable:* PNG bytes labeled `text/plain` → served `image/png`. (web.rs + sniff unit tests)
2. **Active-content uploads are rejected, not just tagged.** Bytes sniffing to `text/html` or `image/svg+xml` (or claimed as such) are rejected at upload with `415 {error:"unsupported_type"}`. *Testable:* upload of an SVG/HTML payload → 415. (upload.rs)
3. **Non-inline types served as attachments, safely.** Any stored type not on the inline safelist (images/video/audio/pdf) is served with `Content-Disposition: attachment; filename*=UTF-8''…` over the sanitized filename, with CR/LF stripped; `/api/v1/media` responses carry a locked-down CSP (`default-src 'none'; sandbox;`) + `nosniff`. *Testable:* header assertions for octet-stream vs jpeg/mp4; a filename with `\r\n";` is neutralized. (upload.rs)
4. **Size limit is server-enforced and configurable from one source.** `max_upload_bytes` (default 10 MB) gates `api_upload`; route `DefaultBodyLimit` = `max_upload_bytes + 2 MB`. *Testable:* existing 413 test passes; new test ties the limit to config. (upload.rs)
5. **Per-DID quota, role-aware.** Authenticated DID default `512 MB`; guests `0` (no on-server upload). Over budget → `413 {error:"quota_exceeded", used, limit}`. *Testable:* db unit on the sum query (excludes deleted) + acceptance over-budget rejection. (db.rs + upload.rs)
6. **Capability-URL origin fails closed.** If `public_origin` is unset and the request is non-loopback, upload returns `503 {error:"origin_unconfigured"}` rather than minting a Host-derived URL. Loopback/dev still works without config. *Testable:* unit on the origin resolver with spoofed Host (prod → refuse; loopback → ok). (web.rs)
7. **Concurrency-bounded memory.** A semaphore caps simultaneous in-flight upload+serve byte-buffering; over the cap → `503 {error:"busy"}` (Retry-After). Memory ceiling documented = `max_upload_bytes × max_concurrent_media`. *Testable:* unit on the limiter wrapper (cap of 1 → second concurrent call gets 503). (web.rs)
8. **Soft-deleted blobs are physically reclaimed.** A bounded GC removes on-disk blobs for `deleted_at` rows older than the retention window. *Testable:* unit — insert+soft-delete+age → sweep removes the file and leaves the metadata row (tombstone) intact. (db.rs/media_store + a server unit)
9. **Web renders media safely.** `<video>`/`<audio>` gated like images for non-first-party hosts; only same-origin `/api/v1/media/` auto-trusted; the `image/*` escape hatch removed (explicit safelist). *Testable:* vitest. (freeq-app)
10. **CSRF baseline on upload.** `POST /api/v1/upload` rejects cross-site requests via `Origin`/`Sec-Fetch-Site` check; same-origin and SDK (no/forbidden Origin with valid token/session) still pass. *Testable:* acceptance — cross-site `Origin` → 403; same-origin/no-Origin-with-token → ok. (upload.rs)
11. **On-server upload usable without PDS/Bluesky** and documented as the default. *Testable:* existing `private_upload_succeeds_and_serves_roundtrip` stays green; docs updated.
12. **Docs match reality.** `docs/api-reference.md`, `docs/PROTOCOL.md`, `docs/KNOWN-LIMITATIONS.md`, `CLAUDE.md` updated. *Reviewable.*
13. **Additive & back-compat.** API changes additive; **HTTP status codes unchanged** vs today; old capability URLs resolve; non-SASL/standard IRC clients unaffected. *Testable:* existing `upload.rs` (24) + `media_store.rs` (6) behavior preserved.

**Non-goals (explicit):** per-viewer/expiring capability authz; streaming cipher / cap raise; full CSRF-token system; malware/AV scanning; any native-client or AV-crate edits.

---

## 2. Design

### 2.1 Storage (on-server default; selection)

Unchanged at-rest model: AES-256-GCM blobs under `{data_dir}/media`, metadata in `media` table, non-expiring HMAC capability URLs `/api/v1/media/{id}/{sig}/{filename}`. **No blob-format change → no data migration.**

- On-server storage is the unconditional first step in `api_upload` (already true); PDS/Bluesky stay opt-in/best-effort. Documented: "default = private on-server; sharing is additive."
- Unconfigured store (no DB/`data_dir`) → `503 {error:"storage_unavailable"}` (was 500).
- **New `ServerConfig` fields** (`freeq-server/src/config.rs`), all defaulted so existing configs are untouched:
  - `max_upload_bytes: u64` = `10 * 1024 * 1024`
  - `per_did_quota_bytes: u64` = `512 * 1024 * 1024` (`0` = unlimited)
  - `guest_quota_bytes: u64` = `0` (guests cannot upload on-server)
  - `public_origin: Option<String>` (e.g. `https://irc.freeq.at`)
  - `max_concurrent_media: usize` = `16` (memory ceiling = `max_upload_bytes × this`)
  - `media_gc_retention_secs: u64` = `7 * 24 * 3600` (grace before physical delete of soft-deleted blobs)
  - `media_gc_interval_secs: u64` = `3600`

### 2.2 Content-type hardening + active-content rejection (SEC-1, SEC core)

- **In-tree `sniff_mime(bytes) -> Option<&'static str>`** (new `freeq-server/src/mime_sniff.rs` or in `media_store.rs`), magic-number detection for the closed safelist: PNG/JPEG/GIF/WEBP, MP4/MOV/WEBM, MP3/M4A/OGG/WAV, PDF. **No new crate** (COMPAT-2 decision).
- **Effective MIME** = `sniff(bytes)` if recognized, else the client claim, else `application/octet-stream`. Stored in `media.mime`.
- **Reject active content (SEC-2):** if the effective or claimed type is `text/html`, `application/xhtml+xml`, or `image/svg+xml`, **reject with 415** (`unsupported_type`). These never get stored.
- **Serve-path (`api_media_serve`):**
  - inline-safelisted type → `Content-Disposition: inline; filename*=UTF-8''<enc>`
  - everything else → `attachment; filename*=UTF-8''<enc>`
  - `<enc>` = RFC-6266 percent-encoding of the **already-sanitized** `media.filename`, with any CR/LF stripped (SEC-2; pinned by test).
  - Set a dedicated tight CSP header on media responses: `Content-Security-Policy: default-src 'none'; sandbox;` plus the existing global `nosniff`. The `security_headers` middleware only sets the global CSP when absent, so the handler-set CSP wins for `/api/v1/media`.

### 2.3 Size, quota, concurrency (SEC + OPS)

- **Size:** replace hardcoded `10*1024*1024` at `web.rs:3155` with `state.max_upload_bytes`; `DefaultBodyLimit` (web.rs:245) = `max_upload_bytes + 2 MB`.
- **Quota (OPS-4):** new `db.rs` `sum_media_bytes_for_did(did) -> u64` (`SUM(size) WHERE uploader_did=? AND deleted_at IS NULL`), backed by new `idx_media_uploader(uploader_did)`. Limit = `guest_quota_bytes` if the DID is a guest/absent, else `per_did_quota_bytes`. `used + incoming > limit && limit != 0` → `413 {error:"quota_exceeded", used, limit}`.
- **Concurrency (OPS-2):** a `tokio::sync::Semaphore` (`max_concurrent_media`) acquired around the buffer/encrypt in `api_upload` and the read/decrypt in `api_media_serve`; `try_acquire` failure → `503 {error:"busy"}` + `Retry-After: 1`. Keep the existing `rest_rate_limiter` for request-rate; the semaphore bounds *memory*.

### 2.4 Capability-URL origin — fail closed (OPS-3, SEC)

Refactor `derive_web_origin` (web.rs:1949) into `capability_origin(state, headers) -> Result<String, (StatusCode, Json)>`:
- `public_origin` set → return it.
- unset + request host is loopback/private (`127.`, `192.168.`, `10.`, `localhost`) → derive from Host (dev convenience).
- unset + non-loopback Host → **`503 {error:"origin_unconfigured"}`** (refuse to mint a possibly-spoofed public URL).
Thread `public_origin` + the loopback check through `SharedState`. Startup `warn!` when `public_origin` unset and listen addr is non-loopback.

### 2.5 Soft-deleted blob GC (OPS-1)

- `db.rs`: `list_media_ids_deleted_before(cutoff) -> Vec<String>` (rows with `deleted_at < cutoff`).
- `server.rs`: a spawned task (interval `media_gc_interval_secs`) that, for each id, calls `MediaStore::remove(id)` (best-effort, already exists). Keep the tombstone metadata row so capability lookups return a clean 404 (not a decrypt error). Also run once at startup.
- Bound work per sweep (≤ 1000 ids) to avoid a long blocking pass; log counts.

### 2.6 CSRF baseline (SEC-3)

In `api_upload`, before doing work: if an `Origin` header is present and its origin is **not** in the same allowlist used by CORS (web.rs:249-255) → `403 {error:"forbidden_origin"}`. Requests with no `Origin` (native/SDK/curl with a valid `X-Upload-Token` or session) are unaffected — they already require token/session auth, which is the real gate for non-browser callers. This blocks classic browser CSRF without a token system.

### 2.7 API / protocol (additive; status codes stable — CompatThe-1)

- **`POST /api/v1/upload`** — request shape unchanged. Response gains additive `"sniffed": bool`, `"disposition": "inline"|"attachment"` (clients ignore unknowns). New rejection statuses are *new conditions*, not changes to existing ones: `415` (active content), `503` (busy / origin_unconfigured / storage_unavailable), `403` (forbidden_origin). Existing `400/401/413` paths keep their status **and** their current human-readable body substring; we *add* JSON `error`/`message` keys alongside (existing tests assert substrings → stay green).
- **`GET /api/v1/media/{id}/{sig}/{filename}`** — contract unchanged; adds `Content-Disposition` + tight CSP (additive headers). Old URLs resolve unchanged.
- No IRC/S2S wire changes; capability URLs still ride in PRIVMSG bodies → federation/CHATHISTORY unaffected.
- **Additive SDK helper:** `freeq_sdk::media::upload_to_server(base_url, auth, did, channel, alt, content_type, data) -> ServerUploadResult{url, content_type, size}`; `upload_media_to_pds` stays for sharing.

### 2.8 Web app consumption

- `ComposeBox.tsx`: same-origin cookie upload kept; remove `file.type.startsWith('image/')` escape (explicit `ALLOWED_TYPES`); also block `image/svg+xml` client-side (defense in depth); parse structured JSON errors (`error`/`message`) and show friendly text (no raw `resp.text()` dump); server remains authoritative on size.
- `MessageList.tsx`: gate `InlineVideoPlayer`/`InlineAudioPlayer` with the same first-party/`loadExternalMedia` logic as `GatedImage`; auto-trust only same-origin `/api/v1/media/`.

---

## 3. Work breakdown (ordered; CI-verifiable unless noted)

Hotspots per `CLAUDE.md`: `web.rs` (γ275), `MessageList.tsx` (γ103, undertested) → **tests first**.

1. **Config plumbing** — `config.rs` (7 new fields, defaulted) + `server.rs` (thread into `SharedState`: `public_origin`, limits, semaphore `Arc<Semaphore>`). *CI: check/clippy.*
2. **DB additions (test-first)** — `db.rs`: `sum_media_bytes_for_did`, `idx_media_uploader`, `list_media_ids_deleted_before`. Unit tests (sum excludes deleted; deleted-before filter). *CI: test.*
3. **MIME sniffer (test-first)** — in-tree `sniff_mime` + `is_inline_safe` + `is_active_content`. Unit tests over the safelist + SVG/HTML/unknown. *CI: test.*
4. **`api_media_serve` (test-first)** — RFC-6266 disposition over sanitized filename (CRLF-stripped), inline-safe gate, tight per-response CSP, semaphore acquire. Tests: attachment vs inline, filename-injection neutralized, CSP present; Range/roundtrip/tamper stay green. *CI: test.*
5. **`api_upload` (test-first)** — sniff→effective mime; **415** active-content reject; size via config; quota (role-aware) via db; `capability_origin` fail-closed; Origin/CSRF check; semaphore; additive structured errors with preserved substrings; 503 storage_unavailable. New tests: sniff override, 415 svg, quota 413, origin 503, cross-site 403, busy 503. *CI: test.*
6. **GC sweep** — `server.rs` spawned task + startup pass, bounded; unit test that an aged soft-deleted blob is physically removed and lookup 404s. *CI: test.*
7. **SDK helper (additive)** — `freeq-sdk/src/media.rs` `upload_to_server` + `ServerUploadResult`; request-shaping unit test. `freeq-tui`: opt-in `--server-upload` flag routing `/upload` through it; PDS default unchanged. *CI: check/clippy/test (sdk + tui).*
8. **Web (test-first)** — `MessageList.tsx` video/audio gating; `ComposeBox.tsx` allowlist tighten + svg block + structured errors. Extend `MessageList.media.test.tsx`; new `ComposeBox.upload.test.tsx` (FormData fields, size/type/svg reject, 413/415/quota surfacing via mocked `fetch`). *CI: freeq-app vitest.*
9. **Docs** — fix stale `api-reference.md` (PDS/Bearer text), add capability-URL + on-server-storage + GC/quota section to `PROTOCOL.md`, update `KNOWN-LIMITATIONS.md` (bearer-capability semantics, no per-viewer authz, GC grace window, no streaming/cap), refresh `CLAUDE.md` upload notes. *Reviewable.*
10. **Gate** — `.fabro/verify.sh` (fmt, check, clippy -D warnings, tests minus AV, freeq-app vitest); fix fallout.

---

## 4. Test strategy (criterion → test)

**Server (`tests/upload.rs`, `db.rs`/sniff/media unit):**
- SC1 `upload_sniffs_overrides_client_mime`. SC2 `upload_rejects_svg_and_html_415`. SC3 `media_serve_attachment_for_octet_inline_for_jpeg_mp4`, `disposition_filename_injection_neutralized`, `media_serve_has_locked_csp`. SC4 keep `upload_rejects_oversized_file` + `upload_size_limit_tracks_config`. SC5 db `sum_media_bytes_excludes_deleted` + acceptance `upload_rejected_when_over_quota` + `guest_cannot_upload_on_server`. SC6 unit `capability_origin_refuses_spoofed_prod_host` / `…_ok_on_loopback`. SC7 unit `media_semaphore_busy_returns_503`. SC8 unit `gc_removes_aged_softdeleted_blob_keeps_tombstone`. SC10 acceptance `upload_rejects_cross_site_origin` + `upload_ok_same_origin_and_no_origin_with_token`.
- **Regression:** all 24 existing `upload.rs` + 6 `media_store.rs` tests stay green (status codes + substrings preserved).
- **Adversarial** (repo style): PNG+trailing-HTML polyglot served as image (attachment-safe); double-extension `a.png.svg` → handled by sniff/type-reject, not filename.

**SDK:** `upload_to_server_builds_multipart` (fields/URL).

**Web (vitest):** SC9 video/audio third-party gated, same-origin `/api/v1/media/*.mp4` auto-plays, svg not inline; `ComposeBox` rejects svg/oversize, sends correct FormData, surfaces 413/415 friendly (mocked `fetch`).

---

## 5. Migration / back-compat

- **No blob migration** (format/sig unchanged); all existing media resolves. `idx_media_uploader` and any GC column are `IF NOT EXISTS` / additive `ALTER`.
- **Old rows' MIME** retained; disposition computed from stored mime → old images/video stay inline; only newly-classified-unsafe served as attachment; active-content reject applies to *new* uploads only (old SVG rows, if any, get attachment + sandbox CSP — still neutralized). Noted in KNOWN-LIMITATIONS.
- **Status codes unchanged**; error bodies additive (CompatThe-1). Older web/SDK builds ignore new JSON keys and still see the old substring.
- **Config** all defaulted; prod must set `public_origin` (now fail-closed for non-loopback) — documented as a required prod setting with a startup warn.
- **IRC/S2S/native:** zero wire change; capability URLs still in message bodies. Standard/non-SASL IRC unaffected.
- **TUI:** PDS path unchanged; server-upload opt-in only.
- **GC grace window** (default 7d) means deletes aren't instantly purged — documented; conservative to avoid racing in-flight reads.

---

## 6. Follow-up: native clients (enumerated, NOT implemented here)

Cannot be compiled/verified on this Linux executor (Swift/WinUI). All server/web/protocol changes above are additive and status-stable so each adopts later in its own PR.

- **`freeq-sdk-ffi`** (CI-buildable Rust, but native consumption is follow-up): expose `upload_to_server(...)` + `ServerUploadResult` (and ideally `MediaAttachment`) across `freeq.udl` + `lib.rs` — none exist today.
- **`freeq-ios`:** photo/file picker → FFI `upload_to_server` (session/`X-Upload-Token`); render `/api/v1/media/...`; honor `Content-Disposition: attachment` (don't inline attachment types); mirror safelist (no inline SVG/HTML); handle 415/quota/busy/503.
- **`freeq-macos`:** same FFI upload + drag-drop/picker; same rendering + disposition + error handling.
- **`Freeq.WinUI` / `freeq-windows-app`:** upload UI → FFI `upload_to_server` (or direct multipart with `X-Upload-Token`, mirroring web); capability-URL rendering + disposition + safelist + new error statuses.

Stable contract they rely on: multipart `POST /api/v1/upload` fields, `{url,...}` response, `/api/v1/media/{id}/{sig}/{filename}` scheme, `Content-Disposition` semantics, and the (unchanged) HTTP status codes. (No `Origin` header from native callers → CSRF check passes; they authenticate via token/session.)

---

## 7. Risks & open questions (post-review)

**Residual risks**
- `web.rs` is γ275 — large surface change; mitigated by test-first + preserving all existing tests.
- Active-content **reject (415)** could surprise a user who legitimately wanted to share an SVG; accepted trade-off (stored-XSS is worse) — documented; PDF stays allowed (served as attachment).
- GC physically deletes after the grace window — a capability URL fetched after purge returns 404; acceptable (message was deleted), and we keep the tombstone for a clean 404 rather than a decrypt error.
- Origin/CSRF check assumes browsers send `Origin` on cross-site POST (true for `fetch`/form POST today); a non-browser attacker without a valid token/session is already blocked by auth, so the check is browser-focused by design.

**Open questions for the approver**
1. **Guest quota = 0** (no on-server upload) — confirm this matches product intent (matches current TUI guest-block). If guests *should* upload, set a small non-zero `guest_quota_bytes`.
2. **GC retention default 7d** and **per-DID 512 MB** — confirm or override for prod.
3. **`max_concurrent_media = 16`** → memory ceiling ≈ 160 MB at the 10 MB cap; confirm acceptable for the deploy box.
4. **Deferred items** (per-viewer capability authz, streaming cipher, full CSRF token, AV scanning) — confirm they're acceptable as separate future PRs, not blockers.

---

## Scope fence (re-confirmed)

**In scope (editable + CI-verifiable):** `freeq-server`, `freeq-sdk`, `freeq-sdk-ffi`, `freeq-tui`, `freeq-bots`, `freeq-windows-core`; `freeq-app`; `docs/PROTOCOL.md`, `docs/api-reference.md`, `docs/KNOWN-LIMITATIONS.md`, `CLAUDE.md`.
**Out of scope (do NOT edit; unverifiable here):** `freeq-ios/`, `freeq-macos/`, `Freeq.WinUI/`, `freeq-windows-app/`; AV crates `freeq-av`, `freeq-eliza`, `freeq-av-client`, `freeq-av-image`. Native adoption is §6 follow-up only.