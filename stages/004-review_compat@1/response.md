Now I have a thorough picture of the code and the plan. Let me produce the critique.

---

## Compatibility Critique

### Issue 1 — `DefaultBodyLimit` and config coupling: existing tests will silently bypass the new limit

**Severity: Medium — breaks test isolation, could mask regressions**

The plan (§2.3, step 1/5) says `DefaultBodyLimit` will be derived from `max_upload_bytes + 2 MB overhead`. The actual code sets it at the **router build site** (`web.rs:245`) using a compile-time constant `12 * 1024 * 1024`. The existing test harness builds `ServerConfig { ..Default::default() }`, which will have `max_upload_bytes = 10 MB` (default). If the router is built before `SharedState` (which it is — the limit layer is applied at `make_router()` call time, before `.with_state()`), you cannot easily thread a runtime config value into a layer unless you change the function signature to accept the config. The plan acknowledges this but doesn't note that `start_server()` / `start_server_with_db()` in `upload.rs` also need to pass the config-derived limit, otherwise the quota test passes against a different cap than production. **Required plan change:** explicitly note that `make_router` signature must accept the config value (or the `SharedState` must be created first), and that all test helpers must pass the configured limit.

---

### Issue 2 — Error body type change: plain-`String` Err becomes JSON, breaks existing test assertions

**Severity: High — existing test suite breaks**

Today `api_upload` returns `Err((StatusCode, String))` (a bare string body — see `web.rs:3155–3158`, `3224–3227`). Several tests in `upload.rs` almost certainly assert on `resp.text()` content (e.g. `"File too large (max 10MB)"`, `"Upload requires an active connection"`). The plan changes the 413 body to structured JSON `{"error":"file_too_large","message":"..."}` and the 401 body to `{"error":"not_authenticated",...}`. The plan claims all existing 24 tests must stay green (SC9) but also changes the error body shape without specifying which tests need to be updated. These are in direct conflict. **Required plan change:** the plan must either (a) enumerate which existing test assertions on error text must be updated, or (b) keep the plain-string form for the pre-existing error paths and use JSON only for the two *new* error codes (`quota_exceeded`, `storage_unavailable`). As written, SC9 and the "structured error bodies" change cannot both be satisfied without touching existing tests.

---

### Issue 3 — `api_media_serve` response mutation silently breaks the 206 Partial Content path

**Severity: Medium — Range-request clients (native video players) affected**

`api_media_serve` builds two independent `HeaderMap` objects — one for 206 (line 3841–3851) and one for 200 (line 3854–3860). The plan adds `Content-Disposition` to the serve path (§2.2, step 4) but describes it generically. If the implementer only adds the header to the 200 branch, all Range-requesting clients (native iOS/macOS `AVPlayer`, video elements doing progressive download) get 206 responses **without** `Content-Disposition`. Attachment disposition would then be ignored by those players in exactly the case where it matters most. Both branches need the header. The plan's test says "attach for svg/html, inline for jpeg/mp4" but does not specify a Range-request variant of the attachment test. **Required plan change:** the test matrix for step 4 must include a Range sub-request and assert `Content-Disposition` is present on 206 responses.

---

### Issue 4 — Capability URL origin fix introduces a silent regression for existing production deployments without `public_origin`

**Severity: Medium — deployed servers, native clients, CHATHISTORY replay**

The plan (§2.4) says: if `public_origin` is unset, fall back to `Host`-header derivation **only for loopback/private hosts**. The existing `derive_web_origin` (lines 1949–1963) uses the `Host` header unconditionally for all hosts, including production non-loopback ones. Every existing production deployment that hasn't set `public_origin` will continue using the `Host` header — except now, if the plan's "loopback-only" restriction is enforced strictly, any server listening on a public IP without `public_origin` will get a fallback (presumably an error or some default), breaking capability URL minting. Old CHATHISTORY messages already in the DB reference URLs built with the old `Host`-derived origin — those keep working (capability URLs are signed, not origin-sensitive). But **new** uploads on a legacy deployment without `public_origin` could get broken URLs. The plan acknowledges a startup `warn!` but does not define what happens when `public_origin` is unset and the host is non-loopback: does it fall back to the `Host` header anyway (defeating the fix), or refuse to mint (breaking uploads)? This is under-specified and creates a flag-day for any operator upgrading without reading the docs. **Required plan change:** define the exact fallback behavior for a non-loopback server without `public_origin` (most defensible: use `Host` with a loud warning, never refuse upload). Also note that `freeq-ios`/`freeq-macos` may have cached capability URLs minted under the old host-derived origin — these remain valid (HMAC covers only the ID), so that's fine, but the plan should confirm this explicitly.

---

### Issue 5 — `freeq-sdk-ffi` UDL ABI: adding `upload_to_server` without UDL entry is dead code; with a UDL entry it is an ABI break

**Severity: Medium — iOS and macOS native apps**

The plan (§2.5) says it will add `upload_to_server(...)` + `ServerUploadResult` to `freeq-sdk`. The follow-up section (§6) says `freeq-sdk-ffi` would later expose this across the UDL boundary. **But the plan currently says it may expose it in FFI as "an optional step."** UniFFI is not optionally additive — adding a new `interface`, `dictionary`, or `function` to `freeq.udl` changes the generated Swift/Kotlin scaffolding, requiring the native app to rebuild against the new XCFramework. If the UDL is updated in this PR, the existing `freeq-ios`/`freeq-macos` builds linked against the old framework will fail to link or produce a runtime crash (UniFFI checks protocol conformance at construction). If UDL is **not** updated, native apps can't call the new function at all — fine for now, but the plan needs to say explicitly: **do not touch `freeq.udl` in this PR**. The plan is ambiguous ("could be done here as a separate optional step"). **Required plan change:** state clearly that `freeq.udl` and `freeq-sdk-ffi/src/lib.rs` are not modified in this PR. The Rust function in `freeq-sdk/src/media.rs` is Rust-internal only; UDL exposure is follow-up.

---

### Issue 6 — `ComposeBox.tsx`: the `!file.type.startsWith('image/')` escape hatch removal is a behavior break for existing drag-and-drop flows

**Severity: Low-Medium — web app regression**

Line 177 of `ComposeBox.tsx`:
```ts
if (!ALLOWED_TYPES.includes(file.type) && !file.type.startsWith('image/')) {
```
The plan says "remove the `image/*` escape hatch" (§2.6). That hatch currently lets through any browser-reported `image/*` type — including `image/avif`, `image/heic`, `image/tiff`, `image/bmp`, which some iOS Safari drag-and-drop or paste events produce. Removing it means those files are silently rejected with no way for the user to proceed, whereas today they upload and the server stores them (client-claimed MIME). The plan does not define what users see when the escape hatch is gone and an unusual-but-valid image type is pasted from clipboard. The compact safelist (`ALLOWED_TYPES`) will need `image/avif`, `image/heif`/`image/heic` at minimum for Safari/iOS WebKit compatibility. **Required plan change:** define the complete `ALLOWED_TYPES` list for the web app including the Safari-specific `image/heic`, `image/heif`, and `image/avif` before removing the escape hatch, or the change regresses file-paste on mobile Safari.

---

### Issue 7 — `InlineVideoPlayer` / `InlineAudioPlayer` gating plan ignores the video/audio URL-matching path, which is extension-based, not origin-based

**Severity: Medium — web app, privacy regression for third-party video/audio**

The plan (§2.6) says gate `InlineVideoPlayer`/`InlineAudioPlayer` behind `loadExternalMedia` / first-party logic "exactly like `GatedImage`." Looking at the real `MessageContent` (lines 421–444): the `videoMatch`/`audioMatch` check uses `VIDEO_URL_RE` (extension-based regex), which matches any URL ending in `.mp4`, `.webm`, etc. regardless of host. Today an external `https://evil.example/tracking.mp4` would be rendered as an auto-loading `<video>` element — a privacy/tracking leak. The existing `GatedImage` pattern fixes this for images but the plan's proposed fix for video/audio is correct in intent. However, the test file (`MessageList.media.test.tsx`) only tests same-origin `/api/v1/media/*.mp4` (which already works). **The plan provides zero test coverage for the actual fix: that a third-party `.mp4` URL is gated.** The existing 3-test file will pass unchanged even if the implementation does nothing to gate external video. **Required plan change:** add a test asserting that a non-origin `.mp4` URL (e.g. `https://cdn.example.com/video.mp4`) renders a gate/placeholder rather than a `<video>` element when `loadExternalMedia` is `false`.

---

### Issue 8 — Quota check races with concurrent uploads from the same DID

**Severity: Low-Medium — quota enforcement correctness**

Step 5 (§2.3) describes: read quota sum, check `used + incoming > limit`, then store. This is a TOCTOU race: two concurrent uploads from the same DID can both read the same `used` value, both pass the check, and both store, overfilling the quota. The existing `db.rs` mutex (`Mutex<Connection>`) serializes DB access within a single server, which would prevent the race at the DB layer — but only if the quota read and the insert happen in the same locked section. If `api_upload` reads the sum (locking DB), releases the lock, does auth checks, then re-acquires the lock to insert, the race window exists. The plan does not address this. **Required plan change:** specify that the quota read and insert must occur in the same `db.lock()` guard, or use a DB-level `INSERT OR FAIL` with a check constraint. Document the concurrency guarantee explicitly.

---

### Issue 9 — `TUI`: the `--server-upload` flag path still has no upload auth mechanism

**Severity: Medium — TUI cannot actually use the new endpoint**

The TUI has no concept of `X-Upload-Token` or an active WebSocket session for the server. The `api_upload` auth check (lines 3208–3227) requires either an upload token (minted by the broker, which the TUI does not use) **or** an active WebSocket IRC session with the DID registered in `session_dids`. The TUI uses a raw TCP connection, not WebSocket. Whether that connection's DID ends up in `session_dids` is something the plan does not verify — it needs to check whether TCP-connected sessions populate `session_dids` the same way WebSocket sessions do. If they don't, the TUI's `--server-upload` flag will always get 401. **Required plan change:** verify (with a test) that an authenticated TCP IRC connection populates `session_dids` such that a subsequent HTTP `POST /api/v1/upload` with the same DID is authorized. If it does, document it. If it doesn't, note this in the TUI step and the Known Limitations.

---

### Issue 10 — `Content-Disposition` header on `api_media_serve` is a behavior change for existing cached responses

**Severity: Low — but affects browser and native client caching**

Current serve path sets `Cache-Control: private, max-age=31536000, immutable`. Browsers and `AVPlayer` that have already cached a `/api/v1/media/...` response for a JPEG or MP4 will serve from cache without the new `Content-Disposition` header, which is fine. But for types that the plan will now classify as `attachment` (SVG, unknown), any blob already cached as `inline` in a browser will continue to render inline from cache until the cache expires (1 year). Since the URL is capability-URL-stable, there's no way to bust the cache. This is an acceptable trade-off for old blobs (as the plan notes in migration §5), but **new uploads should immediately get correct disposition**. The plan is silent on whether disposition is stored in the DB (it isn't — the plan adds logic to compute it at serve-time from `row.mime`). That's fine, but it means the `is_inline_safe()` function must be deterministic and match the sniff decision made at upload time. **Required plan change:** add a note that disposition is computed from stored `mime` at serve-time (not stored separately), and verify via test that a re-serve of an existing JPEG row returns `inline` even after the server upgrade.

---

### Scope fence assessment

The plan correctly excludes edits to `freeq-ios`, `freeq-macos`, `Freeq.WinUI`, and `freeq-windows-app`. The follow-up section (§6) accurately describes what those clients will need. The scope fence holds — **with one exception**: the plan's description of `freeq-sdk-ffi` as an "optional step" here is ambiguous and must be resolved to "not this PR" (see Issue 5 above).

---

### Summary table

| # | What breaks | Who is affected | Required fix |
|---|-------------|-----------------|--------------|
| 1 | `DefaultBodyLimit` can't be config-derived at current router build point | CI, prod | Document `make_router` signature change; update test helpers |
| 2 | Error body string → JSON breaks existing test assertions | CI (upload.rs) | Enumerate tests to update, or restrict JSON errors to new paths only |
| 3 | `Content-Disposition` missing from 206 Partial Content responses | iOS/macOS AVPlayer, `<video>` Range seeks | Add Range-variant test; set header in both 206 and 200 branches |
| 4 | `public_origin` restriction undefined for non-loopback without the field | All prod deployments upgrading without setting the flag | Define exact fallback (warn + use Host), never refuse upload |
| 5 | `freeq.udl` ambiguity: modifying it breaks native ABI; not modifying it means no FFI exposure | iOS, macOS | State explicitly: no UDL changes in this PR |
| 6 | Removing `image/*` escape hatch breaks `image/heic`, `image/avif` paste on Safari | Web app (iOS Safari users) | Define complete safelist including HEIC/AVIF before removing escape hatch |
| 7 | No test for third-party video/audio gating — the fix could be omitted and CI passes | Privacy (external `.mp4` loads) | Add test: external `.mp4` is gated when `loadExternalMedia=false` |
| 8 | Quota read–then-insert TOCTOU under concurrent uploads | Per-DID quota enforcement | Quota read + insert in same `db.lock()` guard |
| 9 | TUI `--server-upload`: no proof TCP sessions populate `session_dids` | TUI upload feature | Add test or explicit doc that TCP sessions are authorized |
| 10 | Old cached responses ignore new `attachment` disposition for 1 year | Existing cached SVG/unknown files | Acknowledge and note in KNOWN-LIMITATIONS |