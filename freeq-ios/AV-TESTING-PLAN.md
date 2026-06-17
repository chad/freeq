# iOS AV (audio/video) — make it work well + deep automated flow tests

Goal: iOS video & audio work really well; deep AUTOMATED tests proving audio
and video actually flow end-to-end. No human interaction. Work until done.

## Architecture (as discovered)
- Swift capture → Rust SDK via UniFFI (`FreeqAv`):
  - mic: `CallMicCapture` → `FreeqAv.pushAudioFrame(samples:[Float])`
  - camera: `CallCameraCapture` → `FreeqAv.pushVideoFrame(bgra,w,h,tsUs)`
  - control: `setCameraEnabled`, `setMuted`, `isConnected`, `leave`, `new(...)`
- Remote media in via `AvEvent` → `AvEventHandler.onAvEvent`:
  - `audioTrackStarted/Stopped(nick)`, `videoTrackStarted/Stopped(nick)`,
    `videoFrame(nick,bgra,w,h)`
- UI: `CallView`, `VideoFeedView`, `MediaCapture`.
- Rust SDK handles MoQ/SFU publish+subscribe (freeq-av / freeq-sdk-ffi).

## Existing tests
- `freeqTests/AvSessionTests.swift` (1150 lines)
- `freeqTests/BufferRoutingTests.swift` (682 lines)

## Steps
- [x] Baseline build — found the test target wouldn't run:
      1. stale `FreeqSDK.xcframework` (bindings referenced AV FFI symbols the
         committed lib lacked) → rebuilt via `build-rust.sh` (device=AV,
         sim=AV-stub) so bindings + lib match.
      2. `IrcMessage` FFI type gained `reactions: [ReactionTally]`; 4 stale
         `BufferRoutingTests` call sites omitted it → patched with `reactions: []`.
- [x] Mapped coverage: AvSessionTests(50) = state machine + camera orientation;
      the media DATA PATH (mic resample/downmix, camera stride-pack, inbound
      BGRA→pixelbuffer render, capture→push wiring) was UNtested.
- [x] Testability extractions (minimal, behavior-preserving):
      - `CallMicCapture.convert(_:using:to:)` + `.targetFormat`
      - `CallCameraCapture.packTightlyPackedBGRA(base:width:height:rowBytes:)`
      - `VideoSampleBuffer.makePixelBuffer(...)` + `.readTightlyPacked(...)`
- [x] Wrote `freeqTests/AvMediaFlowTests.swift` — deep flow tests:
      audio resample 44.1k/16k/48k, stereo→mono downmix, silence; mic→push
      wiring; camera stride-pack; inbound pixel round-trip (incl. padded
      stride); enqueue valid/mismatch; camera→push wiring; out→in byte-integrity
      loopback; videoFrame event → render state.
- [ ] Run green (in progress)
- [ ] Commit (do not push — main push blocked by 142MB artifact in chad's commit)

## Note on scope
The simulator `FreeqAv` is an AV stub (openh264 can't target the sim), so real
on-device MoQ media flow is not unit-testable here — by the project's own design.
The automatable surface is the Swift data path + driver-seam wiring, which these
tests now cover deeply. Real network AV remains a device/manual concern.

## Notes
- Working on branch `ios-av-testing` (off local main; preserves chad's commits).
- Sim: iPhone 17 (FB44C77A-EC26-4783-BEED-39F44A815078), Xcode 26.2.
