import AVFoundation
import SwiftUI

/// Voice/video call panel — shown when the user is in an AV session.
/// Camera and screen sharing are off by default. Screen share uses the native
/// video path until the SDK grows web-style `/screen` spotlight broadcasts.
struct CallView: View {
    @Environment(AppState.self) private var appState
    let channel: String

    var body: some View {
        VStack(spacing: 0) {
            if appState.isInCall {
                if appState.isCallExpanded { expandedGrid } else { participantStrip }
                controlsBar
            }
        }
        .background(.bar)
    }

    private var participantStrip: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 8) {
                tile(nick: appState.nick.isEmpty ? "You" : appState.nick, label: "You", isLocal: true)
                ForEach(appState.callParticipants, id: \.self) { nick in
                    tile(nick: nick, label: nick, isLocal: false)
                }
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 8)
        }
    }

    @ViewBuilder
    private func tile(nick: String, label: String, isLocal: Bool) -> some View {
        let hasVideo = isLocal
            ? ((appState.isCameraOn && appState.localPreviewCapture != nil) || appState.isScreenSharing)
            : appState.participantsWithVideo.contains(nick)
        VStack(spacing: 4) {
            ZStack {
                RoundedRectangle(cornerRadius: 8)
                    .fill(Color(nsColor: .controlBackgroundColor))
                    .frame(width: 110, height: 80)
                if isLocal, appState.isCameraOn, let cap = appState.localPreviewCapture {
                    LocalPreviewView(capture: cap)
                        .frame(width: 110, height: 80)
                        .clipShape(RoundedRectangle(cornerRadius: 8))
                } else if isLocal, appState.isScreenSharing {
                    VStack(spacing: 6) {
                        Image(systemName: "rectangle.on.rectangle")
                            .font(.title2.weight(.semibold))
                        Text("Sharing screen")
                            .font(.caption2.weight(.medium))
                    }
                    .foregroundStyle(Theme.accent)
                } else if !isLocal {
                    RemoteVideoTile(appState: appState, nick: nick)
                        .frame(width: 110, height: 80)
                        .clipShape(RoundedRectangle(cornerRadius: 8))
                        .opacity(hasVideo ? 1 : 0)
                }
                if !hasVideo {
                    Text(String(nick.prefix(2).uppercased()))
                        .font(.title2.weight(.bold))
                        .foregroundStyle(Color.accentColor)
                }
            }
            Text(label)
                .font(.caption2)
                .foregroundStyle(.secondary)
                .lineLimit(1)
        }
    }

    private var expandedGrid: some View {
        VStack(spacing: 6) {
            ForEach(appState.callParticipants, id: \.self) { nick in
                expandedTile(nick: nick, isLocal: false)
            }
            expandedTile(nick: appState.nick.isEmpty ? "You" : appState.nick, isLocal: true)
        }
        .padding(8)
        .frame(maxWidth: .infinity, minHeight: 240, maxHeight: .infinity)
    }

    @ViewBuilder
    private func expandedTile(nick: String, isLocal: Bool) -> some View {
        let hasVideo = isLocal
            ? ((appState.isCameraOn && appState.localPreviewCapture != nil) || appState.isScreenSharing)
            : appState.participantsWithVideo.contains(nick)
        ZStack {
            RoundedRectangle(cornerRadius: 14)
                .fill(Color(nsColor: .controlBackgroundColor))
            if isLocal, appState.isCameraOn, let cap = appState.localPreviewCapture {
                LocalPreviewView(capture: cap)
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else if isLocal, appState.isScreenSharing {
                VStack(spacing: 12) {
                    Image(systemName: "rectangle.on.rectangle")
                        .font(.system(size: 52, weight: .semibold))
                    Text("Sharing your screen")
                        .font(.headline.weight(.semibold))
                }
                .foregroundStyle(Theme.accent)
            } else if !isLocal {
                RemoteVideoTile(appState: appState, nick: nick)
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
                    .opacity(hasVideo ? 1 : 0)
            }
            if !hasVideo {
                Text(String(nick.prefix(2).uppercased()))
                    .font(.system(size: 46, weight: .bold))
                    .foregroundStyle(Color.accentColor)
            }
            VStack {
                Spacer()
                HStack {
                    Text(isLocal ? "You" : nick)
                        .font(.caption.weight(.medium))
                        .foregroundStyle(.white)
                        .padding(.horizontal, 8).padding(.vertical, 3)
                        .background(Color.black.opacity(0.55))
                        .clipShape(Capsule())
                    Spacer()
                }
                .padding(8)
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .clipShape(RoundedRectangle(cornerRadius: 14))
    }

    private var controlsBar: some View {
        HStack(spacing: 14) {
            HStack(spacing: 6) {
                Circle().fill(Color.green).frame(width: 8, height: 8)
                Text(appState.isScreenSharing ? "Screen" : (appState.isCameraOn ? "Video" : "Voice"))
                    .font(.subheadline.weight(.medium))
                    .foregroundStyle(.green)
                if !channel.isEmpty {
                    Text(channel)
                        .font(.subheadline)
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                }
                Text("· \(appState.callParticipants.count + 1)")
                    .font(.subheadline)
                    .foregroundStyle(.secondary)
            }
            Spacer()
            controlButton(systemName: appState.isCallExpanded
                ? "arrow.down.right.and.arrow.up.left"
                : "arrow.up.left.and.arrow.down.right", active: false) {
                appState.isCallExpanded.toggle()
            }
            controlButton(systemName: appState.isMuted ? "mic.slash.fill" : "mic.fill",
                          active: appState.isMuted, activeColor: .red) {
                appState.toggleMute()
            }
            controlButton(systemName: appState.isCameraOn ? "video.fill" : "video.slash.fill",
                          active: appState.isCameraOn) {
                appState.toggleCamera()
            }
            controlButton(systemName: appState.isScreenSharing ? "rectangle.on.rectangle.fill" : "rectangle.on.rectangle",
                          active: appState.isScreenSharing,
                          activeColor: Theme.accent) {
                appState.toggleScreenShare()
            }
            controlButton(systemName: "phone.down.fill", active: true, activeColor: .red) {
                appState.leaveCall()
            }
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 10)
        .background(Color(nsColor: .controlBackgroundColor))
    }

    @ViewBuilder
    private func controlButton(systemName: String, active: Bool,
                               activeColor: Color = .accentColor,
                               action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Image(systemName: systemName)
                .font(.system(size: 15))
                .foregroundStyle(.white)
                .frame(width: 36, height: 36)
                .background(active ? activeColor : Color.gray.opacity(0.4))
                .clipShape(Circle())
        }
        .buttonStyle(.plain)
    }
}

// MARK: - Local self-view (AVCaptureVideoPreviewLayer)

struct LocalPreviewView: NSViewRepresentable {
    let capture: CallCameraCapture

    func makeNSView(context: Context) -> PreviewContainer {
        let v = PreviewContainer()
        v.attach(capture.previewLayer)
        return v
    }

    func updateNSView(_ nsView: PreviewContainer, context: Context) {
        nsView.attach(capture.previewLayer)
    }

    final class PreviewContainer: NSView {
        private weak var preview: AVCaptureVideoPreviewLayer?
        override init(frame frameRect: NSRect) {
            super.init(frame: frameRect)
            wantsLayer = true
        }
        required init?(coder: NSCoder) { fatalError("init(coder:) not used") }

        func attach(_ layer: AVCaptureVideoPreviewLayer) {
            guard preview !== layer else { return }
            preview?.removeFromSuperlayer()
            self.layer?.addSublayer(layer)
            preview = layer
            layoutPreview()
        }
        override func layout() {
            super.layout()
            layoutPreview()
        }
        private func layoutPreview() {
            preview?.frame = bounds
        }
    }
}

// MARK: - Remote participant tile (AVSampleBufferDisplayLayer)

struct RemoteVideoTile: NSViewRepresentable {
    let appState: AppState
    let nick: String

    func makeNSView(context: Context) -> SampleBufferView {
        let v = SampleBufferView()
        appState.bindVideoSink(nick: nick, to: v.displayLayer)
        return v
    }

    func updateNSView(_ nsView: SampleBufferView, context: Context) {
        appState.bindVideoSink(nick: nick, to: nsView.displayLayer)
    }

    final class SampleBufferView: NSView {
        let displayLayer = AVSampleBufferDisplayLayer()
        override init(frame frameRect: NSRect) {
            super.init(frame: frameRect)
            wantsLayer = true
            displayLayer.videoGravity = .resizeAspectFill
            layer?.addSublayer(displayLayer)
        }
        required init?(coder: NSCoder) { fatalError("init(coder:) not used") }
        override func layout() {
            super.layout()
            displayLayer.frame = bounds
        }
    }
}

// MARK: - BGRA → CMSampleBuffer

/// Decodes a tightly-packed BGRA buffer into a `CMSampleBuffer` and enqueues
/// it on the given display layer. Called from the AV callback handler.
enum VideoSampleBuffer {
    @discardableResult
    static func enqueue(bgra: [UInt8], width: Int, height: Int,
                        on layer: AVSampleBufferDisplayLayer) -> Bool {
        guard bgra.count == width * height * 4 else {
            print("[av] BGRA size mismatch: got \(bgra.count), expected \(width * height * 4)")
            return false
        }

        var pixelBuffer: CVPixelBuffer?
        let attrs: [CFString: Any] = [kCVPixelBufferIOSurfacePropertiesKey: [:]]
        let status = CVPixelBufferCreate(
            kCFAllocatorDefault, width, height,
            kCVPixelFormatType_32BGRA, attrs as CFDictionary, &pixelBuffer
        )
        guard status == kCVReturnSuccess, let pb = pixelBuffer else {
            print("[av] CVPixelBufferCreate failed: \(status)")
            return false
        }

        CVPixelBufferLockBaseAddress(pb, [])
        defer { CVPixelBufferUnlockBaseAddress(pb, []) }

        let rowBytes = CVPixelBufferGetBytesPerRow(pb)
        guard let dst = CVPixelBufferGetBaseAddress(pb) else { return false }
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

        var formatDesc: CMVideoFormatDescription?
        let fmtStatus = CMVideoFormatDescriptionCreateForImageBuffer(
            allocator: kCFAllocatorDefault, imageBuffer: pb, formatDescriptionOut: &formatDesc
        )
        guard fmtStatus == noErr, let desc = formatDesc else { return false }

        var timing = CMSampleTimingInfo(
            duration: CMTime(value: 1, timescale: 30),
            presentationTimeStamp: CMClockGetTime(CMClockGetHostTimeClock()),
            decodeTimeStamp: .invalid
        )

        var sampleBuffer: CMSampleBuffer?
        let sbStatus = CMSampleBufferCreateReadyWithImageBuffer(
            allocator: kCFAllocatorDefault, imageBuffer: pb,
            formatDescription: desc, sampleTiming: &timing, sampleBufferOut: &sampleBuffer
        )
        guard sbStatus == noErr, let sb = sampleBuffer else { return false }

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
