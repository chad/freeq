import AVFoundation
import CoreVideo
import Foundation
import UIKit

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

    /// Cached so the orientation handler doesn't have to look it back up.
    /// Default reflects the configure-time angle (90° portrait preview, 0°
    /// data output). The data-output value is locked at this initial value
    /// across orientation changes — see the long comment in
    /// `configureIfNeeded`.
    private var initialDataOutputAngle: CGFloat = 0
    /// Tracks the last orientation we accepted as "meaningful". Initialised
    /// to portrait but updated to the current device orientation in
    /// `configureIfNeeded` so a call that starts in landscape doesn't snap
    /// to portrait for the first frame.
    private var lastValidOrientation: UIDeviceOrientation = .portrait
    /// Pinned-in-memory copy of the rotation angle that *would* be applied
    /// to the preview, regardless of whether an
    /// `AVCaptureVideoPreviewLayer.connection` currently exists. The
    /// connection is nil before `session.startRunning` builds it, but the
    /// orientation observer can still fire — we cache the angle here and
    /// apply it via affine transform as a fallback. Tests read this to pin
    /// the connection-less path.
    private(set) var pendingPreviewAngle: CGFloat = 90
    /// Provider for the "current device orientation" — overridable in tests
    /// so we don't depend on `UIDevice.current.orientation`, which returns
    /// `.unknown` in the simulator/unit-test environment.
    fileprivate var orientationProvider: () -> UIDeviceOrientation = {
        UIDevice.current.orientation
    }

    override init() {
        self.session = AVCaptureSession()
        self.videoOutput = AVCaptureVideoDataOutput()
        self.previewLayer = AVCaptureVideoPreviewLayer(session: session)
        self.previewLayer.videoGravity = .resizeAspectFill
        super.init()
        // `orientationDidChangeNotification` is silent until *something* turns
        // on the accelerometer-backed orientation source. Apps that don't
        // care don't get the cost. We care.
        if Thread.isMainThread {
            UIDevice.current.beginGeneratingDeviceOrientationNotifications()
        } else {
            DispatchQueue.main.async {
                UIDevice.current.beginGeneratingDeviceOrientationNotifications()
            }
        }
        NotificationCenter.default.addObserver(
            self,
            selector: #selector(deviceOrientationDidChange),
            name: UIDevice.orientationDidChangeNotification,
            object: nil
        )
    }

    deinit {
        NotificationCenter.default.removeObserver(self)
        DispatchQueue.main.async {
            UIDevice.current.endGeneratingDeviceOrientationNotifications()
        }
    }

    @objc private func deviceOrientationDidChange() {
        applyOrientationOnMain(orientationProvider())
    }

    /// Hop to main if we aren't already there. `AVCaptureConnection` and
    /// `CALayer` mutations both require the main thread; notifications can
    /// fire on any thread; `configureIfNeeded` runs on the capture queue.
    private func applyOrientationOnMain(_ orientation: UIDeviceOrientation) {
        if Thread.isMainThread {
            applyOrientation(orientation)
        } else {
            DispatchQueue.main.async { self.applyOrientation(orientation) }
        }
    }

    /// Rotate only the local preview connection. The data-output connection
    /// (encoder feed) is fed by `captureOutput` which software-rotates the
    /// buffer to keep receivers seeing the user upright — see the rotation
    /// matrix and `rotatedFrame(_:width:height:for:)`.
    ///
    /// ## Angle matrix
    ///
    /// The app's UI is portrait-locked (Info.plist declares no
    /// `UISupportedInterfaceOrientations`, so SwiftUI never re-lays-out the
    /// preview tile when the device rotates). That means the preview rect's
    /// own "up" direction is always the device's "up" direction — but the
    /// device's "up" direction in the user's visual field changes as they
    /// tilt the phone.
    ///
    /// We want the user to see their own head at the top of *their visual
    /// field* (i.e. anchored to gravity), regardless of how the phone is
    /// tilted. Apple's canonical mapping (portrait=90, landscapeLeft=0,
    /// landscapeRight=180, portraitUpsideDown=270) is for a UI that
    /// rotates with the device. Ours doesn't.
    ///
    /// Derivation: in raw sensor coords, the user's head sits at the
    /// buffer's left edge when the phone is held in portrait. As the
    /// phone rotates CW by θ, the user (stationary in the world) rotates
    /// CCW by θ in the buffer. Composing that with the rotation needed
    /// to land the head at the rect position that maps to gravity-up
    /// gives:
    ///
    ///   - .portrait              → 90° CW
    ///   - .portraitUpsideDown    → 90° CW
    ///   - .landscapeLeft         → 270° CW
    ///   - .landscapeRight        → 270° CW
    ///
    /// The user-reported bug ("upside-down in landscape") was caused by
    /// shipping Apple's standard rotating-UI mapping in a portrait-locked
    /// app: in landscapeLeft we were applying 0° (the right answer if the
    /// rect rotates with the device), which left the user 90° off from
    /// gravity-up.
    /// Same as `previewAngleOrNil` but with a defaulted return for
    /// `faceUp`/`faceDown`/`unknown` so callers that want a guaranteed
    /// angle (the configure-time seed) get one.
    private func previewAngle(for orientation: UIDeviceOrientation) -> CGFloat {
        previewAngleOrNil(for: orientation) ?? 90
    }

    /// Returns the angle, in CW degrees, the preview connection should
    /// be set to for the given device orientation in a portrait-locked
    /// UI. Returns nil for `faceUp`/`faceDown`/`unknown` so callers can
    /// decide whether to leave the prior orientation in place (the
    /// observer path) or fall back to portrait (the initial seed path).
    private func previewAngleOrNil(for orientation: UIDeviceOrientation) -> CGFloat? {
        switch orientation {
        case .portrait, .portraitUpsideDown:
            return 90
        case .landscapeLeft, .landscapeRight:
            return 270
        case .faceUp, .faceDown, .unknown:
            return nil
        @unknown default:
            return nil
        }
    }

    fileprivate func applyOrientation(_ orientation: UIDeviceOrientation) {
        let angle: CGFloat? = previewAngleOrNil(for: orientation)
        guard let angle else {
            print("[camera] orientation: \(orientation.rawValue) — skipped (faceUp/faceDown/unknown)")
            return
        }
        lastValidOrientation = orientation
        pendingPreviewAngle = angle
        let conn = previewLayer.connection
        let supported = conn?.isVideoRotationAngleSupported(angle) ?? false
        print("[camera] orientation: raw=\(orientation.rawValue) angle=\(angle) connection=\(conn != nil) supported=\(supported)")
        if let conn, supported {
            conn.videoRotationAngle = angle
            // Clear any leftover affine transform from a pre-connection
            // fallback — without this, when the connection appears mid-call
            // the affine rotation would compound with the connection's
            // rotation, double-rotating the preview.
            previewLayer.setAffineTransform(.identity)
        } else {
            // Fallback: if AVCaptureVideoPreviewLayer's connection isn't
            // ready yet (it isn't until startRunning has connected the
            // session), apply the rotation as a CALayer affine transform.
            //
            // `videoRotationAngle` is documented as a CLOCKWISE rotation,
            // but iOS's `CGAffineTransform(rotationAngle:)` interprets a
            // positive angle as COUNTER-clockwise. Negate so the fallback
            // matches the connection-driven path visually.
            let radians = -angle * .pi / 180.0
            previewLayer.setAffineTransform(CGAffineTransform(rotationAngle: radians))
        }
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

        // Data-output connection: deliberately NOT rotated. The H.264 encoder
        // is configured from VideoPreset::P720 = 1280×720 landscape; if we
        // rotated 90° here we'd push 720×1280 frames into an encoder set up
        // for 1280×720, which the encoder rejects (and the catalog never
        // advertises video, so subscribers see nothing). Receivers can rotate
        // their display layer if desired.
        if let connection = videoOutput.connection(with: .video) {
            if connection.isVideoMirroringSupported {
                connection.automaticallyAdjustsVideoMirroring = false
                connection.isVideoMirrored = false
            }
        }

        // Preview layer's connection is independent of the data-output's.
        // Initialize from the CURRENT device orientation (the user may be
        // holding the phone in landscape when the call starts) rather
        // than hard-coding 90°/portrait.
        //
        // We seed `lastValidOrientation` and `pendingPreviewAngle`
        // SYNCHRONOUSLY here on the capture queue so the very first
        // `captureOutput` invocation (which reads `lastValidOrientation`
        // for the broadcast-side software rotation) sees the correct
        // value rather than the configure-time `.portrait` default. We
        // also kick off the main-thread `applyOrientation` to set the
        // preview connection angle. After this initial application, the
        // notification observer keeps everything in sync on every flip.
        let initial = orientationProvider()
        let resolvedInitial: UIDeviceOrientation = (initial == .unknown
                                                    || initial == .faceUp
                                                    || initial == .faceDown)
            ? .portrait
            : initial
        lastValidOrientation = resolvedInitial
        // Best-effort: set pendingPreviewAngle directly too so any
        // reader that races the main-thread hop sees the right value.
        pendingPreviewAngle = previewAngle(for: resolvedInitial)
        applyOrientationOnMain(resolvedInitial)
        // Capture the configured data-output angle so the orientation tests
        // can pin it as the immutable baseline. Defaults to 0 (landscape).
        if let dConn = videoOutput.connection(with: .video) {
            initialDataOutputAngle = dConn.videoRotationAngle
        }

        configured = true
    }
}

// MARK: - Test hooks

extension CallCameraCapture {
    /// Mirror of `configureIfNeeded` that does the parts a unit test can
    /// exercise without `startRunning` (which needs camera permission and
    /// real hardware). Just constructs the preview connection so that the
    /// orientation handler has something to write into.
    func configureForTest() {
        // Touch the previewLayer's connection so `applyOrientation` finds
        // something to rotate. On a real device, `startRunning` builds the
        // connection; on a unit test we never start the session, so we
        // build one manually if needed.
        //
        // AVCaptureVideoPreviewLayer.connection is nil until the layer is
        // attached to a running session. In tests we treat the absence
        // as a no-op: the orientation logic still records `lastValidOrientation`
        // so the test can read it back.
        _ = previewLayer
    }

    /// Drive `applyOrientation` from outside without posting a notification.
    /// Used by `CallCameraCaptureOrientationTests`.
    func applyOrientationForTest(_ orientation: UIDeviceOrientation) {
        applyOrientation(orientation)
    }

    /// What angle would `applyOrientation` have set for the most recent
    /// supported orientation? Returns 90 as the configure-time default if
    /// nothing has been applied yet.
    ///
    /// In the simulator/unit-test environment the preview connection
    /// almost never exists, so this falls back to `pendingPreviewAngle`,
    /// which is the angle the connection *would* receive once it appears.
    var previewRotationAngleForTest: CGFloat {
        if let conn = previewLayer.connection {
            return conn.videoRotationAngle
        }
        return pendingPreviewAngle
    }

    /// Configured-once data-output rotation. Must NOT change across
    /// orientation events — guarded by `testDataOutputRotationIsImmutable…`.
    var dataOutputRotationAngleForTest: CGFloat {
        videoOutput.connection(with: .video)?.videoRotationAngle ?? initialDataOutputAngle
    }

    /// Drive the orientation provider for tests. Hosts in the simulator
    /// always report `.unknown` for `UIDevice.current.orientation`, so we
    /// can't rely on the OS to feed the orientation; tests inject a value
    /// here and re-trigger `configureIfNeeded` / observers as needed.
    func setOrientationProviderForTest(_ provider: @escaping () -> UIDeviceOrientation) {
        self.orientationProvider = provider
    }

    /// Drive the `configureIfNeeded` initial-orientation read without
    /// actually starting an AVCaptureSession (which needs camera permission
    /// and real hardware). Pins the initial preview angle.
    func simulateInitialConfigure() {
        let initial = orientationProvider()
        let resolved: UIDeviceOrientation = (initial == .unknown || initial == .faceUp || initial == .faceDown)
            ? .portrait
            : initial
        applyOrientation(resolved)
    }

    /// Read-only access to the last orientation we accepted as "meaningful"
    /// (i.e. not faceUp/faceDown/unknown). Tests read this to confirm that
    /// the resume-from-faceUp path remembers the last UI orientation.
    var lastValidOrientationForTest: UIDeviceOrientation { lastValidOrientation }
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

        // Pack the (possibly row-padded) capture buffer into a tightly-
        // packed BGRA byte array, then software-rotate it to match the
        // current device orientation. This is what makes a portrait-held
        // iOS user appear upright on the receiver's screen: the encoder
        // is locked at 1280×720 landscape and the only knob that travels
        // out the wire is the pixel content itself, so we rotate the
        // pixels here.
        let packed = CallCameraCapture.packTightlyPackedBGRA(
            base: base, width: width, height: height, rowBytes: rowBytes
        )

        let rotated = CallCameraCapture.rotatedFrame(
            sourceBGRA: packed,
            sourceWidth: width,
            sourceHeight: height,
            for: lastValidOrientation
        )
        rotated.data.withUnsafeBufferPointer { buf in
            cb(buf.baseAddress!, rotated.data.count, rotated.width, rotated.height, tsMicros)
        }
    }

    /// Pack a (possibly row-padded) BGRA capture buffer into a tightly-packed
    /// `width*height*4` byte array, stripping any `rowBytes - width*4` stride
    /// padding the capture device added. Extracted from `captureOutput` so the
    /// stride-handling is unit-testable without a live `AVCaptureSession`.
    static func packTightlyPackedBGRA(
        base: UnsafeRawPointer,
        width: Int,
        height: Int,
        rowBytes: Int
    ) -> [UInt8] {
        let expectedRow = width * 4
        var packed = [UInt8](repeating: 0, count: width * height * 4)
        packed.withUnsafeMutableBufferPointer { dst in
            for y in 0..<height {
                let src = base.advanced(by: y * rowBytes).assumingMemoryBound(to: UInt8.self)
                let dstRow = dst.baseAddress!.advanced(by: y * expectedRow)
                dstRow.update(from: src, count: expectedRow)
            }
        }
        return packed
    }

    // MARK: - Software rotation for the broadcast path

    /// Pure rotate-and-letterbox of a tightly-packed BGRA buffer. The
    /// encoder is locked at the source's landscape dimensions
    /// (1280×720 for the P720 preset), so we always return a buffer of
    /// the source's dimensions — portrait orientations are letterboxed
    /// with black bars on the long axis.
    ///
    /// Rotation table (CW, derived to land the user's head at the
    /// output frame's TOP regardless of how the iOS user is holding
    /// the phone):
    ///
    ///   - .landscapeRight       → 0°
    ///   - .portrait             → 90°  (letterbox)
    ///   - .landscapeLeft        → 180°
    ///   - .portraitUpsideDown   → 270° (letterbox)
    ///   - .faceUp/.faceDown/.unknown → use `.portrait` rotation
    ///     (matches the preview's prior-orientation behaviour; the
    ///     orientation observer never updates `lastValidOrientation`
    ///     to a face-up reading anyway).
    static func rotatedFrame(
        sourceBGRA: [UInt8],
        sourceWidth: Int,
        sourceHeight: Int,
        for orientation: UIDeviceOrientation
    ) -> (data: [UInt8], width: Int, height: Int) {
        let resolved: UIDeviceOrientation
        switch orientation {
        case .portrait, .portraitUpsideDown, .landscapeLeft, .landscapeRight:
            resolved = orientation
        case .faceUp, .faceDown, .unknown:
            resolved = .portrait
        @unknown default:
            resolved = .portrait
        }

        switch resolved {
        case .landscapeRight:
            return (sourceBGRA, sourceWidth, sourceHeight)
        case .landscapeLeft:
            return (rotate180(sourceBGRA, width: sourceWidth, height: sourceHeight),
                    sourceWidth, sourceHeight)
        case .portrait:
            return (rotate90CWLetterboxed(sourceBGRA,
                                         width: sourceWidth,
                                         height: sourceHeight),
                    sourceWidth, sourceHeight)
        case .portraitUpsideDown:
            return (rotate270CWLetterboxed(sourceBGRA,
                                          width: sourceWidth,
                                          height: sourceHeight),
                    sourceWidth, sourceHeight)
        default:
            return (sourceBGRA, sourceWidth, sourceHeight)
        }
    }

    /// In-place 180° rotation: `out[y][x] = in[H-1-y][W-1-x]`.
    private static func rotate180(_ src: [UInt8], width: Int, height: Int) -> [UInt8] {
        var out = [UInt8](repeating: 0, count: src.count)
        out.withUnsafeMutableBufferPointer { dst in
            src.withUnsafeBufferPointer { sb in
                for y in 0..<height {
                    for x in 0..<width {
                        let srcIdx = ((height - 1 - y) * width + (width - 1 - x)) * 4
                        let dstIdx = (y * width + x) * 4
                        dst[dstIdx]     = sb[srcIdx]
                        dst[dstIdx + 1] = sb[srcIdx + 1]
                        dst[dstIdx + 2] = sb[srcIdx + 2]
                        dst[dstIdx + 3] = sb[srcIdx + 3]
                    }
                }
            }
        }
        return out
    }

    /// Rotate 90° CW and letterbox into a `width × height` (landscape)
    /// black canvas. The portrait content is height-fit: scaled to fill
    /// the full output height, padded with black columns on each side.
    ///
    /// Mapping reasoning: a 90° CW rotation of a W×H buffer produces an
    /// H×W image. We then scale that H×W image to fit in W×H (the output
    /// canvas) — height fit gives a (H × H × H/W)-wide column centred in
    /// the canvas.
    private static func rotate90CWLetterboxed(_ src: [UInt8], width W: Int, height H: Int) -> [UInt8] {
        // Rotated dimensions (logical): H × W (portrait).
        // Letterbox so rotated-height (W) fits canvas-height (H):
        //   scale = H / W.
        //   contentColW = round(H * H / W).
        let contentColW = max(1, (H * H + W / 2) / W)
        let xOffset = (W - contentColW) / 2
        var out = [UInt8](repeating: 0, count: W * H * 4)
        out.withUnsafeMutableBufferPointer { dst in
            src.withUnsafeBufferPointer { sb in
                for y in 0..<H {
                    for x in 0..<contentColW {
                        // (x, y) in canvas → local coords inside content
                        // window: (lx, ly) where lx ∈ [0, contentColW),
                        // ly = y. Rotated image is H×W; nearest-neighbour
                        // sample into rotated coords:
                        //   ry = ly * W / H.
                        //   rx = lx * H / contentColW.
                        // Mapping rotated (rx, ry) back to source (sx, sy)
                        // for a 90° CW rotation of a W×H source:
                        //   sx = ry, sy = H - 1 - rx.
                        let ry = (y * W) / H
                        let rx = (x * H) / contentColW
                        let sx = ry
                        let sy = H - 1 - rx
                        let srcIdx = (sy * W + sx) * 4
                        let dstIdx = (y * W + (x + xOffset)) * 4
                        dst[dstIdx]     = sb[srcIdx]
                        dst[dstIdx + 1] = sb[srcIdx + 1]
                        dst[dstIdx + 2] = sb[srcIdx + 2]
                        dst[dstIdx + 3] = sb[srcIdx + 3]
                    }
                }
            }
        }
        return out
    }

    /// Rotate 270° CW (== 90° CCW) and letterbox. Symmetric to
    /// `rotate90CWLetterboxed`. Mapping: for a 270° CW rotation of a
    /// W×H source, rotated (rx, ry) maps back to source (sx, sy):
    ///   sx = W - 1 - ry, sy = rx.
    private static func rotate270CWLetterboxed(_ src: [UInt8], width W: Int, height H: Int) -> [UInt8] {
        let contentColW = max(1, (H * H + W / 2) / W)
        let xOffset = (W - contentColW) / 2
        var out = [UInt8](repeating: 0, count: W * H * 4)
        out.withUnsafeMutableBufferPointer { dst in
            src.withUnsafeBufferPointer { sb in
                for y in 0..<H {
                    for x in 0..<contentColW {
                        let ry = (y * W) / H
                        let rx = (x * H) / contentColW
                        let sx = W - 1 - ry
                        let sy = rx
                        let srcIdx = (sy * W + sx) * 4
                        let dstIdx = (y * W + (x + xOffset)) * 4
                        dst[dstIdx]     = sb[srcIdx]
                        dst[dstIdx + 1] = sb[srcIdx + 1]
                        dst[dstIdx + 2] = sb[srcIdx + 2]
                        dst[dstIdx + 3] = sb[srcIdx + 3]
                    }
                }
            }
        }
        return out
    }
}
