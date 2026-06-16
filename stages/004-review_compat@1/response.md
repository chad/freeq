Here is the full critique:

---

# Compatibility Review: File Upload Hardening Plan

**Code verified against:** `web.rs`, `db.rs`, `media_store.rs`, `ComposeBox.tsx`, `MessageList.tsx`, `freeq-sdk/src/media.rs`, `freeq-sdk-ffi/src/lib.rs`, all four native clients.

---

## Scope Fence Assessment

The plan correctly excludes `freeq-ios`, `freeq-macos`, `Freeq.WinUI`, and `freeq-windows-app` from edits. §6's follow-up guidance per client is accurate in structure. **However, `freeq-android` is missing from both the out-of-scope list and §6** — it has a real upload path (`PhotoUpload.kt`) that will be affected.

---

## Issues (by severity)

---

### ISSUE 1 — `application/octet-stream` fallback → new hard 415 break for macOS and Windows

**Severity: HIGH.**

`freeq-macos/Views/Chat/ComposeBar.swift:387`:
```swift
default: contentType = "application/octet-stream"
```
`Freeq.WinUI/Services/UploadService.cs:75`:
```csharp
_ => "application/octet-stream",
```

Both clients fall back to `application/octet-stream` for unknown file types. After this plan's magic-byte validation, the sniffer cannot classify `application/octet-stream` as an allowlisted type and the server returns `415`. Both clients surface this as a generic "Upload failed" with no user guidance.

The plan's §5 claims "Old web/TUI/native clients upload exactly as before" — **this is false** for macOS file drag-and-drop of unrecognised types and all Windows uploads of anything outside the known-extension list.

**Required plan change:** Either (a) accept `application/octet-stream` with forced `Content-Disposition: attachment` (defense-in-depth still applies via CSP), or (b) explicitly document this as a known behavior change in §5 and add "fix the default fallback" as item 0 in §6 for macOS and Windows before 415 ships.

---

### ISSUE 2 — Windows `UploadService.cs` omits the `did` multipart field; SC7 is wrong

**Severity: HIGH (pre-existing bug the plan inherits and misrepresents).**

`Freeq.WinUI/Services/UploadService.cs` builds its multipart with only `file`, `channel`, `alt`. It never sends `did`. The server at `web.rs:3197–3198` returns `400 Bad Request` when `did` is empty. Windows does send a `Bearer` token in `Authorization`, but the server ignores it on the upload route (auth is via `upload_tokens` or `session_dids`). **Windows uploads already return 400 today.**

SC7 ("older clients upload exactly as before") is false for Windows. §6 for `Freeq.WinUI` says only "type allowlist + 415 surfacing / consume `storage`/`sha256` / respect `Content-Disposition` / quota messaging" — it omits the broken `did` field.

**Required plan change:** Add to §6 (Windows), item 0: "Add `did` form field to `UploadService.cs` — currently omitted, causing all uploads to return 400." Correct the SC7 claim.

---

### ISSUE 3 — `freeq-android` is entirely absent from the plan

**Severity: MEDIUM.**

`freeq-android/…/PhotoUpload.kt` is a real upload client: it correctly sends `did`, `channel`, `cross_post`, and the file. It handles `401` and `413` specially but has **no case for 415**. After this plan, any upload of a type the server rejects will show "Upload failed (415)" with no user guidance. The plan lists `freeq-android` neither in-scope nor out-of-scope, and §6 has no entry for it.

**Required plan change:** Add `freeq-android` to the out-of-scope list. Add a §6 entry: (1) surface 415 as "file type not supported", (2) note that Android's `contentResolver.getType()` may return types not in the server allowlist (and document the allowlist in PROTOCOL.md so Android can align).

---

### ISSUE 4 — S7 scope-authz test is unimplementable without designing auth on the serve route

**Severity: MEDIUM.**

`api_media_serve` receives only `ConnectInfo(addr)`, `State`, `HeaderMap`, and path params. There is **no session identifier** in a plain media GET. The server sets no session cookies (confirmed: zero `Set-Cookie` in `web.rs`). Session identity is in `state.session_dids` keyed by WebSocket session ID — which doesn't exist on media GETs. The web app fetches media via `<img src="...">` (browser GETs carry no auth headers without a cookie).

The plan acknowledges the serve-time auth problem in §2.6 ("turning it on by default would break every existing client's inline rendering") but still commits to a testable "member 200, non-member 403" integration test in S7. That test cannot pass without adding a new auth mechanism to the serve route, which the plan does not design.

**Required plan change:** S7 scope must be reduced to: "flag on/off is wired and defaults to off; actual per-DID enforcement is deferred as a follow-up that will design a token or cookie mechanism for the serve route." The S7 integration test should only assert flag-off preserves existing behavior.

---

### ISSUE 5 — "Sniffed type always wins" rule breaks the ISO-BMFF audio/video ambiguity more than the plan admits

**Severity: LOW-MEDIUM.**

iOS sends voice recordings as `filename="voice.m4a"`, `Content-Type: audio/mp4`. An ISO-BMFF magic-byte sniffer may return `video/mp4` (the container is identical for MP4 and M4A). The plan dismisses this in Q5 as "cosmetic." It is partly correct but the rule "served type is the sniffed type, never the declared type" is too broad:

- **Named-file path** (`voice.m4a`): `pick_media_filename("voice.m4a", "video/mp4")` preserves the `.m4a` extension (line 3413 branch: `n.contains('.')` → keep provided name). The web app's `AUDIO_URL_RE = /…\.(?:m4a|…)/i` still matches. Audio player renders. ✓

- **No-filename path** (any client omitting a filename): `pick_media_filename(None, "video/mp4")` yields `.mp4`. The web app's `VIDEO_URL_RE` matches first. A voice clip renders as a video player. ✗ Regression.

Additionally, the `content_type` returned in the upload response changes from `"audio/mp4"` (client-declared) to `"video/mp4"` (sniffed). No current native client reads this field from the response, so no break today — but future adopters of the new `storage`/`content_type` response fields will receive a misleading type for voice content.

**Required plan change:** Qualify the "sniffed type wins" rule: when both the sniffed type and the declared type are allowlisted **and share the same underlying container format** (ISO-BMFF: `audio/mp4`, `audio/x-m4a`, `video/mp4`, `video/quicktime`), preserve the declared type. The sniffed type wins only when the two types differ in *category* (e.g., declared `image/jpeg` but sniffed `video/mp4`).

---

### ISSUE 6 — `"content_type"` response field value change is not "strictly additive"

**Severity: LOW. Wording error that will mislead future implementers.**

The upload response currently returns `"content_type": "<client-declared>"`. After the plan it returns `"content_type": "<server-sniffed>"`. This is a **value change in an existing field**, not a new field. §5 calls the response changes "strictly additive" — that claim is false. In practice, no current native client reads `content_type` from the response (all read only `url`), so the real-world impact is zero today. But the characterisation matters for clients that will adopt the new `storage`/`sha256` fields and expect `content_type` to be stable.

**Required plan change:** §5 should say: "Response contract: new fields `storage` and `sha256` are additive. The existing `content_type` field now holds the server-validated type rather than the client-declared type; in the common case these match. Clients adopting the new fields should not assume `content_type` equals what they sent."

---

### ISSUE 7 — `"storage"` field will report `"server+pds"` even when the PDS upload silently failed

**Severity: LOW. Misleading to any client that adopts the new field.**

The PDS upload is best-effort (`web.rs:3336–3381`): on `Err(e)` it logs a warning and continues. The plan says `storage = "server+pds"` when "share_pds succeeded." If the implementation sets this field based on the `share_pds` *request flag* rather than actual PDS success, clients that adopted the field will believe the media is on the PDS when it is not.

**Required plan change:** §2.5 must state the `storage` value is determined by actual PDS upload success. Track a `pds_uploaded: bool` local variable; set it `true` only in the `Ok(result)` branch. S8 test must assert `storage == "server"` when `share_pds=true` was requested but PDS upload failed (simulate via a disabled/missing PDS session).

---

### ISSUE 8 — `text/plain` in server allowlist with no client ever sending it

**Severity: LOW. Benign inconsistency but needs a rationale.**

The plan adds `text/plain` to `ALLOWED_MEDIA_TYPES`. No client sends it: the web app's `ALLOWED_TYPES` excludes it, macOS/iOS/Windows have no `.txt` case, and the TUI uses the PDS path. `text/plain` is therefore only reachable via `curl` or future clients, and is always served as `attachment` (safe). The plan should either document this explicitly ("server-only, forward-compatibility placeholder") or remove it to keep the allowlist honest.

---

## What the Plan Gets Right

- Capability URL format unchanged (HMAC over `id`, non-expiring). Existing capability URLs in old messages keep working. ✓
- DB migrations are additive (`ADD COLUMN` only, nullable). No migration risk. ✓
- No new required upload request fields (SC7 holds for iOS, macOS-with-`did`, Android). ✓
- `scope` enforcement defaults **off**; `<img>` loads from native clients are unaffected by default. ✓
- `Content-Disposition: inline` for image/video/audio is correct — consistent with the iOS code that already works around PDS `attachment` disposition on `AVPlayer`. `attachment` only for pdf/text. ✓
- Per-route CSP `default-src 'none'; sandbox` does not affect `AVPlayer` (native HTTP clients ignore CSP). ✓
- `loadExternalMedia` defaults `true` (`localStorage !== 'false'`) — gating external video/audio for users in privacy mode is a security improvement, not a regression. ✓
- `freeq-tui` uses `freeq_sdk::media::upload_media_to_pds` (PDS path), not the server upload route. Unaffected by server-side content-type validation. ✓
- `freeq-sdk` / `freeq-sdk-ffi`: S10 changes are optional helpers only. No FFI ABI change planned. ✓
- iOS `MessageList` already proxies PDS audio to avoid `Content-Disposition: attachment` blocking `AVPlayer`. The plan's `inline` disposition for freeq-server audio will make this workaround unnecessary for server-hosted content. ✓

---

## Summary Table

| # | Break / Risk | Who it affects | Required plan change |
|---|---|---|---|
| 1 | `application/octet-stream` → new 415 break | macOS drag-and-drop, Windows unknown types | Add to allowlist (forced attachment) or document the break + §6 fix |
| 2 | Windows `UploadService.cs` omits `did` (pre-existing; SC7 falsely says it works) | Windows users | Add "send `did`" to §6 Windows; correct SC7 |
| 3 | `freeq-android` absent from scope fence and §6 | Android users | Add to out-of-scope list; add §6 entry with 415 and allowlist guidance |
| 4 | S7 scope-authz test requires serve-route auth that doesn't exist | Implementation | Scope S7 test down to flag-wiring only; defer auth design |
| 5 | "Sniffed wins" rule → `audio/mp4` stored/served as `video/mp4` for unnamed uploads | iOS voice (named OK), future nameless audio uploads | Qualify rule: declared type wins when same container, different subtype |
| 6 | `content_type` value changes but §5 says "strictly additive" | Plan readers / future native adopters | Fix §5 wording to acknowledge value change |
| 7 | `storage` field reports request intent, not PDS outcome | Web app / future adopters | Track `pds_uploaded` bool; test PDS-fail → `storage=="server"` |
| 8 | `text/plain` in server allowlist, no client sends it | Latent inconsistency | Document intentionality or remove |