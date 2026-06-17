import AVFoundation
import Foundation

/// Drives the iOS microphone and pumps mono 48 kHz float samples to the
/// Rust AV pipeline via `push_audio_frame`.
///
/// Capture is Swift-driven for the same reason video is (see
/// `CallCameraCapture`): iroh-live's audio *input* backend isn't viable on
/// iOS. Without this the iOS broadcast carries silence and nobody — Eliza
/// included — can hear the user.
///
/// Lifecycle: `start()` requests mic permission and begins delivering
/// samples to `onSamples`; `stop()` tears the engine down. Audio is
/// always-on for a call, so this runs for the whole call.
final class CallMicCapture {
    /// Fires on the audio engine's render thread — keep the handler fast.
    var onSamples: (([Float]) -> Void)?

    private let engine = AVAudioEngine()
    private var converter: AVAudioConverter?
    private var running = false
    private var deliverCount = 0

    /// What the Rust `PushAudioSource` expects: mono 48 kHz float32.
    /// Exposed (static) so tests can build a converter against the exact
    /// target the live path uses.
    static let targetFormat = AVAudioFormat(
        commonFormat: .pcmFormatFloat32,
        sampleRate: 48_000,
        channels: 1,
        interleaved: false
    )!

    private let target = CallMicCapture.targetFormat

    /// Request microphone permission, then start capture. Idempotent.
    func start() {
        AVCaptureDevice.requestAccess(for: .audio) { [weak self] granted in
            guard let self else { return }
            guard granted else {
                print("[mic] permission denied — others won't hear you")
                return
            }
            DispatchQueue.main.async { self.begin() }
        }
    }

    private func begin() {
        guard !running else { return }

        // Re-assert a record-capable audio session. Anything that ran
        // since the call started (iroh-live's playback backend, etc.) may
        // have left the session playback-only — which gives AVAudioEngine
        // a dead input route, so captured buffers come back silent.
        let session = AVAudioSession.sharedInstance()
        do {
            try session.setCategory(
                .playAndRecord,
                mode: .voiceChat,
                options: [.defaultToSpeaker, .allowBluetooth]
            )
            try session.setActive(true)
            // Route to the loud speaker by default — `.voiceChat` mode
            // otherwise sends call audio to the quiet handset receiver.
            try session.overrideOutputAudioPort(.speaker)
        } catch {
            print("[mic] audio session setup failed: \(error)")
        }

        let input = engine.inputNode
        let inFormat = input.outputFormat(forBus: 0)
        print("[mic] input \(inFormat.sampleRate)Hz x\(inFormat.channelCount)ch "
            + "(session input available: \(session.isInputAvailable))")
        guard inFormat.sampleRate > 0, inFormat.channelCount > 0 else {
            print("[mic] input route unavailable — capture aborted")
            return
        }

        converter = AVAudioConverter(from: inFormat, to: target)
        input.installTap(onBus: 0, bufferSize: 2048, format: inFormat) { [weak self] buffer, _ in
            self?.deliver(buffer)
        }
        do {
            engine.prepare()
            try engine.start()
            running = true
            print("[mic] capture started")
        } catch {
            print("[mic] engine start failed: \(error)")
            input.removeTap(onBus: 0)
        }
    }

    /// Pure conversion of one captured PCM buffer to the target format
    /// (mono 48 kHz float32) using the supplied converter. Extracted from
    /// `deliver` so it's testable without an `AVAudioEngine`/microphone:
    /// a test builds a converter from any input format to
    /// [`targetFormat`] and feeds synthetic buffers. Returns the converted
    /// mono samples, or `nil` on a converter error.
    static func convert(
        _ buffer: AVAudioPCMBuffer,
        using converter: AVAudioConverter,
        to target: AVAudioFormat
    ) -> [Float]? {
        let ratio = target.sampleRate / buffer.format.sampleRate
        let capacity = AVAudioFrameCount(Double(buffer.frameLength) * ratio) + 1024
        guard let out = AVAudioPCMBuffer(pcmFormat: target, frameCapacity: capacity) else { return nil }

        var supplied = false
        var convErr: NSError?
        let status = converter.convert(to: out, error: &convErr) { _, outStatus in
            if supplied {
                outStatus.pointee = .noDataNow
                return nil
            }
            supplied = true
            outStatus.pointee = .haveData
            return buffer
        }
        if status == .error {
            print("[mic] convert error: \(convErr?.localizedDescription ?? "unknown")")
            return nil
        }
        let frames = Int(out.frameLength)
        guard frames > 0, let channel = out.floatChannelData else { return [] }
        return Array(UnsafeBufferPointer(start: channel[0], count: frames))
    }

    /// Convert one captured buffer to mono 48 kHz and hand it to `onSamples`.
    private func deliver(_ buffer: AVAudioPCMBuffer) {
        guard let converter, let onSamples else { return }
        guard let samples = CallMicCapture.convert(buffer, using: converter, to: target),
              !samples.isEmpty else { return }
        let frames = samples.count

        // Heartbeat: the peak amplitude tells us whether real audio is
        // being captured (peak > 0) or the input route is dead (peak ~ 0).
        deliverCount += 1
        if deliverCount == 1 || deliverCount % 100 == 0 {
            let peak = samples.reduce(Float(0)) { Swift.max($0, abs($1)) }
            print("[mic] delivered #\(deliverCount): \(frames) samples, peak "
                + String(format: "%.4f", peak))
        }
        onSamples(samples)
    }

    /// Stop capture and release the engine. Idempotent.
    func stop() {
        guard running else { return }
        engine.inputNode.removeTap(onBus: 0)
        engine.stop()
        converter = nil
        running = false
        deliverCount = 0
        print("[mic] capture stopped")
    }
}
