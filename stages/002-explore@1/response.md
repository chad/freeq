I have a complete, grounded picture. Here is the findings brief.

---

# Findings Brief: File Upload Surface in freeq

## 1. Current architecture (end-to-end)

freeq has a **private-by-default, on-server media store** already built. The flow:

1. **Client** builds `multipart/form-data` → `POST /api/v1/upload` (fields: `file`, `did`, `alt?`, `channel?`, `share_pds?`, `share_bluesky?/cross_post?`).
2. **Server** (`web.rs::api_upload`) authenticates the DID, **always** stores bytes **encrypted-at-rest on local disk** (AES-256-GCM), records metadata in SQLite, and mints a **signed capability URL**: `{origin}/api/v1/media/{id}/{sig}/{filename}`.
3. **Optionally** (only if `share_pds`/`share_bluesky`) the server *additionally* proxies the blob to the user's AT Protocol PDS via the SDK — best-effort, never fails the private upload.
4. **Server** returns `{ url, content_type, size, private }`. The client **string-concatenates the URL into a PRIVMSG body**.
5. On render, clients **regex-scan message text** to detect media URLs and render inline; `GET /api/v1/media/{id}/{sig}/{filename}` (`api_media_serve`) verifies the HMAC capability, decrypts, and serves with HTTP Range support.

**On-server storage is already the default** — the feature is hardening/enhancing this existing path, not building it.

## 2. Files & functions that matter

**Server (`freeq-server/src/`)**
- `media_store.rs` — the whole private store. `MediaStore::{new,sign,verify,capability_url,path_for,put,get,remove}`, `derive_enc_key`/`derive_cap_key` (HMAC domain-separated from `msg_signing_key`), `new_id` (128-bit), `sanitize_filename` (path-traversal guard), `encrypt`/`decrypt` (AES-256-GCM), `tighten_dir/file` (0700/0600). Fully unit-tested.
- `web.rs` (**gamma 275 hotspot**) — `api_upload` (`:3113`, multipart parse, 10MB cap at `:3155`, auth at `:3203`, PDS step-up at `:3235`), `pick_media_filename` (`:3396`), `api_media_serve` (`:3791`), `parse_single_range` (`:3867`), `api_blob_proxy` (`:3674`, SSRF allowlist), routes (`:196`/`:201`), `DefaultBodyLimit::max(12MB)` global layer (`:245`).
- `db.rs` — `media` table (`:430`, schema: id/uploader_did/scope/mime/size/alt/filename/created_at/deleted_at), `insert_media` (`:1086`), `get_media` (`:1116`), `soft_delete_media` (`:1141`).
- `server.rs` (**gamma 334 hotspot**) — `SharedState.media_store` (`:734`), `upload_tokens` (`:723`), store init gated on `db.is_some()` (`:1283`).

**SDK (`freeq-sdk/src/`)**
- `media.rs` — `upload_media_to_pds` (`:315`, PDS-only, not freeq-server), `MediaUploadResult`, `MediaAttachment` tag (de)serialization, `fetch_link_preview` (`:179`, SSRF-guarded, 64KB cap).
- `ssrf.rs` — `resolve_and_check`, `pinned_client` (DNS pinning vs rebinding), `is_private_ip/hostname`.
- **`freeq-sdk-ffi` has NO media/upload exports** — native FFI clients can't upload via the SDK; they call the REST endpoint directly.

**Web app (`freeq-app/src/`)**
- `components/ComposeBox.tsx` — `doUpload()` (`:193`, FormData build, 401 broker-refresh + 403 step-up retry), `handleFileSelect` (`:168`), `MAX_FILE_SIZE=10MB`/`ALLOWED_TYPES` (`:9-11`), paste/drag handlers.
- `components/FileDropOverlay.tsx` — window-level drop → CustomEvent.
- `components/MessageList.tsx` (**gamma 103, UNDERTESTED**) — `MessageContent` regex dispatch (`:368`), `isTrustedImageUrl` (`:218`, trusts same-origin `/api/v1/media/`), `GatedImage` (`:235`), detection regexes (`:56-71`).
- `store.ts` — only *viewing* state (`loadExternalMedia`, `lightboxUrl`); no upload state.

**TUI / native** — `freeq-tui` (`app.rs::MediaUploader`, PDS-cred based) and iOS/macOS/Windows/Android all build multipart by hand and POST to `/api/v1/upload` (confirmed: `MediaCapture.swift:472`, `UploadService.cs:17`).

## 3. Constraints the plan must respect

- **CI gate** (`.fabro/verify.sh` mirrors `ci.yml`): `cargo fmt --check`, `cargo check`, `clippy -D warnings` (warnings = errors), `cargo test` — all `--workspace` excluding AV crates (`freeq-av`, `freeq-eliza`, `freeq-av-client`, `freeq-av-image`). If `freeq-app/` changes, `vitest` must pass. **This executor cannot compile Swift/WinUI/Kotlin** — native clients are out of scope for edits.
- **Hotspots / "write tests FIRST"**: `web.rs` (275), `server.rs` (334), and `MessageList.tsx` (103, undertested) are the files this feature touches. CLAUDE.md philosophy: *"If something feels 'too clever,' it's probably wrong"* — favor minimal, auditable changes.
- **Capability-URL invariant**: URLs are **non-expiring** by design (CHATHISTORY replay must keep rendering). HMAC over `id` only; AES key + cap key both derived from the server's `msg_signing_key` seed.
- **10MB cap** is load-bearing: it's *why* the serve path can decrypt-in-memory and support Range without a streaming cipher (`media_store.rs:15`). Raising it has architectural ripple.
- **Existing tests**: `tests/upload.rs` (auth, oversize 413, private roundtrip, tampered-sig 403, range 206, step-up, blob-proxy SSRF, OG SSRF, CSP). `MessageList.media.test.tsx` (3 render cases). `oauth-step-up.test.ts`.

## 4. Threat model / failure modes (where hardening is needed)

| Risk | Current state | Gap |
|---|---|---|
| **Content-type spoofing** | Server trusts client-declared MIME; **no sniffing/whitelist**. `pick_media_filename` maps known MIMEs. | Server accepts *any* content-type; a `.html`/SVG/script could be stored and served with attacker-chosen type. No magic-byte validation. |
| **Stored-XSS via served blobs** | `api_media_serve` sets `Content-Type` from stored mime; **no `Content-Disposition`, no `X-Content-Type-Options: nosniff` on this route, no sandbox CSP.** | A blob served as `text/html`/SVG on the API origin could execute. Needs `nosniff` + `Content-Disposition: attachment` (or inline-only for known-safe types) + restrictive CSP on the media route. |
| **Size limit enforcement** | 10MB checked **after** `field.bytes()` reads the whole part into memory; global `DefaultBodyLimit` is 12MB. | Cap enforced post-buffering (memory pressure window). Limit + body-limit are slightly inconsistent (10 vs 12MB). No per-DID/quota limit. |
| **Path traversal** | `sanitize_filename` strips path components; id is opaque base64url; shard dir derived from id. | Tested and solid. `path_for` uses first 2 chars of id (could be `_`); URL `{filename}` is cosmetic only. Low risk. |
| **Capability-URL leakage** | Possession = access; non-expiring; HMAC unforgeable. URL only reaches conversation members *in theory* — but it's pasted into PRIVMSG text, so **anyone who can read the channel/history (incl. via REST/CHATHISTORY/federation) gets the URL forever.** | No per-recipient binding, no revocation beyond soft-delete, no scope check at serve time (the `scope` column is recorded but **not enforced** in `api_media_serve` — any valid sig serves regardless of channel membership). |
| **Quota / DoS** | Only IP rate-limit on upload + 10MB cap. No per-DID storage quota, no total-disk cap, no cleanup/GC. | Unbounded disk growth; soft-deleted blobs stay on disk (`soft_delete_media` flips a flag but `media_store::remove` isn't wired to it). |
| **Auth** | Upload requires active WS session or `X-Upload-Token` (HMAC, <300s, DID-bound). | Solid. But `did` is a form field cross-checked against session — fine. |
| **SSRF** | `api_blob_proxy` uses host **allowlist** (not DNS-pinned, unlike SDK's `resolve_and_check`). | Blob proxy is allowlist-only; acceptable but inconsistent with the pinned-client pattern elsewhere. |
| **Orphan/rollback** | Upload rolls back blob if DB insert fails. | Good. But no reconciliation job for blobs whose messages were deleted, or DB rows whose blobs vanished. |

## 5. Native clients (eventual needs — NOT edited here)

All four call `POST /api/v1/upload` with hand-rolled multipart and expect `{url, content_type, size}`:
- **iOS** (`MediaCapture.swift`, `ComposeView.swift`, `PhotoPicker.swift`), **macOS** (`FileUpload.swift`), **Windows** (`UploadService.cs`), **Android** (`PhotoUpload.kt`).

They will *eventually* need to consume any new response fields (e.g. structured attachment metadata, a `nosniff`/disposition contract, server-validated content-type, or an upload-token refresh). **Any change to the upload request/response contract or capability-URL format must stay backward-compatible** or be versioned, since these clients can't be recompiled in this environment.

## 6. Open design questions the plan must answer

1. **Content-type hardening**: Add server-side magic-byte sniffing + an allowlist (mirror the JS `ALLOWED_TYPES`)? Reject or coerce mismatched types? This is the single biggest security gap.
2. **Serve-path safety headers**: Add `X-Content-Type-Options: nosniff`, `Content-Disposition` (attachment vs inline-by-type), and a tight CSP/`Content-Security-Policy: sandbox` on `/api/v1/media/*`? (Affects how inline rendering works in all clients.)
3. **Scope enforcement at serve time**: Should `api_media_serve` verify the requester is a member of `row.scope` (channel/DM), upgrading from pure capability-URL to capability+authz? This changes the "possession = access" + non-expiring invariant and breaks unauthenticated `<img>` loads — needs care.
4. **Structured attachment metadata**: Move from URL-in-PRIVMSG-text + regex detection to an IRCv3 tag (mime/size/alt) carried to the renderer (`MediaAttachment` already exists in the SDK but is unused by web)? Reduces render-side ambiguity and the gating asymmetry (video/audio aren't gated, only `<img>`).
5. **Quota & lifecycle**: Per-DID/total storage quota, wiring `soft_delete_media` → physical `remove`, and a GC/reconciliation job. What limits and retention?
6. **Size cap**: Keep 10MB (and align the 12MB body-limit to it), or raise it (requires streaming cipher rework)? Enforce *before* full buffering via `Content-Length` precheck.
7. **Test-first targets**: New `web.rs`/`media_store.rs` tests for sniffing, headers, scope; the undertested `MessageList.tsx` and untested `doUpload()`/no-e2e-upload gaps in `freeq-app`.

This brief is the foundation for the implementation plan. No files were edited.