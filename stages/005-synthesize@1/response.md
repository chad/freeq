Revised plan written to `/tmp/plan.md`. Note up front: **only `/tmp/review-compat.md` existed** — `review-security.md` and `review-ops.md` were missing/empty, so I did not invent their critiques. Here is the full revised plan:

---

# Implementation Plan: Harden & Enhance File Upload (on-server storage default) — REVISED

**Status:** revised after review · **Scope:** `freeq-server`, `freeq-sdk`, `freeq-app`, `freeq-tui`, `docs/`
**Out of scope (not edited here):** `freeq-ios`, `freeq-macos`, `Freeq.WinUI`, `freeq-windows-app`, **`freeq-android`**, AV crates.
**Guiding principle (CLAUDE.md):** *"If something feels 'too clever,' it's probably wrong."*

---

## Changes from review

**Reviews available:** Only `/tmp/review-compat.md` was present. **`/tmp/review-security.md` and `/tmp/review-ops.md` were missing/empty** — I did not invent their content. All changes below come from the one real (compatibility) review plus a code re-read.

**Folded in (compat review):**
- **C1 (HIGH) — `application/octet-stream` 415 break:** macOS (`ComposeBar.swift:387`) and Windows (`UploadService.cs:75`) default unknown files to `application/octet-stream`; the sniffer would 415 them. **Fix:** allowlist accepts `application/octet-stream` as a pass-through bucket — stored, never inline (always `attachment`), accepted only when sniffing finds no recognized *active* (HTML/SVG/script) bytes.
- **C2 (HIGH) — Windows omits `did`:** documented as Windows follow-up item 0 (pre-existing 400, not introduced here); SC7 claim corrected.
- **C3 (MED) — `freeq-android` absent:** added to out-of-scope list and §6.
- **C4 (MED) — S7 scope-authz test unimplementable:** no session identity on a media GET. **Cut full enforcement from this PR;** S7 becomes inert flag plumbing + a design note. "member 200 / non-member 403" criterion removed.
- **C5 (LOW-MED) — ISO-BMFF audio/video ambiguity:** "sniffed wins" qualified — declared type kept when declared+sniffed share the same container (audio/mp4 vs video/mp4).
- **C6 (LOW) — `content_type` value change:** §5 reworded; not called "strictly additive."
- **C7 (LOW) — `storage` must reflect real outcome:** track `pds_uploaded` bool; `"server+pds"` only on PDS `Ok`. New failure-case test.
- **C8 (LOW) — `text/plain` divergence:** **removed** `text/plain` from the server allowlist (no client sends it).

**Consciously rejected / deferred:**
- **Full scope-at-serve authorization:** deferred per C4 — no implementable auth surface today; needs per-recipient signed URLs or a media bearer token. Only inert plumbing + design note ship now.
- **Raising the 10MB cap:** out of scope (needs streaming cipher).
- **Backfilling `sha256`/`validated_type`:** out of scope; lazy `Option` fallback suffices.

---

## 0. Grounding facts

The private on-server store already exists and is the default: `POST /api/v1/upload` → `web.rs::api_upload` (always private store via `media_store::MediaStore`, signed capability URL, optional best-effort PDS push); served at `GET /api/v1/media/{id}/{sig}/{filename}` → `api_media_serve` (**no session identity in this request**). `media` table at `db.rs:430`; `soft_delete_media` unused/unwired. Global middleware already sets `nosniff` everywhere — so nosniff is NOT a gap; **content-type trust and `Content-Disposition` are.** No server-side sniffing today. Size cap 10MB checked post-buffer; only pre-buffer guard is `DefaultBodyLimit(12MB)` (10/12 mismatch). No quota/GC. Web app injects the URL into PRIVMSG text and re-detects by regex; only `<img>` honors `loadExternalMedia`.

---

## 1. Scope fence (re-confirmed)

**In scope:** `freeq-server`, `freeq-sdk`, `freeq-sdk-ffi`, `freeq-tui`, `freeq-bots`, `freeq-windows-core`, `freeq-app`, `docs/`+`CLAUDE.md`.
**Out of scope:** `freeq-ios`, `freeq-macos`, `Freeq.WinUI`, `freeq-windows-app`, **`freeq-android`** (added per C3), and the CI-excluded AV crates. Contract changes are additive/versioned so all six native targets adopt later (§6).

---

## 2. Design

### 2.1 Data model
`media` gets additive nullable columns `sha256 TEXT`, `validated_type TEXT` (`Option` in `MediaRow`/`get_media`; null → fall back to `mime`). New `db.rs::sum_media_bytes_for_did(did) -> u64` backs quota.

### 2.2 Content-type validation (SC1) — revised for C1, C5, C8
`media_store.rs::validate_content_type(declared, bytes) -> Result<&'static str, RejectReason>`:
1. **Sniff** via in-crate `sniff_media_type` (the ~12 signatures already in `pick_media_filename` + **active-markup detection**: leading `<`/`<!DOCTYPE`/`<html`/`<svg`/`<?xml`/`<script`, BOM+markup → `Active`).
2. **Decision table:**
   - `Active` → **415** regardless of declared type.
   - Recognized media `S`: if declared+`S` share the same ISO-BMFF container (declared ∈ {`audio/mp4`,`audio/x-m4a`}, `S`=`video/mp4`) and declared is allowlisted → keep **declared** (C5); else use **`S`** (sniffed wins across categories).
   - `Unknown` + declared `application/octet-stream` → accept **`application/octet-stream`** (C1 bucket).
   - `Unknown` + other declared → **415**.
3. Result must be in `ALLOWED_MEDIA_TYPES` or `octet-stream`, else **415**.

`ALLOWED_MEDIA_TYPES` (**no `text/plain`**, C8): jpeg/png/gif/webp, mp4/quicktime/webm, mpeg/mp4(m4a)/x-m4a/ogg/wav/x-wav, application/pdf, **application/octet-stream** (attachment-only). Called before `store.put`; `Err` → 415 store-nothing; validated type is recorded + returned.

> **Q1 (unchanged):** in-crate sniffer, no new dep; `infer` is the override.

### 2.3 Serve-path safety headers (SC2)
`api_media_serve` (Range + full): serve `validated_type` (fallback `mime`); assert global `nosniff` stays (route-level test); `Content-Disposition: inline` for image/video/audio, `attachment` for pdf/octet/other; per-route `Content-Security-Policy: default-src 'none'; sandbox`.

### 2.4 Size enforcement (SC3)
`MAX_UPLOAD_BYTES = 10MB` const; `DefaultBodyLimit::max(MAX_UPLOAD_BYTES + 256KB)`; `Content-Length` precheck → early 413; keep authoritative post-decode check.

### 2.5 On-server-default contract + response (SC4) — C6, C7
Track `pds_uploaded` (true only on PDS `Ok`). Response adds `storage` (`"server"` / `"server+pds"` by **actual outcome**) + `sha256`; `content_type` now carries the **validated** type. Request unchanged; no new required field.

### 2.6 Auth / capability — scope enforcement CUT to plumbing only (C4)
Capability URLs stay non-expiring, HMAC-over-`id`, unchanged. **No session identity on a media GET**, so full enforcement isn't implementable. Ship only `media_require_membership: bool` (default false), parsed/stored, **not consulted** in serve except a startup warn if true. Real design (per-recipient signed URLs or media bearer token) documented in `PROTOCOL.md` as a deferred follow-up. Possession=access remains the shipped model.

### 2.7 Quota & lifecycle (SC5)
`media_max_bytes_per_did` (default 256MB; 0=unlimited); `api_upload` checks `sum_media_bytes_for_did` → 413 on exceed. `delete_media(id, requester_did)` helper: authorize (uploader/op) → `soft_delete_media` → `store.remove`. `get_media` already filters `deleted_at IS NULL` → serve 404 post-delete. No new public REST endpoint.

### 2.8 Web app consumption (SC6)
`ComposeBox.tsx`: consume `storage`/`content_type`; request stays additive (no `text/plain` added). `MessageList.tsx` (test-first): generalize `loadExternalMedia` gate to external video/audio via `isTrustedMediaUrl`/`GatedMedia`; first-party `/api/v1/media/` ungated. No new store state.

---

## 3. Work breakdown (ordered; CI unless noted)

> Hotspots (test FIRST): `web.rs` (275), `server.rs` (334), `MessageList.tsx` (103).

- **S1.** Constants + size: `media_store.rs` consts; `web.rs:3155`/`:245` use them + `Content-Length` precheck. *(CI)*
- **S2.** Validation: `sniff_media_type` + `validate_content_type` (unit tests incl. octet pass-through, ISO-BMFF audio-keeps-declared, HTML-as-jpeg→415); `api_upload` calls before `put`. *(CI)*
- **S3.** DB columns: `ADD COLUMN sha256/validated_type`, extend `insert_media`/`MediaRow`/`get_media`, add `sum_media_bytes_for_did`. *(CI)*
- **S4.** Serve headers: disposition + per-route CSP + validated type (Range+full). *(CI)*
- **S5.** Quota: `config.rs` field; `api_upload` check. *(CI)*
- **S6.** Deletion: `delete_media` helper. *(CI)*
- **S7.** Scope-flag **plumbing only** (C4): `config.rs` flag + startup warn; no serve authz. *(CI)*
- **S8.** Response fields: `storage` (from `pds_uploaded`) + `sha256`. *(CI)*
- **S9.** Web app: `ComposeBox.tsx` consume fields + **415→"file type not supported"**; `MessageList.tsx` gate video/audio. New `ComposeBox.test.tsx`; extend `MessageList.media.test.tsx`. *(CI: vitest)*
- **S10.** SDK (optional): share sniffer/allowlist only if reused; no FFI ABI change. *(CI)*
- **S11.** Docs: `PROTOCOL.md` upload contract (incl. octet rule, `content_type` caveat, deferred scope note) + §6 pointers; `CLAUDE.md` TODO/test count.

---

## 4. Test strategy

**Server unit:** sniff/validate (HTML-as-jpeg→415, PNG ok, octet+unknown→pass, ISO-BMFF audio kept, image-declared+video-bytes→video) → SC1; db insert/get + sum → SC3, SC5.
**Server integration (`tests/upload.rs`):** spoofed/active→415, served==validated → SC1; disposition+CSP+nosniff → SC2; Content-Length precheck + oversize 413 → SC3; `storage:"server"`, no-PDS-without-share, **share_pds+PDS-fail→still "server"** (C7) → SC4; quota 413 + delete 404+gone → SC5; old null-column row serves + no new required field → SC7; `media_require_membership=true` doesn't change serve (C4) → §2.6.
**Web (vitest):** `doUpload()` FormData/401/403/**415**/URL→PRIVMSG → SC6; video/audio gating + first-party ungated → SC6.
**CI:** `.fabro/verify.sh` → SC8.

---

## 5. Migration / back-compat — corrected per C1, C2, C6

- Additive nullable `ADD COLUMN`; old rows `Option`-fallback to `mime`; no backfill.
- Existing blobs/URLs unchanged; capability format + non-expiring preserved.
- No new required request field. Clients **that already send `did`** (web, iOS, macOS, Android) upload as before *for allowlisted types*. **Windows already returns 400 today** (omits `did`, C2) — neither fixed nor worsened here; see §6 item 0.
- **Validation tightening (C1):** new 415 only for active/markup bytes or un-corroborated non-octet types. **macOS drag-drop and Windows unknown types send `octet-stream` → now pass** (stored, `attachment`). Prior "uploads exactly as before" claim removed.
- **Response (C6):** `storage`/`sha256` additive; **`content_type` value is now server-validated** (can differ from client-declared) — a value-semantics change, not additive-only. Zero impact today (no client reads it); adopters must treat it as authoritative.
- Serve headers additive; `inline` keeps current rendering; `attachment` only on pdf/octet.
- Defaults permissive (quota 256MB, scope-flag inert).

---

## 6. Follow-up: native clients (future PRs — NOT implemented here)

**`Freeq.WinUI`/`freeq-windows-app`** (`UploadService.cs`): **0. Send `did` form field — currently omitted → all Windows uploads 400 today (C2).** 1. Map unknowns or accept octet/`attachment`; surface 415. 2. Consume `storage`/`sha256`, treat `content_type` as authoritative. 3. Respect `attachment`. 4. Quota-aware 413.
**`freeq-ios`** (`MediaCapture.swift`/`ComposeView.swift`/`PhotoPicker.swift`): 1. Surface 415; voice (`audio/mp4`) preserved by C5. 2. `storage` + authoritative `content_type`. 3. Distinguish quota vs oversize 413. 4. Honor `attachment`.
**`freeq-macos`** (`{FileUpload,ComposeBar}.swift`): 1. `ComposeBar.swift:387` octet default now accepted (C1), served `attachment`; optionally map for inline. 2–4 as iOS + external-media privacy toggle.
**`freeq-android`** (`PhotoUpload.kt`, **added per C3**): 1. Surface 415 (only 401/413 handled at `:316–317` today). 2. Align `contentResolver.getType()` with allowlist; rely on octet bucket for unknowns (`cross_post` already accepted). 3. `storage`/`sha256` + authoritative `content_type`.

**Stable contract:** request unchanged; response adds `storage`+`sha256`, `content_type` becomes validated; capability URL unchanged + non-expiring; serve adds `Content-Disposition` + per-route CSP. Documented in `PROTOCOL.md`.

---

## 7. Risks & open questions

- **Q1** In-crate sniffer (chosen) vs `infer` crate.
- **Q2** Full scope-at-serve authz deferred (C4): needs new authenticated serve mechanism; possession=access until then.
- **Q3** `octet-stream` allowlist bucket (C1): mitigated by always-`attachment`, active-markup 415, sandbox CSP — deliberate compat/security balance.
- **Q4** ISO-BMFF declared-wins (C5): narrow audio/mp4 exception; other categories defer to sniffer.
- **Q5** `content_type` now server-authoritative (C6); documented; zero impact today.
- **Q6** 10MB cap retained (no streaming).
- **Q7** Quota counts server-stored bytes; PDS copies not double-counted.
- **Risk** Hotspot edits localized + test-first.
- **Risk** `insert_media` gains args — keep `#[allow(too_many_arguments)]` or use a `MediaInsert` struct.
- **Missing reviews:** `review-security.md` and `review-ops.md` were absent — **no independent security or ops sign-off exists.** Security-relevant design (sniffing, active-markup rejection, sandbox CSP, attachment disposition, quota, GC-on-delete) is present, but flag this gap for the human gate.

---

Two items for the human gate: (1) this plan ships **without** an independent security or ops review (those files were missing); (2) the biggest deliberate trade-off is **Q3** — accepting `application/octet-stream` as a stored-but-attachment-only bucket to avoid breaking macOS/Windows, defended by active-markup rejection + sandbox CSP. Everything else is additive and CI-verifiable.