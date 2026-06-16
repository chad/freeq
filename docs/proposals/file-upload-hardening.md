# Proposal: Harden & Enhance File Upload (on-server storage default)

> **Provenance:** drafted by the Fabro `feature` workflow (run `01KV70WV0C0X3AZ5PZMKHBW91W`)
> on the `fabro-freeq` boxd VM — research → plan → peer review → synthesis.
> **Review status:** the compatibility review fed in and is folded below (items C1–C8);
> the security and storage/ops reviews did not reach the synthesizer this run (parallel-node
> sandbox isolation — a known wiring limitation), so those lenses are NOT yet applied. Treat
> this as a strong draft spec pending a security + ops pass, not a finished design.

---

# Implementation Plan: Harden & Enhance File Upload (on-server storage default) — REVISED

**Status:** revised after review · **Scope:** `freeq-server`, `freeq-sdk`, `freeq-app`, `freeq-tui`, `docs/`
**Out of scope (not edited here):** `freeq-ios`, `freeq-macos`, `Freeq.WinUI`, `freeq-windows-app`, **`freeq-android`**, AV crates.
**Guiding principle (CLAUDE.md):** *"If something feels 'too clever,' it's probably wrong."* Keep changes minimal, auditable, additive.

---

## Changes from review

**Reviews available:** Only `/tmp/review-compat.md` was present. **`/tmp/review-security.md` and `/tmp/review-ops.md` were missing/empty** — I did not invent their content. All changes below come from the one real (compatibility) review plus my own re-read of the code. Where a security/ops concern is obvious and already in the original plan (sniffing, sandbox CSP, quota, GC) it stays; I did **not** fabricate new security/ops critiques to fill the gap.

**Folded in (compat review):**
- **C1 (HIGH) — `application/octet-stream` 415 break:** macOS (`ComposeBar.swift:387`) and Windows (`UploadService.cs:75`) default unknown files to `application/octet-stream`. The sniffer would 415 them. **Fix:** the allowlist now accepts `application/octet-stream` as a pass-through bucket — stored, never served inline (always `Content-Disposition: attachment`), and only when magic-byte sniffing yields *no recognized dangerous active type* (HTML/SVG/script bytes still 415 regardless of declared type). §5 corrected: the "uploads exactly as before" claim is removed and replaced with a precise statement.
- **C2 (HIGH) — Windows omits `did`:** documented in §6 as Windows follow-up item 0 (pre-existing 400, not introduced here). SC7 wording corrected to scope its back-compat claim to clients that already send `did`.
- **C3 (MED) — `freeq-android` absent:** added to the out-of-scope list (header + §1) and given a §6 follow-up entry.
- **C4 (MED) — S7 scope-authz test is unimplementable:** there is no session identifier on a media GET (no cookie, no WS session, `<img>` carries no auth). **Decision: cut full scope-enforcement from this PR.** S7 is reduced to introducing the config flag *plumbing only* (default off, no behavioral authz) plus a documented design note for the real follow-up (a bearer-token or signed-per-recipient serve mechanism). The "member 200 / non-member 403" success criterion is **removed**; replaced with a flag-plumbing test.
- **C5 (LOW-MED) — ISO-BMFF audio/video ambiguity:** §2.2 now qualifies the "sniffed type wins" rule: the sniffed type wins only when it is a **different category** from the declared type (e.g. declared `image/*` but sniffed `video/mp4`). When declared and sniffed share the same container (ISO-BMFF: `audio/mp4`/`audio/x-m4a` vs `video/mp4`), and the declared type is allowlisted, the **declared type is kept**.
- **C6 (LOW) — `content_type` value change:** §5 reworded — `storage`/`sha256` are additive; `content_type`'s *value* now reflects the validated type (can differ from client-declared). Not called "strictly additive."
- **C7 (LOW) — `storage` must reflect real outcome:** §2.5 now tracks a `pds_uploaded` bool; `storage = "server+pds"` only on the PDS `Ok` branch, else `"server"`. New test asserts `storage=="server"` when `share_pds=true` but PDS upload fails.
- **C8 (LOW) — `text/plain` divergence:** **Decision: remove `text/plain` from the server allowlist.** No client sends it; keeping it adds a server-only path nobody exercises. (Reviewer's option (a).) `octet-stream` already covers the "unknown but allowed, attachment-only" case for curl/future clients.

**Consciously rejected / deferred (with reasons):**
- **Full scope-at-serve authorization (the original §2.6 "dark flag" with member/non-member enforcement):** rejected for this PR per C4 — it has no implementable auth surface today and would require a new authenticated media-serve mechanism (per-recipient signed URLs or a media bearer token). Deferred to a dedicated follow-up; only inert flag plumbing + a design note ship now.
- **Raising the 10MB cap:** still out of scope (would need a streaming cipher). Unchanged.
- **Backfilling `sha256`/`validated_type` for old rows:** still out of scope; lazy `Option` fallback is sufficient.

---

## 0. Grounding facts (from exploration)

The private on-server store **already exists and is the default**:

- Upload: `POST /api/v1/upload` → `web.rs::api_upload` (`:3113`). Always stores privately via `media_store::MediaStore`, mints a signed capability URL, optionally also pushes to PDS (best-effort, `web.rs:3336–3381`).
- Store: `media_store.rs` — AES-256-GCM at rest, HMAC capability URLs over `id`, `sanitize_filename` path-traversal guard, 0700/0600 perms, `MAX` 10MB rationale at `:15`.
- Serve: `GET /api/v1/media/{id}/{sig}/{filename}` → `web.rs::api_media_serve` (`:3791`). HMAC-verifies, decrypts, Range support. **Receives only `ConnectInfo(addr)`, `State`, `HeaderMap`, path `(id,sig,filename)` — no session identity.**
- DB: `media` table (`db.rs:430`) with `insert_media`/`get_media`/`soft_delete_media`. **`scope` recorded but not enforced; `soft_delete_media` is unused and not wired to physical removal.**
- Global middleware `security_headers` (`web.rs:4180`) **already sets `X-Content-Type-Options: nosniff` on every response, incl. media.** nosniff is NOT a gap; content-type *trust* and `Content-Disposition` are.
- **No server-side content-type validation / magic-byte sniffing.** Server trusts the client-declared multipart `Content-Type`. Web client whitelists (`ComposeBox.tsx:11`); macOS/Windows default unknown → `application/octet-stream`; Android uses `contentResolver.getType()`; TUI uses the PDS path (not this route).
- Size cap: `10MB` checked **after** full buffering (`web.rs:3155`); only pre-buffer guard is `DefaultBodyLimit::max(12MB)` (`web.rs:245`). 10 vs 12 inconsistency.
- No per-DID quota, no GC. No `infer`/`mime` crate in `freeq-server/Cargo.toml`.
- Web app puts the capability URL **into PRIVMSG text**; `MessageList.tsx` re-detects by regex. Only `<img>` honors `loadExternalMedia`; video/audio don't. `MessageList.tsx` (gamma 103) and `doUpload()` undertested.

---

## 1. Scope fence (re-confirmed)

**In scope (CI-buildable, editable):** `freeq-server`, `freeq-sdk`, `freeq-sdk-ffi`, `freeq-tui`, `freeq-bots`, `freeq-windows-core` (Rust), `freeq-app`, and `docs/` + `CLAUDE.md`.

**Out of scope (cannot compile/verify on this Linux executor):** `freeq-ios`, `freeq-macos`, `Freeq.WinUI`, `freeq-windows-app` (Swift/WinUI), **`freeq-android`** (Kotlin — added per C3), and the AV crates CI excludes (`freeq-av`, `freeq-eliza`, `freeq-av-client`, `freeq-av-image`).

Contract changes are **additive and versioned** so all six out-of-scope native targets can adopt later (§6).

---

## 2. Design

### 2.1 Data model

`media` table — **additive nullable columns** (`db.rs` `ALTER TABLE ... ADD COLUMN`, existing migration pattern):
- `sha256 TEXT` — hex digest of plaintext (integrity/dedupe; null for old rows).
- `validated_type TEXT` — the server-derived type actually served (null → fall back to `mime`).

`get_media`/`MediaRow` return new fields as `Option`. New `db.rs::sum_media_bytes_for_did(did) -> u64` (SUM(size) WHERE uploader_did=? AND deleted_at IS NULL) backs quota.

### 2.2 Content-type validation (SC1) — revised for C1, C5, C8

New `media_store.rs::validate_content_type(declared: &str, bytes: &[u8]) -> Result<&'static str, RejectReason>`:

1. **Sniff** `bytes` → an in-crate `sniff_media_type(&[u8]) -> SniffResult` covering the ~12 signatures already enumerated in `pick_media_filename` (JPEG/PNG/GIF/WEBP, ISO-BMFF `ftyp` → mp4 family, WEBM/Matroska, MP3/ID3, OGG, WAV/RIFF, PDF). It also detects **dangerous active types** by signature/heuristic: leading `<`/`<!DOCTYPE`/`<html`/`<svg`/`<?xml`/`<script`, or a UTF-8 BOM followed by markup → flagged `Active`.
2. **Decision table** (resolves C1, C5):
   - If sniff = `Active` (HTML/SVG/XML/script-ish) → **415**, regardless of declared type. This is the security core: spoofing a script as `image/jpeg` is rejected.
   - Else if sniff = a recognized media type `S`:
     - If `declared` and `S` are the **same category** container (both ISO-BMFF; declared ∈ {`audio/mp4`,`audio/x-m4a`} while `S`=`video/mp4`) **and `declared` is allowlisted** → keep **`declared`** (C5: audio/m4a not downgraded to video).
     - Else → use **`S`** (sniffed wins across categories; defeats `image/jpeg`-declared-but-`video/mp4`-bytes).
   - Else if sniff = `Unknown` (no recognized signature, no active markers) **and** `declared == "application/octet-stream"` → accept as **`application/octet-stream`** (C1 pass-through bucket).
   - Else (`Unknown` + declared is some other non-octet type we can't corroborate) → **415**.
3. Result type must be in `ALLOWED_MEDIA_TYPES` or be `application/octet-stream`; otherwise **415**.

`ALLOWED_MEDIA_TYPES` (single server-side const; **no `text/plain`** per C8): `image/jpeg`, `image/png`, `image/gif`, `image/webp`, `video/mp4`, `video/quicktime`, `video/webm`, `audio/mpeg`, `audio/mp4`, `audio/x-m4a`, `audio/ogg`, `audio/wav`, `audio/x-wav`, `application/pdf`, **`application/octet-stream`** (attachment-only).

`api_upload` calls `validate_content_type` after reading bytes, **before** `store.put`; on `Err` returns **415** with a short body (`"unsupported or unverifiable file type"`) and stores nothing. The validated type is what's recorded (`validated_type`) and returned (`content_type`).

> **Decision (Q1, unchanged):** in-crate sniffer, no new dependency on the gamma-334 server. `infer` remains the reviewer-overridable alternative.

### 2.3 Serve-path safety headers (SC2)

In `api_media_serve` (both Range and full paths):
- `Content-Type` = `row.validated_type` (fallback `row.mime`).
- `X-Content-Type-Options: nosniff` — already set globally; add a **route-level assertion test** so a future middleware change can't silently drop it.
- `Content-Disposition`:
  - `inline; filename="{sanitized}"` for `image/*`, `video/*`, `audio/*` (renderable; needed for `<img>`/`<video>`/`AVPlayer`).
  - `attachment; filename="{sanitized}"` for `application/pdf` and **`application/octet-stream`** and anything else.
- `Content-Security-Policy: default-src 'none'; sandbox` on this route (handler-set; global middleware skips when CSP present, `web.rs:4199`). Neutralizes any stored markup even if validation were bypassed. (Reviewer confirmed AVPlayer ignores CSP.)

### 2.4 Size enforcement (SC3)

- `pub const MAX_UPLOAD_BYTES: usize = 10 * 1024 * 1024;` in `media_store.rs`; used everywhere.
- `DefaultBodyLimit::max(MAX_UPLOAD_BYTES + 256 * 1024)` (cap + multipart envelope), replacing the literal 12MB.
- In `api_upload`, **before** reading the `file` field, check the request `Content-Length` header (if present) against `MAX_UPLOAD_BYTES + overhead` → early `413`. Keep the authoritative post-decode `> MAX_UPLOAD_BYTES` check (transfer-encoding can differ).

### 2.5 On-server-default contract + response shape (SC4) — revised for C6, C7

Track `let mut pds_uploaded = false;` set `true` **only** in the PDS `Ok(result)` branch (`web.rs:3355`). Response (additive `storage`/`sha256`; existing keys kept):

```json
{ "url": "...", "content_type": "<validated>", "size": 1234,
  "private": true, "storage": "server", "sha256": "<hex>" }
```

- `storage = if pds_uploaded { "server+pds" } else { "server" }` — reflects **actual outcome**, not the request flag (C7).
- `content_type` now carries the **validated** type (may differ from client-declared when magic bytes disagree — see §5, C6).
- Request unchanged: `file, did, alt?, channel?, share_pds?, share_bluesky?/cross_post?`. **No new required field.**

### 2.6 Auth / capability — scope enforcement CUT to plumbing only (C4)

- Capability URLs stay **non-expiring**, HMAC-over-`id`, format unchanged (CHATHISTORY replay).
- **There is no session identity on a media GET** (no cookie, no WS session, `<img>` sends no auth). Full member/non-member enforcement is therefore **not implementable** without a new authenticated serve mechanism. **This PR ships only inert config plumbing:**
  - Add `media_require_membership: bool` (default `false`) to `config.rs`/`ServerConfig`, parsed and stored but **not consulted in `api_media_serve`** beyond a `tracing::warn!` once at startup if set to `true` ("media_require_membership has no effect until the authenticated-serve follow-up ships").
  - Document the real design in a `docs/PROTOCOL.md` note: full enforcement requires either (a) per-recipient signed/expiring URLs, or (b) a media bearer token presented on the serve request. Deferred.
- This keeps "possession = access" as the shipped model and does not break any inline rendering or native client.

### 2.7 Quota & lifecycle (SC5)

- `media_max_bytes_per_did: u64` config (default `256 * 1024 * 1024`; `0` = unlimited). `api_upload` calls `sum_media_bytes_for_did(did)`; if `current + size > limit` → `413` body `"storage quota exceeded"`.
- Deletion wiring: `delete_media(id, requester_did)` helper — authorize (uploader OR channel op via existing op checks) → `db.soft_delete_media` → `store.remove(id)`. `get_media` already filters `deleted_at IS NULL`, so serving returns `404` post-delete; the new `store.remove` reclaims disk. No new public REST endpoint in this PR (helper + unit/integration coverage only).

### 2.8 Web app consumption (SC6)

- `ComposeBox.tsx`: consume `result.storage`/`result.content_type` for the success affordance; request stays additive (still sends file/did/channel/alt/share_*). No `text/plain` added to client allowlist (C8).
- `MessageList.tsx` (gamma 103, test-first): generalize the `loadExternalMedia` gate to **video and audio** for *external* (non-first-party) URLs, matching `GatedImage`. First-party `/api/v1/media/` stays ungated. Implement via `isTrustedMediaUrl` (generalized from `isTrustedImageUrl`) + a `GatedMedia` wrapper.
- No new store state; reuse `loadExternalMedia`.

---

## 3. Work breakdown (ordered; CI-verified unless noted)

> Hotspots (write test FIRST): `web.rs` (275), `server.rs` (334), `MessageList.tsx` (103).

- **S1. Constants + size enforcement** *(CI)* — `media_store.rs` `MAX_UPLOAD_BYTES`, `ALLOWED_MEDIA_TYPES`; `web.rs` `:3155`/`:245` use constant + `Content-Length` precheck. Test first: extend `upload_rejects_oversized_file` + a precheck case.
- **S2. Content-type validation** *(CI)* — `media_store.rs` `sniff_media_type` + `validate_content_type` (+module unit tests incl. octet-stream pass-through, ISO-BMFF audio-keeps-declared, HTML-as-jpeg→reject); `api_upload` calls before `put`, records `validated_type`. Test first.
- **S3. DB additive columns** *(CI)* — `db.rs` `ADD COLUMN sha256/validated_type`, extend `insert_media`/`MediaRow`/`get_media`, add `sum_media_bytes_for_did`. Test first: insert/get/sum.
- **S4. Serve-path headers** *(CI)* — `api_media_serve` disposition + per-route CSP + validated type, Range + full. Test first: header assertions (incl. nosniff present).
- **S5. Quota** *(CI)* — `config.rs` `media_max_bytes_per_did`; `api_upload` check. Test first: over-quota→413.
- **S6. Deletion wiring** *(CI)* — `delete_media` helper (authorize → soft-delete → `store.remove`). Test first: post-delete 404 + blob file absent.
- **S7. Scope-flag plumbing only (C4)** *(CI)* — `config.rs` `media_require_membership` (default false) + startup warn when true; **no serve-path authz**. Test: config parses; flag does not change serve behavior.
- **S8. Response fields** *(CI)* — `api_upload` adds `storage` (from `pds_uploaded`) + `sha256`. Test first: `storage=="server"` for private; `storage=="server"` when `share_pds=true` but PDS fails (C7); validated `content_type` returned.
- **S9. Web app** *(CI: vitest)* — `ComposeBox.tsx` consume fields; `MessageList.tsx` gate video/audio. Tests first: new `ComposeBox.test.tsx` for `doUpload()` (FormData, 401 broker-refresh, 403 step-up retry, **415 surfaced as "file type not supported"**, URL→PRIVMSG); extend `MessageList.media.test.tsx` for video/audio gating.
- **S10. SDK (optional)** *(CI)* — only if the sniffer/allowlist is placed in `freeq-sdk` for reuse; no FFI ABI change. Otherwise skip.
- **S11. Docs** *(content; files build-irrelevant)* — `docs/PROTOCOL.md`: upload contract (fields, cap, validation incl. octet-stream rule, response incl. `storage`/`sha256` semantics + `content_type` value caveat), capability-URL semantics, serve headers, the deferred scope-enforcement design note, and §6 native-adoption pointers. `CLAUDE.md`: TODO tick + `web.rs` test count.

---

## 4. Test strategy (how each SC is pinned)

**Server unit (`media_store.rs` / `db.rs` `#[cfg(test)]`):**
- `sniff_media_type`/`validate_content_type`: HTML-bytes-as-`image/jpeg`→415; PNG ok; `octet-stream`+unknown→pass; ISO-BMFF + declared `audio/mp4`→kept as `audio/mp4` (C5); declared `image/jpeg` + sniffed `video/mp4`→`video/mp4`. → **SC1**
- `db`: insert/get with `sha256`+`validated_type`; `sum_media_bytes_for_did`. → **SC3, SC5**

**Server integration (`freeq-server/tests/upload.rs`, in CI):**
- Spoofed/active type → 415; served `Content-Type` == validated. → **SC1**
- Served blob has `Content-Disposition` (inline for image, attachment for octet/pdf), per-route CSP, nosniff. → **SC2**
- `Content-Length` precheck 413; oversized decoded 413; single cap constant. → **SC3**
- Private upload → `storage:"server"`, `private:true`; bytes never reach PDS without `share_pds`; `share_pds=true` w/ PDS failure → still `storage:"server"` (C7). → **SC4**
- Quota exceeded → 413; `delete_media` → serve 404 + blob gone. → **SC5**
- Pre-seeded old-style `media` row (null `sha256`/`validated_type`) still serves; upload with no new request field works. → **SC7**
- `media_require_membership=true` does **not** change serve behavior (plumbing only, C4). → **§2.6**

**Web (vitest):**
- `ComposeBox.test.tsx`: `doUpload()` FormData fields, 401/403 retry, **415→"file type not supported"**, URL→PRIVMSG, reads validated `content_type`. → **SC6**
- `MessageList.media.test.tsx`: external video/audio gated; first-party `/api/v1/media/` ungated. → **SC6**

**CI gate:** `.fabro/verify.sh` (`fmt`, `check`, `clippy -D warnings`, `test --workspace` w/ exclusions, `freeq-app` vitest). → **SC8**

---

## 5. Migration / back-compat — corrected per C1, C2, C6

- **DB:** additive nullable `ADD COLUMN` only (existing migration style). Old rows null → `Option` fallback to `mime`; no backfill.
- **Existing blobs/URLs:** unchanged on disk; capability-URL format + non-expiring invariant preserved → old pasted URLs keep working forever.
- **Request contract:** no new required field; `share_*`/`cross_post` alias unchanged. Clients **that already send `did`** (web, iOS, macOS, Android) upload as before *for allowlisted types*. **Windows (`Freeq.WinUI`) already returns 400 today** because it omits `did` (C2) — this PR neither fixes nor worsens that; see §6 item 0.
- **Behavior change — validation tightening (C1):** uploads now `415` when bytes are an active/markup type or an un-corroborated non-octet type. **macOS file drag-and-drop and Windows unknown-type uploads send `application/octet-stream`; these now pass** (stored, served `attachment`). So the only *new* rejections are genuinely dangerous or unidentifiable-and-mislabeled content. The prior plan's "uploads exactly as before" claim was wrong and is removed.
- **Response contract (C6):** `storage` and `sha256` are **additive**. The existing **`content_type` field's value** now reflects the **server-validated** type rather than the client-declared one; in the common case they match, but they can differ when magic bytes disagree with the declaration. This is a value-semantics change, not an additive-only change. No client reads this field today, so impact is zero now, but adopters must treat `content_type` as authoritative.
- **Serve contract:** new headers additive; `inline` keeps current rendering for media; `attachment` only hits pdf/octet (not previously inline-rendered).
- **Defaults stay permissive:** quota 256MB, scope-flag off/inert — no surprise for existing deployments.

---

## 6. Follow-up: native clients (future PRs — NOT implemented here)

All call `POST /api/v1/upload`. After this PR they keep working for allowlisted types; to fully adopt:

**`Freeq.WinUI` / `freeq-windows-app`** (`Services/UploadService.cs`):
0. **Send the authenticated `did` as a multipart form field — currently omitted, so all Windows uploads return 400 today (C2).** (Pre-existing; flagged so the follow-up doesn't miss it.)
1. Map unknown file types to an allowlisted type or accept the `attachment` octet-stream path; surface `415` as "file type not supported."
2. Consume `storage`/`sha256`; treat `content_type` as server-authoritative.
3. Respect `Content-Disposition: attachment` on download.
4. Quota-aware error messaging for the new quota `413`.

**`freeq-ios`** (`Views/MediaCapture.swift`, `ComposeView.swift`, `PhotoPicker.swift`):
1. Surface `415` distinctly. Voice messages (`audio/mp4`, `voice.m4a`) keep rendering correctly under the C5 rule.
2. Consume `storage`; treat `content_type` as authoritative.
3. Distinguish quota `413` from oversize `413`.
4. Honor `attachment` for pdf/octet.

**`freeq-macos`** (`Views/Chat/{FileUpload.swift, ComposeBar.swift}`):
1. **`ComposeBar.swift:387` defaults unknown files to `application/octet-stream` — now accepted server-side (C1), served `attachment`. Optionally map to specific types for inline rendering.**
2–4 as iOS, plus an external-media privacy toggle for video/audio to match the web gate.

**`freeq-android`** (`…/PhotoUpload.kt`) — **added per C3:**
1. Surface `415` as "file type not supported" (`PhotoUpload.kt:316–317` only handles 401/413 today).
2. Align `contentResolver.getType()` output with the documented `ALLOWED_MEDIA_TYPES`; rely on the `octet-stream` bucket for unknowns. (`cross_post` alias already accepted server-side.)
3. Consume `storage`/`sha256`; treat `content_type` as authoritative.

**Stable contract after this PR:** request fields unchanged (no new required field); response adds `storage`+`sha256` and `content_type` becomes server-validated; capability-URL format unchanged + non-expiring; serve adds `Content-Disposition` + per-route CSP (nosniff already present). Documented in `docs/PROTOCOL.md`.

---

## 7. Risks & open questions

- **Q1 — Sniffer dependency:** in-crate (~12 signatures + active-markup detection, no new dep; plan's choice) vs the `infer` crate. Reviewer-overridable.
- **Q2 — Full scope-at-serve authz (deferred, C4):** needs a new authenticated serve mechanism (per-recipient signed/expiring URLs or a media bearer token). Shipped as inert flag only. The non-expiring, possession=access model remains until that follow-up.
- **Q3 — `application/octet-stream` as an allowlist bucket (C1):** trades "reject unknowns" for native-client compat. Mitigated by: always `attachment` (never inline), active-markup bytes still 415 regardless of declared type, and sandbox CSP. This is the deliberate compat/security balance.
- **Q4 — ISO-BMFF declared-wins rule (C5):** narrow exception (audio/mp4 vs video/mp4 share `ftyp`). Misclassification across other categories still defers to the sniffer. Voice-message UX preserved.
- **Q5 — `content_type` value semantics (C6):** now server-authoritative; documented. Zero impact today (no client reads it).
- **Q6 — No streaming / 10MB cap:** retained deliberately; raising it needs a streaming cipher; out of scope.
- **Q7 — Quota accounting:** counts server-stored bytes (always stored); PDS copies not double-counted.
- **Risk — gamma hotspots:** changes localized to upload/serve handlers + `MessageList` gating, all test-first; no unrelated refactor.
- **Risk — `clippy -D warnings`:** `insert_media` gains args (`sha256`,`validated_type`) — keep the existing `#[allow(clippy::too_many_arguments)]` or introduce a `MediaInsert` struct if clippy escalates.
- **Missing reviews:** `review-security.md` and `review-ops.md` were absent. Security-relevant design (sniffing, active-markup rejection, sandbox CSP, attachment disposition, quota, GC-on-delete) is present from the original plan, but **no independent security or ops sign-off exists** — flag for the human gate.
