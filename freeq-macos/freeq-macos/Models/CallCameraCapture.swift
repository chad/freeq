import AVFoundation
import Foundation

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
