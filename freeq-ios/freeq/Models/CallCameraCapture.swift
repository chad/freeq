import AVFoundation
import CoreVideo
import Foundation

/// Drives the iOS front camera and pumps BGRA frames to the Rust AV pipeline.
///
/// We do capture in Swift because iroh-live's AVFoundation camera backend is
/// still stubbed (see `rusty-capture/src/platform/apple/camera.rs`). The Rust
/// side exposes `push_video_frame(bgra, w, h, ts_us)` which feeds a
/// `PushVideoSource` plugged into the broadcast's H.264 encoder.
///
/// Lifecycle:
/// - `start()` configures the session, requests permission, kicks the camera
///   on, and begins delivering frames to `onFrame`.
/// - `stop()` tears the session down.
///
/// The capture session is configured for 1280×720, BGRA, 30fps. The preview
/// layer can be wired separately via `previewLayer` for a low-latency local
/// preview (which doesn't go through the Rust copy).
final class CallCameraCapture: NSObject {
    /// Callback fires on the capture queue. Implementations should be fast —
    /// the queue is serial and frames will queue up behind a slow consumer.
    var onFrame: ((_ bgra: UnsafePointer<UInt8>, _ length: Int, _ width: Int, _ height: Int, _ timestampMicros: UInt64) -> Void)?

    /// Preview layer for the local "self view" tile. Always shows the most
    /// recent frame the OS captured, regardless of whether the encoder is
    /// keeping up.
    let previewLayer: AVCaptureVideoPreviewLayer

    private let session: AVCaptureSession
    private let videoOutput: AVCaptureVideoDataOutput
    private let queue = DispatchQueue(label: "at.freeq.camera-capture", qos: .userInitiated)
    private var configured = false

    override init() {
        self.session = AVCaptureSession()
        self.videoOutput = AVCaptureVideoDataOutput()
        self.previewLayer = AVCaptureVideoPreviewLayer(session: session)
        self.previewLayer.videoGravity = .resizeAspectFill
        super.init()
    }

    /// Request camera permission and start delivering frames.
    /// Idempotent — calling again while running is a no-op.
    func start() {
        AVCaptureDevice.requestAccess(for: .video) { [weak self] granted in
            guard let self, granted else {
                print("[camera] permission denied")
                return
            }
            self.queue.async {
                self.configureIfNeeded()
                if !self.session.isRunning {
                    self.session.startRunning()
                }
            }
        }
    }

    func stop() {
        queue.async {
            if self.session.isRunning {
                self.session.stopRunning()
            }
        }
    }

    private func configureIfNeeded() {
        guard !configured else { return }
        session.beginConfiguration()
        defer { session.commitConfiguration() }

        if session.canSetSessionPreset(.hd1280x720) {
            session.sessionPreset = .hd1280x720
        }

        guard let device = AVCaptureDevice.default(.builtInWideAngleCamera, for: .video, position: .front)
                ?? AVCaptureDevice.default(for: .video) else {
            print("[camera] no capture device available")
            return
        }
        guard let input = try? AVCaptureDeviceInput(device: device) else {
            print("[camera] failed to create device input")
            return
        }
        if session.canAddInput(input) {
            session.addInput(input)
        }

        videoOutput.videoSettings = [
            kCVPixelBufferPixelFormatTypeKey as String: kCVPixelFormatType_32BGRA
        ]
        videoOutput.alwaysDiscardsLateVideoFrames = true
        videoOutput.setSampleBufferDelegate(self, queue: queue)
        if session.canAddOutput(videoOutput) {
            session.addOutput(videoOutput)
        }

        // Front camera is mirrored by convention; remote peers prefer the
        // unflipped feed so what they see matches what the camera saw.
        if let connection = videoOutput.connection(with: .video) {
            if connection.isVideoMirroringSupported {
                connection.automaticallyAdjustsVideoMirroring = false
                connection.isVideoMirrored = false
            }
            if connection.isVideoRotationAngleSupported(90) {
                connection.videoRotationAngle = 90
            }
        }

        configured = true
    }
}

extension CallCameraCapture: AVCaptureVideoDataOutputSampleBufferDelegate {
    func captureOutput(_ output: AVCaptureOutput,
                       didOutput sampleBuffer: CMSampleBuffer,
                       from connection: AVCaptureConnection) {
        guard let pixelBuffer = CMSampleBufferGetImageBuffer(sampleBuffer) else { return }
        guard let cb = onFrame else { return }

        CVPixelBufferLockBaseAddress(pixelBuffer, .readOnly)
        defer { CVPixelBufferUnlockBaseAddress(pixelBuffer, .readOnly) }

        let width = CVPixelBufferGetWidth(pixelBuffer)
        let height = CVPixelBufferGetHeight(pixelBuffer)
        let rowBytes = CVPixelBufferGetBytesPerRow(pixelBuffer)
        guard let base = CVPixelBufferGetBaseAddress(pixelBuffer) else { return }

        let ts = CMSampleBufferGetPresentationTimeStamp(sampleBuffer)
        let tsMicros = UInt64(ts.seconds * 1_000_000)

        // The Rust side expects tightly-packed BGRA. AVCaptureSession often
        // produces buffers where rowBytes > width*4 for alignment; copy
        // row-by-row to strip the padding.
        let expectedRow = width * 4
        if rowBytes == expectedRow {
            cb(base.assumingMemoryBound(to: UInt8.self), height * rowBytes, width, height, tsMicros)
        } else {
            var packed = [UInt8](repeating: 0, count: width * height * 4)
            packed.withUnsafeMutableBufferPointer { dst in
                for y in 0..<height {
                    let src = base.advanced(by: y * rowBytes).assumingMemoryBound(to: UInt8.self)
                    let dstRow = dst.baseAddress!.advanced(by: y * expectedRow)
                    dstRow.update(from: src, count: expectedRow)
                }
            }
            packed.withUnsafeBufferPointer { buf in
                cb(buf.baseAddress!, width * height * 4, width, height, tsMicros)
            }
        }
    }
}
