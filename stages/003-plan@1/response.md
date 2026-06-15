Plan written to `/tmp/plan.md`. Here it is:

---

# Implementation Plan — Harden & Enhance File Upload (on-freeq-server storage as default)

> Grounded in the read-only investigation of the current surface:
> `freeq-server/src/web.rs` (`api_upload`, `api_media_serve`, `derive_web_origin`, `security_headers`),
> `freeq-server/src/media_store.rs`, `freeq-server/src/db.rs` (`media` table), `freeq-server/tests/upload.rs`,
> `freeq-app/src/components/ComposeBox.tsx` + `MessageList.tsx`, `freeq-sdk/src/media.rs`, `freeq-tui`.
>
> Reality today: on-server, private-by-default, AES-256-GCM-at-rest media storage **already exists** and the web app already uses it. PDS/Bluesky sharing is opt-in and best-effort. This feature **hardens** that path, makes on-server storage the *uniform* default across CI-verifiable clients, and closes the security gaps the investigation surfaced. It is **not** a greenfield build.

---

## 1. Goal & success criteria

**Goal:** Make on-freeq-server storage the safe, uniform default for file upload across the server and all CI-verifiable clients, and close the concrete security gaps found in the audit — without breaking the protocol/server/web contract for native clients that adopt it later.

"Done" = all of the following are true and pinned by a test in the CI gate (`cargo test` workspace-minus-AV + `freeq-app` vitest):

1. **Content-type is server-authoritative.** The server sniffs uploaded bytes (magic-number), and the `Content-Type` it serves from `/api/v1/media/...` is derived from the sniff, not the client claim. *Testable:* upload bytes labeled `text/plain` that are actually PNG → served as `image/png`; upload an HTML/SVG payload → served with a neutralized type + `Content-Disposition: attachment` (never inline). (web.rs + media_store tests)
2. **No inline-executable media.** `/api/v1/media` responses for non-safelisted types carry `Content-Disposition: attachment; filename="…"`; `image/svg+xml` is never served `inline`. Global `nosniff` already present; we add disposition. *Testable:* serve-path test asserts the header for svg/html/octet-stream and its absence for jpeg/png/mp4. (upload.rs)
3. **Size limit is enforced server-side and consistently configurable.** A single source of truth (`max_upload_bytes`, default 10 MB) gates `api_upload`; the global `DefaultBodyLimit` is derived from it (+ small multipart overhead). *Testable:* existing 413 test still passes; a new test confirms the limit tracks config. (upload.rs)
4. **Per-DID storage quota.** Uploads beyond a per-DID byte budget (default e.g. 200 MB) are rejected with `413` + a structured `{error:"quota_exceeded"}`. *Testable:* db unit test on the new sum query + an acceptance test that the (N+1)th upload over budget is rejected. (db.rs + upload.rs)
5. **Capability-URL origin is not attacker-controllable.** Capability URLs are built from a configured public origin when set, falling back to the `Host` header only for loopback/dev. *Testable:* unit test on the origin-selection function with a spoofed `Host`. (web.rs)
6. **Web app renders media safely.** `<video>`/`<audio>` are gated behind `loadExternalMedia` exactly like images for non-first-party hosts; only same-origin `/api/v1/media/` is auto-trusted; the `image/*` allowlist escape hatch is removed (explicit safe list). *Testable:* vitest in `MessageList.media.test.tsx` + new `ComposeBox` validation test. (freeq-app vitest)
7. **On-server upload is usable without a PDS/Bluesky account** from the web app and is documented as the default. *Testable:* existing `private_upload_succeeds_and_serves_roundtrip` remains green; docs updated.
8. **Docs match reality.** `docs/api-reference.md`, `docs/PROTOCOL.md`, `docs/KNOWN-LIMITATIONS.md` describe the private-by-default on-server model and the capability-URL scheme. *Reviewable:* doc diff.
9. **Additive & back-compat.** All API/protocol changes are additive; old capability URLs keep resolving; non-SASL/standard IRC clients unaffected. *Testable:* full existing `upload.rs` + `media_store.rs` suites still pass unchanged in behavior.

Non-goals (explicitly out): changing the bearer-capability trust model to per-viewer authz (tracked as an open question, not built); streaming cipher / raising the size cap beyond config; touching native clients; touching AV crates.

---

## 2. Design

### 2.1 Storage (the on-server default and how it's selected)

Keep the existing model: bytes AES-256-GCM at rest under `{data_dir}/media`, metadata in the `media` DB table, served via non-expiring HMAC capability URLs `/api/v1/media/{id}/{sig}/{filename}`. No format change to stored blobs → **no data migration**.

- **Default selection:** on-server storage is unconditional and always happens first in `api_upload` (already true). PDS/Bluesky remain opt-in toggles (`share_pds`, `share_bluesky`) and best-effort. We make explicit in code + docs that "default = private on-server; sharing is additive."
- **Availability gating stays:** store requires DB + `data_dir`. We add a clearer 503 (instead of 500) when storage is unconfigured, with a structured body, so clients can message it.
- **New config** in `freeq-server/src/config.rs` (`ServerConfig`), all with defaults so existing configs are unaffected:
  - `max_upload_bytes: u64` (default `10 * 1024 * 1024`)
  - `per_did_quota_bytes: u64` (default `200 * 1024 * 1024`; `0` = unlimited)
  - `public_origin: Option<String>` (e.g. `https://irc.freeq.at`) for capability-URL origin.

### 2.2 Content-type hardening (the core security change)

- Add a dependency-light **magic-number sniffer**. Prefer the `infer` crate (pure-Rust, `cargo audit` clean) added to `freeq-server/Cargo.toml`. If adding a dep is undesirable at review, fall back to a small in-tree `sniff_mime()` covering the safelisted types (PNG/JPEG/GIF/WEBP/MP4/MOV/WEBM/MP3/M4A/OGG/WAV/PDF) by header bytes.
- **Resolve effective MIME at upload time:** `effective_mime = sniff(bytes).unwrap_or(client_claim)`; store `effective_mime` in `media.mime`. If the sniffed type is not in the inline safelist (images/video/audio/pdf), store it but mark it for `attachment` disposition.
- **Serve-path disposition:** in `api_media_serve`, compute `Content-Disposition`:
  - inline-safelisted type → `inline; filename="…"`
  - everything else (incl. `text/html`, `image/svg+xml`, `application/octet-stream`) → `attachment; filename="…"`
  - Never serve `image/svg+xml` as inline; downgrade to `attachment`.
- Global `nosniff` + CSP already applied by `security_headers` middleware — keep, and verify a test asserts they reach `/api/v1/media`.

### 2.3 Quota & limits

- **Size:** single `max_upload_bytes` config; `api_upload` checks the buffered `file` length against it (replace the hardcoded `10 * 1024 * 1024` at web.rs:3155). The route-level `DefaultBodyLimit` (web.rs:245) becomes `max_upload_bytes + 2 MB` overhead.
- **Quota:** new `db.rs` query `sum_media_bytes_for_did(did) -> u64` (sum of `size` where `uploader_did = ? AND deleted_at IS NULL`), backed by a new index `idx_media_uploader (uploader_did)`. `api_upload` rejects with `413 {error:"quota_exceeded", used, limit}` when `used + incoming > per_did_quota_bytes` (skip when limit `0`).
- **Rate:** keep the existing `rest_rate_limiter` (already applied to both upload and serve). No new limiter unless review wants one (open question).

### 2.4 Capability-URL origin

Refactor `derive_web_origin` (web.rs:1949): if `config.public_origin` is set, return it verbatim; otherwise keep the current `Host`-header derivation **only for loopback/private hosts** (dev). Production must set `public_origin`. This removes the Host-injection vector for minted capability URLs. `SharedState` gains a `public_origin: Option<String>` field threaded from config; `api_upload` passes it to a new `capability_origin(state, headers)` helper.

### 2.5 API / protocol changes (additive, versioned)

- **`POST /api/v1/upload`** — unchanged request shape (`file, did, alt, channel, share_pds, share_bluesky`). Response gains additive fields (clients ignore unknown keys): existing `{url, content_type, size, private}` + new `"sniffed": true|false`, `"disposition": "inline"|"attachment"`. Error bodies become structured JSON consistently (`{error, message, ...}`) for `413` (`file_too_large`/`quota_exceeded`), `503` (`storage_unavailable`), `401/403` (already structured for step-up).
- **`GET /api/v1/media/{id}/{sig}/{filename}`** — unchanged contract; adds `Content-Disposition` response header (additive). Old URLs keep working (sig scheme unchanged).
- No IRC/S2S wire changes. Capability URLs continue to ride in the PRIVMSG body, so federation/CHATHISTORY replay is unaffected.
- **SDK server-upload helper (additive):** add `freeq_sdk::media::upload_to_server(base_url, upload_token_or_session, did, channel, alt, content_type, data) -> ServerUploadResult{url, content_type, size}` that POSTs multipart to `/api/v1/upload`. This is **new and additive** — `upload_media_to_pds` stays for the share path. TUI can later switch its default to this (kept minimal here; see §3 step 7).

### 2.6 Web app consumption

- `ComposeBox.tsx`: keep same-origin cookie upload; remove the `file.type.startsWith('image/')` escape hatch (explicit `ALLOWED_TYPES` only); surface structured server errors (parse JSON `error`/`message`, don't dump raw `resp.text()`); keep the 10 MB client check but treat the server as authoritative.
- `MessageList.tsx`: gate `InlineVideoPlayer`/`InlineAudioPlayer` behind the same `loadExternalMedia`/first-party logic as `GatedImage` (auto-allow only same-origin `/api/v1/media/`); keep extension-based dispatch but rely on the server's now-correct `Content-Type` for first-party media.

---

## 3. Work breakdown (ordered; CI-verifiable unless noted)

Per `CLAUDE.md`, `web.rs` (gamma 275) and `MessageList.tsx` (gamma 103, undertested) are hotspots → **write tests first** for each.

1. **Config plumbing** — `freeq-server/src/config.rs`: add `max_upload_bytes`, `per_did_quota_bytes`, `public_origin` with defaults + parsing. `freeq-server/src/server.rs`: thread into `SharedState`. *CI: cargo check/clippy.*
2. **DB quota query (test-first)** — `freeq-server/src/db.rs`: add `sum_media_bytes_for_did` + `idx_media_uploader`. Unit test (insert several rows incl. a soft-deleted one; assert sum excludes deleted). *CI: cargo test.*
3. **MIME sniffer (test-first)** — add sniff helper (in `media_store.rs` or a small `mime_sniff.rs`); decide `infer` dep vs in-tree. Unit tests: PNG/JPEG/PDF/MP4 detected; SVG/HTML non-inline; unknown → fallback. *CI: cargo test, cargo audit (if dep added).*
4. **`api_media_serve` disposition (test-first)** — `freeq-server/src/web.rs`: compute & set `Content-Disposition` from stored mime + `is_inline_safe(mime)`. New tests in `tests/upload.rs`: attachment for svg/html, inline for jpeg/mp4; old Range/roundtrip/tamper tests stay green. *CI: cargo test.*
5. **`api_upload` hardening (test-first)** — `freeq-server/src/web.rs`: sniff → effective mime stored; size check via `max_upload_bytes`; quota check; structured error bodies; 503 on unconfigured store; `DefaultBodyLimit` from config. New tests: sniff-overrides-claim roundtrip, quota_exceeded 413, structured error shape. *CI: cargo test.*
6. **Capability origin** — `freeq-server/src/web.rs`: `capability_origin(...)` using `public_origin` else loopback-only Host fallback; unit test with spoofed Host. *CI: cargo test.*
7. **SDK server-upload helper (additive)** — `freeq-sdk/src/media.rs`: add `upload_to_server(...)` + `ServerUploadResult`; request-shaping unit test. Add a `--server-upload` flag in `freeq-tui` that routes `/upload` through the new helper, PDS path stays opt-in (guarded to avoid regressions). *CI: cargo check/clippy/test (freeq-sdk + freeq-tui).*
8. **Web app (test-first)** — `MessageList.tsx`: gate video/audio; `ComposeBox.tsx`: tighten allowlist + structured errors. Extend `MessageList.media.test.tsx` + new `ComposeBox.upload.test.tsx` (FormData fields, size/type rejection, 413/quota surfacing with mocked `fetch`). *CI: freeq-app vitest.*
9. **Docs** — fix stale `docs/api-reference.md`, add capability-URL + on-server-storage section to `docs/PROTOCOL.md`, update `docs/KNOWN-LIMITATIONS.md`, refresh upload notes in `CLAUDE.md`. *Reviewable.*
10. **Final gate** — run `.fabro/verify.sh` (fmt, check, clippy -D warnings, tests minus AV, freeq-app vitest); fix fallout.

---

## 4. Test strategy (how each success criterion is pinned)

**Server (`freeq-server/tests/upload.rs`, `db.rs`/`media_store.rs` unit tests):**
- SC1/SC2 — `media_serve_sets_attachment_for_svg_and_html`, `media_serve_inline_for_image_and_video`, `upload_sniffs_overrides_client_mime` (upload PNG bytes labeled text/plain, fetch back → `image/png`).
- SC3 — keep `upload_rejects_oversized_file` (413); add `upload_size_limit_tracks_config`.
- SC4 — db unit `sum_media_bytes_excludes_deleted`; acceptance `upload_rejected_when_over_quota`.
- SC5 — unit `capability_origin_ignores_spoofed_host_in_prod`.
- Regression — full existing 24 upload tests + 6 media_store tests stay green (tamper 403, Range 206, private roundtrip, step-up, SSRF, CSP).

**SDK (`freeq-sdk`):** `upload_to_server_builds_multipart` (fields file/did/channel/alt; correct URL).

**Web (`freeq-app` vitest):**
- SC6 — video/audio for third-party host shows gate; same-origin `/api/v1/media/*.mp4` auto-plays; svg URL not inline. `ComposeBox.upload.test.tsx`: rejects `image/svg+xml`/oversize client-side; sends correct FormData; surfaces `413 quota_exceeded` friendly (mocked `fetch`).

**Adversarial (matches repo `*_adversarial.rs` style):** polyglot (valid PNG header + trailing HTML); double-extension filename (`a.png.svg`) → disposition driven by sniff not name.

---

## 5. Migration / back-compat

- **Stored data:** blob format and capability-URL signing unchanged → **no migration**; existing media keeps resolving. New `idx_media_uploader` is `CREATE INDEX IF NOT EXISTS` (additive).
- **Existing rows' MIME:** old rows keep stored `mime`; disposition computed from it, so old image/video rows stay inline; only newly classified-unsafe types get `attachment`. No backfill (noted in KNOWN-LIMITATIONS).
- **API:** request shape unchanged; responses/headers only gain fields → older builds ignore unknowns. Error bodies become structured JSON but keep the same status codes the current tests assert.
- **Config:** all new config has defaults; no `public_origin` → current Host behavior for loopback (dev); prod should set it (documented + startup warn).
- **IRC/S2S/native clients:** zero wire changes; capability URLs still in message bodies. Standard/non-SASL IRC unaffected.
- **TUI:** PDS path preserved; server-upload behind a flag so default doesn't regress.

---

## 6. Follow-up: native clients (enumerated, NOT implemented here)

Cannot be compiled/verified on this Linux executor (Swift/WinUI). The server/web/protocol changes are additive so each adopts later in its own PR.

**`freeq-sdk-ffi`** (in-scope crate, but native consumption is follow-up): expose `upload_to_server(...)` + `ServerUploadResult` (and ideally `MediaAttachment`) across `freeq.udl` + `lib.rs` — currently no media/upload bindings exist.

**`freeq-ios`:** photo/file picker → FFI `upload_to_server` (multipart, session/`X-Upload-Token`); render `/api/v1/media/...`; honor `Content-Disposition: attachment`; mirror web safelist (no inline SVG/HTML).

**`freeq-macos`:** same FFI upload + drag-drop/picker; same capability-URL + disposition handling.

**`Freeq.WinUI` / `freeq-windows-app`:** upload UI → FFI `upload_to_server` (or direct multipart with `X-Upload-Token` mirroring web); capability-URL rendering + disposition + safelist.

Stable contract they rely on: multipart `POST /api/v1/upload` fields, the `{url,...}` response, the `/api/v1/media/{id}/{sig}/{filename}` scheme, and `Content-Disposition` semantics.

---

## 7. Risks & open questions

**Risks**
- **`web.rs` is a gamma-275 hotspot** — editing `api_upload`/`api_media_serve` risks regressions; mitigated by test-first and keeping the 24 existing tests green.
- **New dependency (`infer`)** — `cargo audit` surface + lockfile churn; mitigated by in-tree `sniff_mime()` fallback.
- **Disposition change** could flip a previously-inline type to `attachment`; mitigated — only non-safelisted types flip.
- **`public_origin` rollout** — if prod forgets to set it, fallback to loopback-only Host logic could mis-build on a public Host; mitigated by startup `warn!`.
- **TUI flag vs default** — flipping default could surprise users; gated behind a flag this PR.

**Open questions**
1. **Capability trust model:** keep non-expiring bearer URLs (replay-survivable for CHATHISTORY) or add per-viewer authz/expiry/revocation? Plan keeps bearer + documents the limitation (recommend separate PR).
2. **`infer` crate vs in-tree sniffer** — dependency policy preference.
3. **Default `per_did_quota_bytes`** (200 MB is a guess; guest quota?).
4. **Dedicated upload rate limit** now or deferred?
5. **Soft-deleted blob GC** — out of scope; sweeper to physically free bytes (and quota) for `deleted_at` rows? (Quota currently counts only non-deleted, so delete frees budget but leaves bytes on disk.)
6. **Disposition filename sanitization** — use the already-sanitized `media.filename` to avoid header injection; pin with a test.

---

A few honest caveats for the reviewer: the `infer`-vs-in-tree-sniffer and quota-default choices are deliberately left as decisions rather than assumed; the bearer-capability model is **not** changed here (it's the biggest latent design question and warrants its own PR); and the web app's cookie-based CSRF posture is acknowledged but I scoped CSRF hardening conservatively — flag if you want it pulled into this PR.