import AVFoundation
import SwiftUI

/// Local self-view backed by the camera's `AVCaptureVideoPreviewLayer`.
/// Shows whatever the OS captured, independent of the encoder pipeline —
/// so the local tile never freezes if the network drops or the encoder
/// stalls.
struct LocalPreviewView: UIViewRepresentable {
    let capture: CallCameraCapture

    func makeUIView(context: Context) -> PreviewContainer {
        let v = PreviewContainer()
        v.previewLayer = capture.previewLayer
        return v
    }

    func updateUIView(_ uiView: PreviewContainer, context: Context) {
        uiView.previewLayer = capture.previewLayer
    }

    final class PreviewContainer: UIView {
        var previewLayer: AVCaptureVideoPreviewLayer? {
            didSet {
                guard previewLayer !== oldValue else { return }
                oldValue?.removeFromSuperlayer()
                if let layer = previewLayer, layer.superlayer == nil {
                    self.layer.addSublayer(layer)
                }
                setNeedsLayout()
            }
        }
        override func layoutSubviews() {
            super.layoutSubviews()
            previewLayer?.frame = bounds
        }
    }
}

/// Remote participant video tile backed by an `AVSampleBufferDisplayLayer`.
/// Pulls frames from `appState.videoSink(for:)`, which the AV event handler
/// feeds whenever a `videoFrame` event arrives for this nick.
///
/// The display layer enqueues sample buffers directly — no SwiftUI re-render
/// per frame.
struct RemoteVideoTile: UIViewRepresentable {
    let appState: AppState
    let nick: String

    func makeUIView(context: Context) -> SampleBufferView {
        let v = SampleBufferView()
        appState.bindVideoSink(nick: nick, to: v.displayLayer)
        return v
    }

    func updateUIView(_ uiView: SampleBufferView, context: Context) {
        appState.bindVideoSink(nick: nick, to: uiView.displayLayer)
    }

    static func dismantleUIView(_ uiView: SampleBufferView, coordinator: ()) {
        // Sink lifetime is owned by AppState; nothing to do here.
    }

    final class SampleBufferView: UIView {
        let displayLayer = AVSampleBufferDisplayLayer()

        override init(frame: CGRect) {
            super.init(frame: frame)
            displayLayer.videoGravity = .resizeAspectFill
            layer.addSublayer(displayLayer)
        }
        required init?(coder: NSCoder) { fatalError("init(coder:) not used") }

        override func layoutSubviews() {
            super.layoutSubviews()
            displayLayer.frame = bounds
        }
    }
}

/// Decodes a tightly-packed BGRA buffer into a `CMSampleBuffer` and enqueues
/// it on the given display layer. Called on the AV callback queue from
/// `AppState.AvCallbackHandler` (`videoFrame` case).
///
/// We rebuild the format description only when the dimensions change — the
/// common case is a steady stream of same-sized frames.
enum VideoSampleBuffer {
    /// Build a 32BGRA `CVPixelBuffer` from a tightly-packed BGRA byte array.
    /// Handles row padding (`bytesPerRow > width*4`) by copying row-by-row.
    /// Returns nil on a size mismatch or allocation failure. Extracted from
    /// `enqueue` so the inbound-video pixel path is unit-testable: round-trip
    /// a known frame's bytes without needing a display layer.
    static func makePixelBuffer(bgra: [UInt8], width: Int, height: Int) -> CVPixelBuffer? {
        guard width > 0, height > 0, bgra.count == width * height * 4 else { return nil }

        var pixelBuffer: CVPixelBuffer?
        let attrs: [CFString: Any] = [
            kCVPixelBufferIOSurfacePropertiesKey: [:]
        ]
        let status = CVPixelBufferCreate(
            kCFAllocatorDefault,
            width,
            height,
            kCVPixelFormatType_32BGRA,
            attrs as CFDictionary,
            &pixelBuffer
        )
        guard status == kCVReturnSuccess, let pb = pixelBuffer else {
            print("[av] CVPixelBufferCreate failed: \(status)")
            return nil
        }

        CVPixelBufferLockBaseAddress(pb, [])
        defer { CVPixelBufferUnlockBaseAddress(pb, []) }

        let rowBytes = CVPixelBufferGetBytesPerRow(pb)
        guard let dst = CVPixelBufferGetBaseAddress(pb) else { return nil }
        let expectedRow = width * 4

        bgra.withUnsafeBufferPointer { src in
            if rowBytes == expectedRow {
                memcpy(dst, src.baseAddress!, width * height * 4)
            } else {
                for y in 0..<height {
                    let srcRow = src.baseAddress!.advanced(by: y * expectedRow)
                    let dstRow = dst.advanced(by: y * rowBytes)
                    memcpy(dstRow, srcRow, expectedRow)
                }
            }
        }
        return pb
    }

    /// Read a 32BGRA `CVPixelBuffer` back into a tightly-packed byte array
    /// (strips any row padding). Test helper for verifying pixel round-trips.
    static func readTightlyPacked(_ pb: CVPixelBuffer) -> [UInt8] {
        CVPixelBufferLockBaseAddress(pb, .readOnly)
        defer { CVPixelBufferUnlockBaseAddress(pb, .readOnly) }
        let width = CVPixelBufferGetWidth(pb)
        let height = CVPixelBufferGetHeight(pb)
        let rowBytes = CVPixelBufferGetBytesPerRow(pb)
        let expectedRow = width * 4
        var out = [UInt8](repeating: 0, count: width * height * 4)
        guard let base = CVPixelBufferGetBaseAddress(pb) else { return out }
        out.withUnsafeMutableBufferPointer { dst in
            for y in 0..<height {
                let srcRow = base.advanced(by: y * rowBytes).assumingMemoryBound(to: UInt8.self)
                let dstRow = dst.baseAddress!.advanced(by: y * expectedRow)
                dstRow.update(from: srcRow, count: expectedRow)
            }
        }
        return out
    }

    /// Returns false if the frame couldn't be converted (size mismatch,
    /// allocation failure). Logs but doesn't throw.
    @discardableResult
    static func enqueue(
        bgra: [UInt8],
        width: Int,
        height: Int,
        on layer: AVSampleBufferDisplayLayer
    ) -> Bool {
        guard bgra.count == width * height * 4 else {
            print("[av] BGRA size mismatch: got \(bgra.count), expected \(width * height * 4)")
            return false
        }

        guard let pb = makePixelBuffer(bgra: bgra, width: width, height: height) else {
            return false
        }

        var formatDesc: CMVideoFormatDescription?
        let fmtStatus = CMVideoFormatDescriptionCreateForImageBuffer(
            allocator: kCFAllocatorDefault,
            imageBuffer: pb,
            formatDescriptionOut: &formatDesc
        )
        guard fmtStatus == noErr, let desc = formatDesc else { return false }

        var timing = CMSampleTimingInfo(
            duration: CMTime(value: 1, timescale: 30),
            presentationTimeStamp: CMClockGetTime(CMClockGetHostTimeClock()),
            decodeTimeStamp: .invalid
        )

        var sampleBuffer: CMSampleBuffer?
        let sbStatus = CMSampleBufferCreateReadyWithImageBuffer(
            allocator: kCFAllocatorDefault,
            imageBuffer: pb,
            formatDescription: desc,
            sampleTiming: &timing,
            sampleBufferOut: &sampleBuffer
        )
        guard sbStatus == noErr, let sb = sampleBuffer else { return false }

        // Display immediately; we're already running behind real time.
        if let attachments = CMSampleBufferGetSampleAttachmentsArray(sb, createIfNecessary: true) as? [CFMutableDictionary],
           let first = attachments.first {
            let key = Unmanaged.passUnretained(kCMSampleAttachmentKey_DisplayImmediately).toOpaque()
            let value = Unmanaged.passUnretained(kCFBooleanTrue).toOpaque()
            CFDictionarySetValue(first, key, value)
        }

        layer.enqueue(sb)
        return true
    }
}
