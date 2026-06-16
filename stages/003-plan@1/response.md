Plan written to `/tmp/plan.md`. Here it is in full:

---

# Implementation Plan: Harden & Enhance File Upload (on-server storage default)

**Status:** proposed · **Scope:** `freeq-server`, `freeq-sdk`, `freeq-app`, `freeq-tui`, `docs/`
**Out of scope (not edited here):** `freeq-ios`, `freeq-macos`, `Freeq.WinUI`, `freeq-windows-app`, AV crates.
**Guiding principle (CLAUDE.md):** *"If something feels 'too clever,' it's probably wrong."* Keep changes minimal, auditable, additive.

---

## 0. Grounding facts (from exploration — so the plan is honest)

The private on-server store **already exists and is the default**:

- Upload: `POST /api/v1/upload` → `web.rs::api_upload` (`:3113`). Always stores privately via `media_store::MediaStore`, mints a signed capability URL, optionally also pushes to PDS.
- Store: `media_store.rs` — AES-256-GCM at rest, HMAC capability URLs over `id`, `sanitize_filename` path-traversal guard, 0700/0600 perms.
- Serve: `GET /api/v1/media/{id}/{sig}/{filename}` → `web.rs::api_media_serve` (`:3791`). HMAC-verifies, decrypts, Range support.
- DB: `media` table (`db.rs:430`) with `insert_media`/`get_media`/`soft_delete_media`. **`scope` is recorded but not enforced at serve time. `soft_delete_media` is never called and is not wired to physical blob removal.**
- Global middleware `security_headers` (`web.rs:4180`) **already sets `X-Content-Type-Options: nosniff` on every response, including the media route.** So nosniff is NOT a gap; `Content-Disposition` and content-type *trust* are.
- **No server-side content-type validation / magic-byte sniffing.** Server trusts the client-declared multipart `Content-Type`; web client whitelists (`ComposeBox.tsx:11`) but TUI/native do not.
- Size cap: `10MB` checked **after** the part is fully buffered (`web.rs:3155`); global `DefaultBodyLimit::max(12MB)` (`web.rs:245`) is the only pre-buffer guard. The two are inconsistent (10 vs 12).
- No per-DID quota, no GC, no disk cap. No `infer`/`tree_magic`/`mime` crate in `freeq-server/Cargo.toml` today.
- Web app puts the capability URL **into PRIVMSG text**; `MessageList.tsx` re-detects it by regex. Only `<img>` honors the `loadExternalMedia` privacy gate; video/audio do not. `MessageList.tsx` (gamma 103) and `doUpload()` are undertested.

This feature therefore **hardens** (content-type validation, disposition, size enforcement, optional scope authz, quota/GC) and **enhances** (explicit on-server-default contract, structured response fields the web app and later native clients can use) — without rebuilding the store.

---

## 1. Goal & success criteria

**Goal:** Make on-server storage the explicit, hardened default for uploads, close the content-type / disposition / quota gaps, and keep the upload + capability-URL contract stable and additive so iOS/macOS/Windows can adopt later.

**Done means (each is a test):**

1. **SC1 — Content-type is server-validated.** Upload of bytes whose magic bytes don't match an allowlisted media type is rejected `415`. The *stored & served* `Content-Type` is the server-derived type, never the raw client claim.
2. **SC2 — Served blobs cannot be treated as active content.** `api_media_serve` responses carry `X-Content-Type-Options: nosniff` (already global — assert it stays) **and** `Content-Disposition: inline; filename="..."` for known-renderable types, `attachment` for everything else, plus a per-route `Content-Security-Policy: default-src 'none'; sandbox`.
3. **SC3 — Size cap is enforced before full buffering and is consistent.** A request whose declared `Content-Length` exceeds the cap is rejected `413` without buffering the whole body; the global body limit equals the upload cap + small multipart overhead. The cap is a single named constant.
4. **SC4 — On-server storage is the documented, selected default.** `share_pds=false`/absent → bytes never leave the server; response includes `"storage":"server"` and `"private":true`. `docs/PROTOCOL.md` documents the upload contract + capability-URL semantics.
5. **SC5 — Quota & lifecycle.** Per-DID stored-bytes quota enforced at upload; a `soft_delete_media` path is wired so deleted media stops serving (`404`) and the blob is physically removed.
6. **SC6 — Web app sends correct metadata and renders safely.** `doUpload()` posts `filename` + `alt`; uses the structured response (`url`,`content_type`,`size`,`storage`); video/audio honor the external-media gate symmetrically with images.
7. **SC7 — Back-compat.** Existing stored media still serves. Older clients (no new fields) still upload and get a working capability URL.
8. **SC8 — CI green.** `.fabro/verify.sh` passes: `cargo fmt`, `cargo check`, `clippy -D warnings`, `cargo test --workspace` (CI exclusions), and `freeq-app` vitest.

**Explicit non-goals:** raising the 10MB cap; replacing capability URLs with per-recipient tokens; streaming cipher; editing native clients.

---

## 2. Design

### 2.1 Data model

`media` table — **additive columns only** (new migrations in `db.rs`, same pattern as existing `ALTER TABLE ... ADD COLUMN`):

- `sha256 TEXT` — hex digest of plaintext bytes (dedupe + integrity; nullable for old rows).
- `validated_type TEXT` — the server-derived content-type actually served (nullable; old rows fall back to `mime`).

No column is dropped or made non-null. `get_media` returns the new fields as `Option`. A new `db.rs::sum_media_bytes_for_did(did) -> u64` backs the quota check.

### 2.2 Content-type validation (SC1)

- New `media_store.rs::validate_content_type(declared, bytes) -> Result<&'static str, RejectReason>`: sniff bytes → canonical type; allowlist = a unified server-side `ALLOWED_MEDIA_TYPES` (the same set the web client uses); non-allowlisted/unknown → `415`. The **served** type is the sniffed type, not the declared one. `text/plain` allowed but always served `attachment`.
- Sniffer: prefer a small in-crate `sniff_media_type(&[u8])` matching the ~12 magic signatures already enumerated in `pick_media_filename` (avoids a new dep on the gamma-334 server). `infer` crate is the alternative (see Q1).
- `api_upload` calls this after reading bytes and **before** `store.put`; reject stores nothing.

### 2.3 Serve-path safety headers (SC2)

In `api_media_serve`: serve `validated_type` (fallback `mime`); assert global `nosniff` stays; add `Content-Disposition` (`inline` for image/video/audio, `attachment` for pdf/text/other) and a per-route `Content-Security-Policy: default-src 'none'; sandbox`. Same headers on the Range path.

### 2.4 Size enforcement (SC3)

- `pub const MAX_UPLOAD_BYTES: usize = 10 * 1024 * 1024;` in `media_store.rs`, reused everywhere.
- `DefaultBodyLimit::max(MAX_UPLOAD_BYTES + 256*1024)` instead of literal 12MB.
- `Content-Length` precheck in `api_upload` → early `413`; keep authoritative post-decode check.

### 2.5 On-server-default contract + response shape (SC4)

Response gains additive fields: `"storage": "server"` (or `"server+pds"`), `"sha256"`. Request unchanged — **no new required field** (SC7).

### 2.6 Auth / capability (unchanged invariant + optional opt-in authz)

Capability URLs stay **non-expiring**, HMAC-over-`id`, format unchanged. **Scope enforcement is designed but ships OFF** behind `media_require_membership: bool` (default false): when on, `api_media_serve` checks the requester's session DID against `row.scope`. Default-off preserves "possession = access" and unauthenticated `<img>` loads (and protects un-editable native clients). See Q2.

### 2.7 Quota & lifecycle (SC5)

`media_max_bytes_per_did` config (default 256MB; `0` = unlimited). `api_upload` checks `sum_media_bytes_for_did` → `413` on exceed. Add `delete_media(id, requester_did)` (authorize uploader/op → `soft_delete_media` → `store.remove`); `get_media` already filters `deleted_at IS NULL` so serving returns `404` after delete.

### 2.8 Web app consumption (SC6)

`ComposeBox.tsx`: consume `result.storage`/`content_type`; request stays additive. `MessageList.tsx`: generalize the `loadExternalMedia` gate to video/audio for external URLs (`isTrustedMediaUrl`/`GatedMedia`); first-party `/api/v1/media/` stays ungated. No new store state.

---

## 3. Work breakdown (ordered; all CI-verified unless noted)

> Hotspot rule: `web.rs` (275), `server.rs` (334), `MessageList.tsx` (103) — **write the test first**.

- **S1. Constants + size enforcement** — `media_store.rs` constants; `web.rs` `:3155`/`:245` use the constant + `Content-Length` precheck. Test: extend `upload_rejects_oversized_file`.
- **S2. Content-type validation** — `media_store.rs` `sniff`/`validate` (+unit tests); `api_upload` calls before `put`. Test: spoof→415, served type==sniffed.
- **S3. DB additive columns** — `db.rs` `ADD COLUMN sha256/validated_type`, extend `insert_media`/`MediaRow`/`get_media`, add `sum_media_bytes_for_did`. Test: insert/get/sum.
- **S4. Serve-path headers** — `api_media_serve` disposition + per-route CSP + validated type (Range + full). Test: header assertions.
- **S5. Quota** — `config.rs` field; `api_upload` check. Test: over-quota→413.
- **S6. Deletion wiring** — `delete_media` helper. Test: post-delete 404 + blob gone.
- **S7. Optional scope-authz (dark, default-off)** — `config.rs` flag; `api_media_serve` membership check. Test: on (member 200 / non-member 403), off (unchanged).
- **S8. Response fields** — `api_upload` adds `storage`+`sha256`. Test: assert `storage=="server"`.
- **S9. Web app** — `ComposeBox.tsx` consume fields; `MessageList.tsx` gate video/audio. Tests: new `ComposeBox.test.tsx` for `doUpload()`; extend `MessageList.media.test.tsx`.
- **S10. SDK (optional)** — share `ALLOWED_MEDIA_TYPES`/sniff helper if reused by TUI; otherwise none.
- **S11. Docs** — `docs/PROTOCOL.md` upload contract + capability semantics + serve headers + native-adoption note; `CLAUDE.md` TODO tick + test count.

---

## 4. Test strategy (how each SC is pinned)

- **Server unit** (`media_store.rs`/`db.rs`): sniff/validate (spoof, allowlist, text→attachment) → SC1; insert/get with new fields + `sum_media_bytes_for_did` → SC3, SC5.
- **Server integration** (`tests/upload.rs`, in CI): spoofed type→415, served==sniffed → SC1; disposition+CSP+nosniff → SC2; `Content-Length` precheck + oversize 413 → SC3; `storage:"server"`/no-PDS-without-share → SC4; quota 413 + delete 404+gone → SC5; pre-seeded old row serves + no new required field → SC7; scope flag on/off → §2.6.
- **Web (vitest)**: `ComposeBox.test.tsx` `doUpload()` (FormData, 401/403 retry, URL→PRIVMSG) → SC6; `MessageList.media.test.tsx` video/audio gating + first-party ungated → SC6.
- **CI gate**: `.fabro/verify.sh` end-to-end → SC8.

---

## 5. Migration / back-compat

- **DB:** additive `ADD COLUMN` only (mirrors existing migration style); old rows null → `Option`, serve falls back to `mime`; no backfill required.
- **Existing blobs/URLs:** unchanged; capability-URL format + non-expiring invariant preserved → old pasted URLs keep working.
- **Request contract:** no new required field; `share_*` unchanged.
- **Response contract:** strictly additive (`storage`, `sha256`); old clients ignore unknown keys.
- **Serve contract:** additive headers; `inline` keeps current rendering; `attachment` only hits pdf/text.
- **One behavior change — validation tightening:** out-of-allowlist uploads now `415`. Allowlist = union of what clients already send; documented in `PROTOCOL.md`. Scope flag (off) and quota (256MB) default permissive to avoid surprising existing deployments.

---

## 6. Follow-up: native clients (future PRs — NOT implemented here)

All call `POST /api/v1/upload` and keep working after this change. To fully adopt:

- **`freeq-ios`** (`MediaCapture.swift`, `ComposeView.swift`, `PhotoPicker.swift`): restrict picker/camera to `ALLOWED_MEDIA_TYPES` + surface `415`; display `storage`; distinguish quota `413`; honor `Content-Disposition: attachment` for pdf/text.
- **`freeq-macos`** (`Views/Chat/FileUpload.swift`): same 1–4, plus external-media privacy toggle for video/audio.
- **`Freeq.WinUI`/`freeq-windows-app`** (`Services/UploadService.cs`): client-side allowlist + `415`; consume `storage`/`sha256`; respect disposition on download; quota-aware errors.

**Stable contract after this PR:** request fields unchanged; response adds `storage`+`sha256`; capability-URL format unchanged + non-expiring; serve adds `Content-Disposition` + per-route CSP + `nosniff`. Documented in `docs/PROTOCOL.md`.

---

## 7. Risks & open questions

- **Q1 — Sniffer dependency:** in-crate (~12 signatures, no new dep, plan's assumption) vs the `infer` crate (broader, adds a dep to gamma-334 server). Reviewer decides.
- **Q2 — Scope enforcement default:** shipped **off** because turning it on breaks unauthenticated inline `<img>`/`<video>` loads and the un-editable native clients (no session cookie on `<img>` GETs). Authz-at-serve is a larger, contract-affecting follow-up, deferred.
- **Q3 — `text/plain`:** allowed but always `attachment`+sandbox; alternative is dropping it entirely. Low stakes.
- **Q4 — Quota accounting:** counts server-stored bytes (always stored); PDS copies not double-counted.
- **Q5 — Container ambiguity:** MP4/MOV/M4A share ISO-BMFF `ftyp`; misclassification is cosmetic (both inline), not security-relevant.
- **Q6 — No streaming:** 10MB cap retained deliberately; raising it is out of scope (needs streaming cipher).
- **Risk — gamma hotspots:** changes localized to upload/serve handlers, test-first; no unrelated refactor.
- **Risk — `clippy -D warnings`:** new `insert_media` args may trip `too_many_arguments` (already `#[allow]`'d); keep the allow or introduce a `MediaInsert` struct.

---

Two design decisions I'd flag for the human gate before implementation: **Q1** (new `infer` dep vs in-crate sniffer) and **Q2** (whether scope-at-serve enforcement should ship dark/off as planned, since enabling it would break inline media in the native clients we can't edit). Everything else is additive and CI-verifiable.