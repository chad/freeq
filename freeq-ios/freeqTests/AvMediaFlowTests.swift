import XCTest
import AVFoundation
import CoreMedia
import CoreVideo
@testable import freeq

/// Deep, automated tests for the AV **media data path** — the bytes that
/// actually carry audio and video through a call. `AvSessionTests` covers the
/// signaling/state machine and camera orientation; these cover what was
/// untested: that captured audio is correctly resampled/downmixed to the
/// format the Rust broadcast expects, that captured + received video pixels
/// survive the (de)packing with row-stride padding intact, and that the
/// capture closures are actually wired to the broadcast driver.
///
/// All of this runs on a Simulator destination with no microphone, camera, or
/// MoQ network: the conversions are pure AVFoundation/CoreMedia DSP, and the
/// broadcast side is a `RecordingAvDriver` substituted via `avSessionFactory`.
/// (The simulator `FreeqAv` is an AV stub anyway — real on-device network
/// media flow is out of scope for an automated unit test by construction.)
final class AvMediaFlowTests: XCTestCase {

    // MARK: - Fakes / harness

    /// AvSessionDriver that records the exact audio sample arrays and video
    /// frames the production capture path pushes at it.
    final class RecordingAvDriver: AvSessionDriver {
        var pushedAudio: [[Float]] = []
        var pushedVideo: [(bgra: [UInt8], width: UInt32, height: UInt32, ts: UInt64)] = []
        var muteCalls: [Bool] = []
        var cameraCalls: [Bool] = []
        var leaveCalls = 0
        var connected = true

        func setMuted(muted: Bool) { muteCalls.append(muted) }
        func setCameraEnabled(enabled: Bool) throws { cameraCalls.append(enabled) }
        func pushVideoFrame(bgra: [UInt8], width: UInt32, height: UInt32, timestampUs: UInt64) {
            pushedVideo.append((bgra, width, height, timestampUs))
        }
        func pushAudioFrame(samples: [Float]) { pushedAudio.append(samples) }
        func leave() { leaveCalls += 1; connected = false }
        func isConnected() -> Bool { connected }
    }

    private func makeState(myNick: String = "alice") -> (AppState, () -> RecordingAvDriver?) {
        for k in ["freeq.nick", "freeq.server", "freeq.channels", "freeq.readPositions",
                  "freeq.unreadCounts", "freeq.mutedChannels"] {
            UserDefaults.standard.removeObject(forKey: k)
        }
        BufferCacheStore.clear()
        let state = AppState()
        state.nick = myNick
        state.rawSenderForTest = { _ in }
        var last: RecordingAvDriver? = nil
        state.avSessionFactory = { _, _, _, _, _ in
            let d = RecordingAvDriver()
            last = d
            return d
        }
        return (state, { last })
    }

    /// Build a mono/stereo float PCM buffer filled with a sine wave so the
    /// resampler has real signal to preserve (silence would hide bugs).
    private func makeSine(
        sampleRate: Double,
        channels: AVAudioChannelCount,
        frames: AVAudioFrameCount,
        freq: Double = 440,
        amplitude: Float = 0.5
    ) -> AVAudioPCMBuffer {
        let fmt = AVAudioFormat(
            commonFormat: .pcmFormatFloat32,
            sampleRate: sampleRate,
            channels: channels,
            interleaved: false
        )!
        let buf = AVAudioPCMBuffer(pcmFormat: fmt, frameCapacity: frames)!
        buf.frameLength = frames
        for ch in 0..<Int(channels) {
            let p = buf.floatChannelData![ch]
            for i in 0..<Int(frames) {
                p[i] = amplitude * Float(sin(2.0 * Double.pi * freq * Double(i) / sampleRate))
            }
        }
        return buf
    }

    private func peak(_ s: [Float]) -> Float { s.reduce(0) { Swift.max($0, abs($1)) } }

    private func converter(from sr: Double, channels: AVAudioChannelCount) -> AVAudioConverter {
        let inFmt = AVAudioFormat(
            commonFormat: .pcmFormatFloat32, sampleRate: sr, channels: channels, interleaved: false
        )!
        return AVAudioConverter(from: inFmt, to: CallMicCapture.targetFormat)!
    }

    // MARK: - 1. Outbound audio: capture → mono 48 kHz resample/downmix

    func testMicConvertDownsamples44100StereoToMono48k() {
        // 44.1k→48k is a NON-integer ratio, so AVAudioConverter has resampler
        // priming latency: a single one-shot convert under-produces, and the
        // deficit is made up on subsequent calls. Production reuses ONE
        // converter across the continuous capture stream, so test that way —
        // drive many buffers through one converter and assert the aggregate
        // output converges to the ideal rate-converted count.
        let conv = converter(from: 44_100, channels: 2)
        let buffers = 20                          // 20 × 100 ms = 2 s
        var total = 0
        var maxPeak: Float = 0
        var allFinite = true
        for _ in 0..<buffers {
            let buf = makeSine(sampleRate: 44_100, channels: 2, frames: 4_410)
            let out = try! XCTUnwrap(CallMicCapture.convert(buf, using: conv, to: CallMicCapture.targetFormat),
                                     "each conversion must succeed")
            total += out.count
            maxPeak = Swift.max(maxPeak, peak(out))
            if !out.allSatisfy({ $0.isFinite }) { allFinite = false }
        }
        // 2 s of 44.1k → 48k ≈ 96000 mono samples. One-time priming latency
        // (~hundreds of samples) is amortized across the stream.
        XCTAssertEqual(Double(total), 96_000, accuracy: 600,
                       "2s of 44.1k→48k should stream ~96000 mono samples, got \(total)")
        XCTAssertTrue(allFinite, "no NaN/Inf may reach the broadcast")
        XCTAssertGreaterThan(maxPeak, 0.3, "the sine signal must survive resampling (peak≈0.5)")
        XCTAssertLessThan(maxPeak, 0.75, "downmix/resample must not blow up amplitude")
    }

    func testMicConvertUpsamples16kMonoTo48k() {
        let buf = makeSine(sampleRate: 16_000, channels: 1, frames: 1_600)  // 100 ms
        let out = try! XCTUnwrap(CallMicCapture.convert(buf, using: converter(from: 16_000, channels: 1),
                                                        to: CallMicCapture.targetFormat))
        XCTAssertEqual(Double(out.count), 4_800, accuracy: 64,
                       "16k→48k of 100ms should yield ~4800 samples (3x), got \(out.count)")
        XCTAssertGreaterThan(peak(out), 0.3)
    }

    func testMicConvert48kMonoIsLengthPreservingPassthrough() {
        let buf = makeSine(sampleRate: 48_000, channels: 1, frames: 2_400)
        let out = try! XCTUnwrap(CallMicCapture.convert(buf, using: converter(from: 48_000, channels: 1),
                                                        to: CallMicCapture.targetFormat))
        XCTAssertEqual(Double(out.count), 2_400, accuracy: 8,
                       "same-rate mono conversion must preserve length")
        XCTAssertGreaterThan(peak(out), 0.3)
    }

    func testMicConvertSilenceStaysSilent() {
        let fmt = AVAudioFormat(commonFormat: .pcmFormatFloat32, sampleRate: 44_100, channels: 1, interleaved: false)!
        let buf = AVAudioPCMBuffer(pcmFormat: fmt, frameCapacity: 2_205)!
        buf.frameLength = 2_205  // zero-filled
        let out = try! XCTUnwrap(CallMicCapture.convert(buf, using: converter(from: 44_100, channels: 1),
                                                        to: CallMicCapture.targetFormat))
        XCTAssertEqual(peak(out), 0, accuracy: 1e-4, "silence in must be silence out")
    }

    func testMicConvertStereoDownmixPreservesAmplitude() {
        // Identical L/R sine → mono average is the same sine (amplitude kept),
        // proving the downmix isn't summing-to-clip or dropping a channel.
        let buf = makeSine(sampleRate: 48_000, channels: 2, frames: 4_800, amplitude: 0.6)
        let out = try! XCTUnwrap(CallMicCapture.convert(buf, using: converter(from: 48_000, channels: 2),
                                                        to: CallMicCapture.targetFormat))
        XCTAssertEqual(peak(out), 0.6, accuracy: 0.08, "L==R downmix should preserve amplitude ~0.6")
    }

    // MARK: - 2. Outbound audio wiring: mic samples reach the broadcast driver

    func testMicSamplesAreForwardedToDriverPushAudioFrame() {
        let (state, driverRef) = makeState()
        state.startCall(channel: "#freeq", sessionId: "s1")
        let driver = try! XCTUnwrap(driverRef())
        let cap = try! XCTUnwrap(state.micCapture, "starting a call must create always-on mic capture")

        let frame: [Float] = [0.1, -0.2, 0.3, -0.4]
        cap.onSamples?(frame)

        XCTAssertEqual(driver.pushedAudio.count, 1, "mic samples must be pushed to the broadcast exactly once")
        XCTAssertEqual(driver.pushedAudio.first, frame, "the exact samples must reach pushAudioFrame")
    }

    // MARK: - 3. Outbound video: camera stride-pack strips row padding

    func testPackTightlyPackedBGRAStripsRowPadding() {
        // Simulate a capture buffer whose rows are padded to a larger stride
        // (rowBytes > width*4) — the common CVPixelBuffer alignment case.
        let width = 3, height = 2
        let expectedRow = width * 4          // 12
        let rowBytes = 64                    // padded stride
        var src = [UInt8](repeating: 0xEE, count: rowBytes * height)  // 0xEE = padding marker
        // Known pixel content per row.
        for y in 0..<height {
            for x in 0..<expectedRow {
                src[y * rowBytes + x] = UInt8((y * expectedRow + x) & 0xFF)
            }
        }
        let packed = src.withUnsafeBytes { raw -> [UInt8] in
            CallCameraCapture.packTightlyPackedBGRA(
                base: raw.baseAddress!, width: width, height: height, rowBytes: rowBytes)
        }
        XCTAssertEqual(packed.count, width * height * 4)
        XCTAssertFalse(packed.contains(0xEE), "row padding must be stripped, not copied into the frame")
        for y in 0..<height {
            for x in 0..<expectedRow {
                XCTAssertEqual(packed[y * expectedRow + x], UInt8((y * expectedRow + x) & 0xFF),
                               "pixel (\(x),\(y)) must survive the de-stride")
            }
        }
    }

    func testPackTightlyPackedBGRAIdentityWhenNoPadding() {
        let width = 2, height = 2
        let src: [UInt8] = (0..<(width * height * 4)).map { UInt8($0) }
        let packed = src.withUnsafeBytes { raw -> [UInt8] in
            CallCameraCapture.packTightlyPackedBGRA(
                base: raw.baseAddress!, width: width, height: height, rowBytes: width * 4)
        }
        XCTAssertEqual(packed, src, "no padding → identity copy")
    }

    // MARK: - 4. Inbound video: BGRA bytes survive the pixel-buffer round-trip

    func testInboundPixelBufferRoundTripsBytesEvenWidth() {
        let width = 4, height = 2
        let bgra: [UInt8] = (0..<(width * height * 4)).map { UInt8($0 & 0xFF) }
        let pb = try! XCTUnwrap(VideoSampleBuffer.makePixelBuffer(bgra: bgra, width: width, height: height))
        XCTAssertEqual(CVPixelBufferGetWidth(pb), width)
        XCTAssertEqual(CVPixelBufferGetHeight(pb), height)
        XCTAssertEqual(CVPixelBufferGetPixelFormatType(pb), kCVPixelFormatType_32BGRA)
        XCTAssertEqual(VideoSampleBuffer.readTightlyPacked(pb), bgra, "inbound pixels must round-trip exactly")
    }

    func testInboundPixelBufferRoundTripsBytesWithStridePadding() {
        // width=3 forces CVPixelBuffer to pad each row beyond width*4; the
        // round-trip must still reproduce the exact tightly-packed input.
        let width = 3, height = 5
        let bgra: [UInt8] = (0..<(width * height * 4)).map { UInt8(($0 * 7) & 0xFF) }
        let pb = try! XCTUnwrap(VideoSampleBuffer.makePixelBuffer(bgra: bgra, width: width, height: height))
        XCTAssertGreaterThanOrEqual(CVPixelBufferGetBytesPerRow(pb), width * 4)
        XCTAssertEqual(VideoSampleBuffer.readTightlyPacked(pb), bgra,
                       "padded-stride inbound frame must still reproduce the source bytes")
    }

    func testInboundPixelBufferRejectsSizeMismatch() {
        XCTAssertNil(VideoSampleBuffer.makePixelBuffer(bgra: [0, 0, 0], width: 4, height: 2),
                     "a too-small buffer must be rejected, not read out of bounds")
        XCTAssertNil(VideoSampleBuffer.makePixelBuffer(bgra: [], width: 0, height: 0),
                     "zero dimensions must be rejected")
    }

    func testEnqueueValidFrameSucceedsAndMismatchFails() {
        let layer = AVSampleBufferDisplayLayer()
        let w = 8, h = 8
        let good = [UInt8](repeating: 128, count: w * h * 4)
        XCTAssertTrue(VideoSampleBuffer.enqueue(bgra: good, width: w, height: h, on: layer),
                      "a correctly-sized BGRA frame must enqueue for display")
        XCTAssertFalse(VideoSampleBuffer.enqueue(bgra: [1, 2, 3], width: w, height: h, on: layer),
                       "a size-mismatched frame must be rejected without crashing")
    }

    // MARK: - 5. Outbound video wiring: camera frames reach the driver intact

    func testCameraFramesAreForwardedToDriverPushVideoFrame() {
        let (state, driverRef) = makeState()
        state.startCall(channel: "#freeq", sessionId: "s1")
        let driver = try! XCTUnwrap(driverRef())
        state.toggleCamera()
        let cap = try! XCTUnwrap(state.cameraCapture)

        let w = 2, h = 2
        let bytes: [UInt8] = (0..<(w * h * 4)).map { UInt8($0) }
        bytes.withUnsafeBufferPointer { buf in
            cap.onFrame?(buf.baseAddress!, buf.count, w, h, 123_456)
        }

        XCTAssertEqual(driver.pushedVideo.count, 1, "a captured frame must be pushed to the broadcast")
        let f = driver.pushedVideo[0]
        XCTAssertEqual(f.bgra, bytes, "frame bytes must reach pushVideoFrame intact")
        XCTAssertEqual(f.width, UInt32(w))
        XCTAssertEqual(f.height, UInt32(h))
        XCTAssertEqual(f.ts, 123_456, "presentation timestamp must be forwarded")
    }

    // MARK: - 6. End-to-end loopback: bytes pushed out reproduce on the render path

    func testVideoFrameByteIntegrityAcrossCaptureAndRender() {
        let (state, driverRef) = makeState()
        state.startCall(channel: "#freeq", sessionId: "s1")
        let driver = try! XCTUnwrap(driverRef())
        state.toggleCamera()
        let cap = try! XCTUnwrap(state.cameraCapture)

        // Capture side: a known frame goes out through the driver.
        let w = 4, h = 4
        let original: [UInt8] = (0..<(w * h * 4)).map { UInt8(($0 * 3) & 0xFF) }
        original.withUnsafeBufferPointer { cap.onFrame?($0.baseAddress!, $0.count, w, h, 0) }
        let sent = try! XCTUnwrap(driver.pushedVideo.first)

        // Render side: the same bytes, received as a remote frame, must
        // reproduce exactly through the inbound pixel-buffer path.
        let pb = try! XCTUnwrap(VideoSampleBuffer.makePixelBuffer(bgra: sent.bgra,
                                                                  width: Int(sent.width),
                                                                  height: Int(sent.height)))
        XCTAssertEqual(VideoSampleBuffer.readTightlyPacked(pb), original,
                       "a frame's pixels must survive capture-pack → broadcast → inbound-render unchanged")
    }

    // MARK: - 7. Inbound video event drives the render state

    func testVideoFrameEventMarksParticipantAndRenders() {
        let (state, _) = makeState(myNick: "alice")
        state.startCall(channel: "#freeq", sessionId: "s1")
        state.deliverAvEventForTest(.participantJoined(nick: "bob"))
        let layer = AVSampleBufferDisplayLayer()
        state.bindVideoSink(nick: "bob", to: layer)

        let w = 8, h = 8
        let bgra = [UInt8](repeating: 200, count: w * h * 4)
        state.deliverAvEventForTest(.videoFrame(nick: "bob", bgra: bgra,
                                                width: UInt32(w), height: UInt32(h)))

        XCTAssertTrue(state.participantsWithVideo.contains("bob"),
                      "a received video frame must mark the participant as having video")
    }
}
