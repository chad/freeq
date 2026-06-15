import AVFoundation
import Foundation
import ScreenCaptureKit

/// Drives the macOS camera via `AVCaptureSession` and pumps tightly-packed
/// BGRA frames to the Rust AV pipeline via `pushVideoFrame`.
///
/// Capture is Swift-driven because iroh-live's `capture` backend is stubbed on
/// Apple platforms (the camera is owned by AVFoundation here, not Rust). A
/// low-latency `AVCaptureVideoPreviewLayer` is exposed for the local self-view
/// so the local tile never freezes if the encoder stalls.
final class CallCameraCapture: NSObject {
    /// Fires on the capture queue with a tightly-packed BGRA frame. Keep fast.
    /// (pointer, byteLength, width, height, timestampUs)
    var onFrame: ((UnsafePointer<UInt8>, Int, Int, Int, UInt64) -> Void)?

    let session = AVCaptureSession()
    let previewLayer: AVCaptureVideoPreviewLayer

    private let output = AVCaptureVideoDataOutput()
    private let queue = DispatchQueue(label: "at.freeq.macos.camera")
    private var configured = false

    override init() {
        previewLayer = AVCaptureVideoPreviewLayer(session: session)
        previewLayer.videoGravity = .resizeAspectFill
        super.init()
    }

    /// Configure (once) and start the capture session. Idempotent.
    func start() {
        AVCaptureDevice.requestAccess(for: .video) { [weak self] granted in
            guard let self else { return }
            guard granted else {
                print("[cam] permission denied")
                return
            }
            self.queue.async { self.configureAndRun() }
        }
    }

    private func configureAndRun() {
        if !configured {
            session.beginConfiguration()
            session.sessionPreset = .vga640x480

            guard let device = AVCaptureDevice.default(.builtInWideAngleCamera, for: .video, position: .front)
                ?? AVCaptureDevice.default(for: .video),
                  let input = try? AVCaptureDeviceInput(device: device),
                  session.canAddInput(input) else {
                print("[cam] no usable camera input")
                session.commitConfiguration()
                return
            }
            session.addInput(input)

            output.videoSettings = [
                kCVPixelBufferPixelFormatTypeKey as String: Int(kCVPixelFormatType_32BGRA)
            ]
            output.alwaysDiscardsLateVideoFrames = true
            output.setSampleBufferDelegate(self, queue: queue)
            if session.canAddOutput(output) {
                session.addOutput(output)
            }
            session.commitConfiguration()
            configured = true
        }
        if !session.isRunning {
            session.startRunning()
            print("[cam] capture started")
        }
    }

    /// Stop the capture session. Idempotent.
    func stop() {
        queue.async { [weak self] in
            guard let self, self.session.isRunning else { return }
            self.session.stopRunning()
            print("[cam] capture stopped")
        }
    }
}

/// Captures the primary display through ScreenCaptureKit and emits tightly-
/// packed BGRA frames through the same closure shape as `CallCameraCapture`.
/// This lets macOS screen sharing use the existing native AV video path while
/// the SDK grows a dedicated `/screen` broadcast.
@available(macOS 12.3, *)
final class CallScreenCapture: NSObject {
    /// Fires on ScreenCaptureKit's sample queue with a tightly-packed BGRA frame.
    /// (pointer, byteLength, width, height, timestampUs)
    var onFrame: ((UnsafePointer<UInt8>, Int, Int, Int, UInt64) -> Void)?
    var onStopped: (() -> Void)?

    private let queue = DispatchQueue(label: "at.freeq.macos.screen")
    private var stream: SCStream?
    private var displaySize: CGSize = .zero

    func start() {
        Task { [weak self] in
            await self?.startAsync()
        }
    }

    @MainActor
    private func startAsync() async {
        do {
            let content = try await SCShareableContent.excludingDesktopWindows(
                false,
                onScreenWindowsOnly: true
            )
            guard let display = content.displays.first else {
                print("[screen] no shareable display")
                return
            }
            displaySize = CGSize(width: display.width, height: display.height)

            let filter = SCContentFilter(display: display, excludingWindows: [])
            let config = SCStreamConfiguration()
            config.width = max(2, min(display.width, 1920))
            config.height = max(2, min(display.height, 1080))
            config.pixelFormat = kCVPixelFormatType_32BGRA
            config.minimumFrameInterval = CMTime(value: 1, timescale: 15)
            config.queueDepth = 4
            config.showsCursor = true

            let stream = SCStream(filter: filter, configuration: config, delegate: self)
            try stream.addStreamOutput(self, type: .screen, sampleHandlerQueue: queue)
            self.stream = stream
            try await stream.startCapture()
            print("[screen] capture started")
        } catch {
            print("[screen] capture failed: \(error)")
            onStopped?()
        }
    }

    func stop() {
        guard let stream else { return }
        self.stream = nil
        Task {
            do {
                try await stream.stopCapture()
                print("[screen] capture stopped")
            } catch {
                print("[screen] stop failed: \(error)")
            }
        }
    }
}

@available(macOS 12.3, *)
extension CallScreenCapture: SCStreamOutput, SCStreamDelegate {
    func stream(
        _ stream: SCStream,
        didOutputSampleBuffer sampleBuffer: CMSampleBuffer,
        of type: SCStreamOutputType
    ) {
        guard type == .screen,
              sampleBuffer.isValid,
              let onFrame,
              let pb = CMSampleBufferGetImageBuffer(sampleBuffer) else { return }

        CVPixelBufferLockBaseAddress(pb, .readOnly)
        defer { CVPixelBufferUnlockBaseAddress(pb, .readOnly) }

        let width = CVPixelBufferGetWidth(pb)
        let height = CVPixelBufferGetHeight(pb)
        let rowBytes = CVPixelBufferGetBytesPerRow(pb)
        guard let base = CVPixelBufferGetBaseAddress(pb) else { return }

        let expectedRow = width * 4
        let pts = CMSampleBufferGetPresentationTimeStamp(sampleBuffer)
        let tsUs = pts.isValid
            ? UInt64(max(0, CMTimeGetSeconds(pts)) * 1_000_000)
            : UInt64(Date().timeIntervalSince1970 * 1_000_000)

        var packed = [UInt8](repeating: 0, count: width * height * 4)
        packed.withUnsafeMutableBytes { dst in
            guard let dstBase = dst.baseAddress else { return }
            if rowBytes == expectedRow {
                memcpy(dstBase, base, width * height * 4)
            } else {
                for y in 0..<height {
                    let srcRow = base.advanced(by: y * rowBytes)
                    let dstRow = dstBase.advanced(by: y * expectedRow)
                    memcpy(dstRow, srcRow, expectedRow)
                }
            }
        }
        packed.withUnsafeBufferPointer { buf in
            guard let base = buf.baseAddress else { return }
            onFrame(base, buf.count, width, height, tsUs)
        }
    }

    func stream(_ stream: SCStream, didStopWithError error: Error) {
        print("[screen] capture stopped with error: \(error)")
        DispatchQueue.main.async { [weak self] in
            self?.onStopped?()
        }
    }
}

extension CallCameraCapture: AVCaptureVideoDataOutputSampleBufferDelegate {
    func captureOutput(
        _ output: AVCaptureOutput,
        didOutput sampleBuffer: CMSampleBuffer,
        from connection: AVCaptureConnection
    ) {
        guard let onFrame, let pb = CMSampleBufferGetImageBuffer(sampleBuffer) else { return }

        CVPixelBufferLockBaseAddress(pb, .readOnly)
        defer { CVPixelBufferUnlockBaseAddress(pb, .readOnly) }

        let width = CVPixelBufferGetWidth(pb)
        let height = CVPixelBufferGetHeight(pb)
        let rowBytes = CVPixelBufferGetBytesPerRow(pb)
        guard let base = CVPixelBufferGetBaseAddress(pb) else { return }

        let expectedRow = width * 4
        let tsUs = UInt64(CMTimeGetSeconds(CMSampleBufferGetPresentationTimeStamp(sampleBuffer)) * 1_000_000)

        // Tightly pack into width*height*4 (the SDK expects no row padding).
        var packed = [UInt8](repeating: 0, count: width * height * 4)
        packed.withUnsafeMutableBytes { dst in
            let dstBase = dst.baseAddress!
            if rowBytes == expectedRow {
                memcpy(dstBase, base, width * height * 4)
            } else {
                for y in 0..<height {
                    let srcRow = base.advanced(by: y * rowBytes)
                    let dstRow = dstBase.advanced(by: y * expectedRow)
                    memcpy(dstRow, srcRow, expectedRow)
                }
            }
        }
        packed.withUnsafeBufferPointer { buf in
            onFrame(buf.baseAddress!, buf.count, width, height, tsUs)
        }
    }
}
