# macOS Client Feature-Parity Plan

**Goal:** The macOS client should be feature-complete with the **superset** of the
iOS, web, and TUI clients.

Status legend: ☐ todo · ◐ in progress · ☑ done · ⛔ blocked

---

## Method

Four feature inventories were assembled (macOS / iOS / web / TUI) and ground-truthed
against the actual source. macOS turned out to already cover most of the surface
(auth, channels, messaging, edit/delete/react/reply/threads, signing, CHATHISTORY,
pins, member list, profiles, away, DMs, file upload, **image lightbox, Bluesky
embeds, YouTube thumbnails**, link previews, avatars, bookmarks, quick switcher,
notifications, autocomplete, settings, onboarding, MOTD, reconnect, P2P).

The Explore agent under-reported macOS media support — lightbox/Bluesky/YouTube are
all present in `Views/Chat/MediaViews.swift`.

---

## Confirmed gaps (macOS vs. superset)

### 1. AV — voice/video calls  (HEADLINE; present in iOS + web, absent on macOS)
The macOS `FreeqSDK.xcframework` was built **without** the `av` cargo feature
(0 AV symbols vs iOS's 17).
- ☑ Rebuild macOS SDK with `--features av`, library-mode bindgen, xcframework
      (`freeq-macos/build-rust.sh`). 17 AV symbols now in bindings.
- ☑ AppState AV state (stored props) + `CallController.swift` (AppState ext +
      AvCallbackHandler) ported from iOS
- ☑ Mic capture (`CallMicCapture` — AVAudioEngine, no iOS AVAudioSession)
- ☑ Camera capture (`CallCameraCapture` — AVCaptureSession → BGRA frames)
- ☑ `CallView` UI: participant tiles, mute/camera/expand/hangup, video grid
      (NSViewRepresentable for preview + AVSampleBufferDisplayLayer remote tiles)
- ☑ Signaling TAGMSGs: `av-start` / `av-join` / `av-leave` / `av-state`
- ☑ Toolbar call button per channel; session discovery via REST `/sessions`
- ☑ project.yml: AV frameworks + camera/mic usage strings + entitlements

### 2. Inline video/audio playback + voice messages (web/iOS have it)
- ☑ Inline `VideoPlayer` (AVKit) for `.mp4/.webm/.mov`
- ☑ Inline audio player for `.m4a/.mp3/.ogg/.wav`
- ☑ Voice message rendering (🎤) with playback
- ☐ (stretch) Voice message recording + on-device transcription — deferred

### 3. Slash-command parity (TUI is richest) — ☑ DONE
Added as typed commands + autocomplete + help:
- ☑ `/edit` `/delete` `/react` `/reply`
- ☑ `/pin` `/unpin` `/pins`
- ☑ `/ban` `/unban`
- ☑ `/list` `/names` `/who`
- ☑ `/search` `/find` (in-buffer)
- ☑ `/media` `/img` `/upload` `/crosspost`
- ☑ `/oper` `/reconnect`
- ☑ `/av start|join|leave|mute|camera`
- (`/encrypt` `/decrypt`: macOS uses DID-based E2EE, not TUI's passphrase model — n/a)

### 4. Smaller gaps — assessed
- ☑ Ban *commands* (`/ban` `/unban`) added. Read-only ban *list* UI is NOT
      buildable: the SDK's `FreeqEvent` exposes no ban-list case (367/368), so
      there's nothing to render. Would need an SDK event addition.
- ☑ Step-up auth: NOT needed on macOS. macOS uploads via the server's own
      `/api/v1/upload` (DID-based), not direct PDS blob upload, so the
      incremental-OAuth `blob_upload` scope dance iOS/web do doesn't apply.
- ☐ Channel join-gates / policy editor — web-unique UX; deferred (niche).

### Already present on macOS (verified, not gaps)
Image lightbox, Bluesky embeds, YouTube thumbnails, link previews, drag-and-drop
upload, DID-based E2EE, P2P DMs, pins, in-buffer search, member list / profiles,
away-notify, bookmarks, quick switcher, notifications, autocomplete, MOTD,
onboarding, reconnect.

### Superset items intentionally NOT ported (platform-inappropriate / different arch)
- Live Activity / Dynamic Island, Apple Watch app, CallKit — iOS-only OS surfaces.
- Siri Intents / Spotlight — iOS integrations; out of scope for parity.
- Vi-mode line editing, `/net` stats popup, raw-debug toggle — TUI terminal UX.
- Passphrase channel E2EE (`/encrypt`) — macOS uses DID-based E2EE instead.
- Voice-message *recording* + on-device transcription — iOS stretch; deferred
      (playback of received voice messages IS now supported).

---

## Result
macOS builds clean via `xcodebuild`, codesigns, and launches without crashing.
All substantive cross-platform features of the iOS/web/TUI superset are now
present; remaining deltas are platform-specific OS integrations or use a
different (already-present) architecture on macOS.

---

## Sequencing
- **A.** Kick off AV SDK build (background) — long pole.
- **B.** Pure-Swift media parity (video/audio/voice rendering).
- **C.** Slash-command parity.
- **D.** AV UI + capture + signaling (after SDK builds).
- **E.** Ban UI, policy, step-up auth.
- **F.** Build/compile verification with xcodebuild.

Each phase committed separately (attributed to Chad Fowler, no Claude co-author).
