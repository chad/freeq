import AVFoundation
import Foundation

/// Drives the macOS microphone and pumps mono 48 kHz float samples to the
/// Rust AV pipeline via `pushAudioFrame`.
///
/// Capture is Swift-driven (the iroh-live audio input backend is stubbed on
/// Apple platforms — we capture from `AVAudioEngine` and push frames in).
/// Unlike iOS there is no `AVAudioSession` on macOS; the input node is used
/// directly. `start()` requests mic permission and begins delivering samples
/// to `onSamples`; `stop()` tears the engine down. Audio is always-on for a
/// call, so this runs the whole call.
final class CallMicCapture {
    /// Fires on the audio engine's render thread — keep the handler fast.
    var onSamples: (([Float]) -> Void)?

    private let engine = AVAudioEngine()
    private var converter: AVAudioConverter?
    private var running = false
    private var deliverCount = 0

    /// What the Rust `PushAudioSource` expects: mono 48 kHz float32.
    private let target = AVAudioFormat(
        commonFormat: .pcmFormatFloat32,
        sampleRate: 48_000,
        channels: 1,
        interleaved: false
    )!

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

        let input = engine.inputNode
        let inFormat = input.outputFormat(forBus: 0)
        print("[mic] input \(inFormat.sampleRate)Hz x\(inFormat.channelCount)ch")
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

    /// Convert one captured buffer to mono 48 kHz and hand it to `onSamples`.
    private func deliver(_ buffer: AVAudioPCMBuffer) {
        guard let converter, let onSamples else { return }
        let ratio = target.sampleRate / buffer.format.sampleRate
        let capacity = AVAudioFrameCount(Double(buffer.frameLength) * ratio) + 1024
        guard let out = AVAudioPCMBuffer(pcmFormat: target, frameCapacity: capacity) else { return }

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
            return
        }
        let frames = Int(out.frameLength)
        guard frames > 0, let channel = out.floatChannelData else { return }
        let samples = Array(UnsafeBufferPointer(start: channel[0], count: frames))

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
