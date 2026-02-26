import SwiftUI
import AVFoundation
import PhotosUI
import UniformTypeIdentifiers

// MARK: - Media Attachment Button (replaces photo-only picker)

/// Unified media button — photo library, take photo/video, record audio.
struct MediaAttachmentButton: View {
    @EnvironmentObject var appState: AppState
    let channel: String

    @State private var showingActionSheet = false
    @State private var showingPhotoPicker = false
    @State private var showingCamera = false
    @State private var showingAudioRecorder = false
    @State private var selectedItems: [PhotosPickerItem] = []
    @State private var stagedMedia: StagedMedia? = nil

    var body: some View {
        Button(action: { showingActionSheet = true }) {
            Image(systemName: "plus.circle.fill")
                .font(.system(size: 24))
                .foregroundColor(Theme.accent)
        }
        .confirmationDialog("Attach Media", isPresented: $showingActionSheet) {
            Button("Photo Library") { showingPhotoPicker = true }
            Button("Take Photo or Video") { showingCamera = true }
            Button("Record Audio") { showingAudioRecorder = true }
            Button("Cancel", role: .cancel) { }
        }
        .photosPicker(isPresented: $showingPhotoPicker, selection: $selectedItems, maxSelectionCount: 1, matching: .any(of: [.images, .videos]))
        .onChange(of: selectedItems) {
            if let item = selectedItems.first {
                loadMediaItem(item)
                selectedItems = []
            }
        }
        .fullScreenCover(isPresented: $showingCamera) {
            CameraCapture { result in
                showingCamera = false
                if let result = result {
                    stagedMedia = result
                }
            }
            .ignoresSafeArea()
        }
        .sheet(isPresented: $showingAudioRecorder) {
            AudioRecorderSheet(channel: channel)
                .presentationDetents([.medium])
                .presentationDragIndicator(.visible)
        }
        .sheet(item: $stagedMedia) { media in
            MediaPreviewSheet(media: media, channel: channel)
        }
    }

    private func loadMediaItem(_ item: PhotosPickerItem) {
        Task {
            // Try video first
            if let movie = try? await item.loadTransferable(type: VideoTransferable.self) {
                let data = try? Data(contentsOf: movie.url)
                guard let data = data else { return }
                await MainActor.run {
                    stagedMedia = StagedMedia(
                        data: data,
                        contentType: "video/mp4",
                        thumbnail: generateVideoThumbnail(url: movie.url),
                        filename: "video.mp4",
                        duration: videoDuration(url: movie.url)
                    )
                }
                return
            }

            // Fall back to image
            if let data = try? await item.loadTransferable(type: Data.self) {
                guard let uiImage = UIImage(data: data) else { return }
                await MainActor.run {
                    stagedMedia = StagedMedia(
                        data: data,
                        contentType: "image/jpeg",
                        thumbnail: uiImage,
                        filename: "photo.jpg",
                        duration: nil
                    )
                }
            }
        }
    }

    private func generateVideoThumbnail(url: URL) -> UIImage? {
        let asset = AVAsset(url: url)
        let generator = AVAssetImageGenerator(asset: asset)
        generator.appliesPreferredTrackTransform = true
        generator.maximumSize = CGSize(width: 400, height: 400)
        if let cgImage = try? generator.copyCGImage(at: .zero, actualTime: nil) {
            return UIImage(cgImage: cgImage)
        }
        return nil
    }

    private func videoDuration(url: URL) -> TimeInterval? {
        let asset = AVAsset(url: url)
        let duration = asset.duration
        guard duration.timescale > 0 else { return nil }
        return CMTimeGetSeconds(duration)
    }
}

// MARK: - Video Transferable

struct VideoTransferable: Transferable {
    let url: URL

    static var transferRepresentation: some TransferRepresentation {
        FileRepresentation(contentType: .movie) { video in
            SentTransferredFile(video.url)
        } importing: { received in
            let tempURL = FileManager.default.temporaryDirectory.appendingPathComponent("video_\(UUID().uuidString).mp4")
            try FileManager.default.copyItem(at: received.file, to: tempURL)
            return Self(url: tempURL)
        }
    }
}

// MARK: - Staged Media

struct StagedMedia: Identifiable {
    let id = UUID()
    let data: Data
    let contentType: String
    let thumbnail: UIImage?
    let filename: String
    let duration: TimeInterval?

    var isVideo: Bool { contentType.hasPrefix("video/") }
    var isAudio: Bool { contentType.hasPrefix("audio/") }
    var isImage: Bool { contentType.hasPrefix("image/") }

    var sizeString: String {
        ByteCountFormatter.string(fromByteCount: Int64(data.count), countStyle: .file)
    }

    var durationString: String? {
        guard let d = duration else { return nil }
        let mins = Int(d) / 60
        let secs = Int(d) % 60
        return String(format: "%d:%02d", mins, secs)
    }
}

// MARK: - Camera Capture (photo + video)

struct CameraCapture: UIViewControllerRepresentable {
    let completion: (StagedMedia?) -> Void

    func makeUIViewController(context: Context) -> UIImagePickerController {
        let picker = UIImagePickerController()
        picker.sourceType = .camera
        picker.mediaTypes = [UTType.image.identifier, UTType.movie.identifier]
        picker.videoMaximumDuration = 60
        picker.videoQuality = .typeMedium
        picker.delegate = context.coordinator
        return picker
    }

    func updateUIViewController(_ uiViewController: UIImagePickerController, context: Context) {}

    func makeCoordinator() -> Coordinator {
        Coordinator(completion: completion)
    }

    class Coordinator: NSObject, UIImagePickerControllerDelegate, UINavigationControllerDelegate {
        let completion: (StagedMedia?) -> Void

        init(completion: @escaping (StagedMedia?) -> Void) {
            self.completion = completion
        }

        func imagePickerControllerDidCancel(_ picker: UIImagePickerController) {
            completion(nil)
        }

        func imagePickerController(_ picker: UIImagePickerController, didFinishPickingMediaWithInfo info: [UIImagePickerController.InfoKey : Any]) {
            if let mediaURL = info[.mediaURL] as? URL {
                // Video
                if let data = try? Data(contentsOf: mediaURL) {
                    let asset = AVAsset(url: mediaURL)
                    let generator = AVAssetImageGenerator(asset: asset)
                    generator.appliesPreferredTrackTransform = true
                    generator.maximumSize = CGSize(width: 400, height: 400)
                    let thumb = (try? generator.copyCGImage(at: .zero, actualTime: nil)).map(UIImage.init)
                    let duration = CMTimeGetSeconds(asset.duration)
                    completion(StagedMedia(
                        data: data,
                        contentType: "video/mp4",
                        thumbnail: thumb,
                        filename: "video.mp4",
                        duration: duration > 0 ? duration : nil
                    ))
                    return
                }
            }

            if let image = info[.originalImage] as? UIImage,
               let data = image.jpegData(compressionQuality: 0.85) {
                completion(StagedMedia(
                    data: data,
                    contentType: "image/jpeg",
                    thumbnail: image,
                    filename: "photo.jpg",
                    duration: nil
                ))
                return
            }

            completion(nil)
        }
    }
}

// MARK: - Audio Recorder Sheet

struct AudioRecorderSheet: View {
    @EnvironmentObject var appState: AppState
    @Environment(\.dismiss) var dismiss
    let channel: String

    @State private var recorder: AVAudioRecorder?
    @State private var isRecording = false
    @State private var recordingTime: TimeInterval = 0
    @State private var timer: Timer?
    @State private var audioData: Data?
    @State private var uploading = false
    @State private var error: String?
    @State private var permissionDenied = false

    var body: some View {
        NavigationView {
            ZStack {
                Theme.bgPrimary.ignoresSafeArea()

                VStack(spacing: 24) {
                    Spacer()

                    // Waveform visualization
                    ZStack {
                        Circle()
                            .fill(isRecording ? Theme.danger.opacity(0.15) : Theme.accent.opacity(0.1))
                            .frame(width: isRecording ? 160 : 120, height: isRecording ? 160 : 120)
                            .animation(.easeInOut(duration: 0.8).repeatForever(autoreverses: true), value: isRecording)

                        Circle()
                            .fill(isRecording ? Theme.danger.opacity(0.3) : Theme.accent.opacity(0.2))
                            .frame(width: 100, height: 100)

                        Image(systemName: isRecording ? "waveform" : "mic.fill")
                            .font(.system(size: 36))
                            .foregroundColor(isRecording ? Theme.danger : Theme.accent)
                    }

                    // Timer
                    Text(formatDuration(recordingTime))
                        .font(.system(size: 48, weight: .light, design: .monospaced))
                        .foregroundColor(isRecording ? Theme.danger : Theme.textPrimary)

                    if permissionDenied {
                        Text("Microphone access required.\nGo to Settings → freeq → Microphone")
                            .font(.system(size: 14))
                            .foregroundColor(Theme.danger)
                            .multilineTextAlignment(.center)
                    }

                    if let error = error {
                        Text(error)
                            .font(.system(size: 13))
                            .foregroundColor(Theme.danger)
                    }

                    Spacer()

                    // Controls
                    if let audioData = audioData, !isRecording {
                        // Review mode
                        HStack(spacing: 32) {
                            // Discard
                            Button(action: discard) {
                                VStack(spacing: 6) {
                                    Image(systemName: "trash")
                                        .font(.system(size: 22))
                                        .frame(width: 56, height: 56)
                                        .background(Theme.bgTertiary)
                                        .clipShape(Circle())
                                    Text("Discard")
                                        .font(.system(size: 12))
                                }
                                .foregroundColor(Theme.textMuted)
                            }

                            // Send
                            Button(action: { sendAudio(audioData) }) {
                                VStack(spacing: 6) {
                                    ZStack {
                                        Circle()
                                            .fill(Theme.accent)
                                            .frame(width: 56, height: 56)
                                        if uploading {
                                            ProgressView().tint(.white)
                                        } else {
                                            Image(systemName: "arrow.up")
                                                .font(.system(size: 22, weight: .bold))
                                                .foregroundColor(.white)
                                        }
                                    }
                                    Text("Send")
                                        .font(.system(size: 12))
                                        .foregroundColor(Theme.accent)
                                }
                            }
                            .disabled(uploading)
                        }
                    } else {
                        // Record button
                        Button(action: toggleRecording) {
                            ZStack {
                                Circle()
                                    .fill(isRecording ? Theme.danger : Theme.accent)
                                    .frame(width: 72, height: 72)

                                if isRecording {
                                    RoundedRectangle(cornerRadius: 6)
                                        .fill(.white)
                                        .frame(width: 24, height: 24)
                                } else {
                                    Circle()
                                        .fill(.white)
                                        .frame(width: 24, height: 24)
                                }
                            }
                        }
                        .disabled(permissionDenied)

                        Text(isRecording ? "Tap to stop" : "Tap to record")
                            .font(.system(size: 13))
                            .foregroundColor(Theme.textMuted)
                    }

                    Spacer().frame(height: 24)
                }
            }
            .navigationTitle("Voice Message")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") {
                        stopRecording()
                        dismiss()
                    }
                    .foregroundColor(Theme.accent)
                }
            }
            .toolbarBackground(Theme.bgSecondary, for: .navigationBar)
            .toolbarBackground(.visible, for: .navigationBar)
        }
        .preferredColorScheme(.dark)
        .onAppear { checkPermission() }
        .onDisappear { stopRecording() }
    }

    private func checkPermission() {
        switch AVAudioApplication.shared.recordPermission {
        case .granted: break
        case .denied: permissionDenied = true
        case .undetermined:
            AVAudioApplication.requestRecordPermission { granted in
                DispatchQueue.main.async {
                    permissionDenied = !granted
                }
            }
        @unknown default: break
        }
    }

    private func toggleRecording() {
        if isRecording {
            stopRecording()
        } else {
            startRecording()
        }
    }

    private func startRecording() {
        let session = AVAudioSession.sharedInstance()
        do {
            try session.setCategory(.record, mode: .default)
            try session.setActive(true)
        } catch {
            self.error = "Audio session error"
            return
        }

        let tempURL = FileManager.default.temporaryDirectory.appendingPathComponent("voice_\(UUID().uuidString).m4a")

        let settings: [String: Any] = [
            AVFormatIDKey: Int(kAudioFormatMPEG4AAC),
            AVSampleRateKey: 44100,
            AVNumberOfChannelsKey: 1,
            AVEncoderAudioQualityKey: AVAudioQuality.medium.rawValue,
        ]

        do {
            recorder = try AVAudioRecorder(url: tempURL, settings: settings)
            recorder?.record()
            isRecording = true
            recordingTime = 0
            audioData = nil

            timer = Timer.scheduledTimer(withTimeInterval: 0.1, repeats: true) { _ in
                recordingTime = recorder?.currentTime ?? 0
                // Max 5 minutes
                if recordingTime >= 300 {
                    stopRecording()
                }
            }
        } catch {
            self.error = "Failed to start recording"
        }
    }

    private func stopRecording() {
        timer?.invalidate()
        timer = nil
        guard let recorder = recorder, isRecording else { return }
        recorder.stop()
        isRecording = false

        if let data = try? Data(contentsOf: recorder.url) {
            audioData = data
        }

        // Reset audio session
        try? AVAudioSession.sharedInstance().setActive(false)
    }

    private func discard() {
        audioData = nil
        recordingTime = 0
    }

    private func sendAudio(_ data: Data) {
        uploading = true
        error = nil

        Task {
            // Refresh broker session
            if let brokerToken = appState.brokerToken {
                let brokerBase = appState.authBrokerBase
                var req = URLRequest(url: URL(string: "\(brokerBase)/session")!)
                req.httpMethod = "POST"
                req.setValue("application/json", forHTTPHeaderField: "Content-Type")
                req.httpBody = try? JSONSerialization.data(withJSONObject: ["broker_token": brokerToken])
                _ = try? await URLSession.shared.data(for: req)
            }

            let did = appState.authenticatedDID ?? ""
            let boundary = UUID().uuidString
            var body = Data()

            body.append("--\(boundary)\r\n".data(using: .utf8)!)
            body.append("Content-Disposition: form-data; name=\"did\"\r\n\r\n\(did)\r\n".data(using: .utf8)!)
            body.append("--\(boundary)\r\n".data(using: .utf8)!)
            body.append("Content-Disposition: form-data; name=\"channel\"\r\n\r\n\(channel)\r\n".data(using: .utf8)!)
            body.append("--\(boundary)\r\n".data(using: .utf8)!)
            body.append("Content-Disposition: form-data; name=\"file\"; filename=\"voice.m4a\"\r\nContent-Type: audio/mp4\r\n\r\n".data(using: .utf8)!)
            body.append(data)
            body.append("\r\n--\(boundary)--\r\n".data(using: .utf8)!)

            var request = URLRequest(url: URL(string: "https://irc.freeq.at/api/v1/upload")!)
            request.httpMethod = "POST"
            request.timeoutInterval = 30
            request.setValue("multipart/form-data; boundary=\(boundary)", forHTTPHeaderField: "Content-Type")
            request.httpBody = body

            do {
                let (responseData, response) = try await URLSession.shared.data(for: request)
                let status = (response as? HTTPURLResponse)?.statusCode ?? 0
                if status == 200,
                   let json = try? JSONSerialization.jsonObject(with: responseData) as? [String: Any],
                   let url = json["url"] as? String {
                    await MainActor.run {
                        let durationStr = formatDuration(recordingTime)
                        appState.sendMessage(target: channel, text: "🎤 Voice message (\(durationStr)) \(url)")
                        UIImpactFeedbackGenerator(style: .medium).impactOccurred()
                        dismiss()
                    }
                } else {
                    await MainActor.run {
                        uploading = false
                        self.error = "Upload failed"
                    }
                }
            } catch {
                await MainActor.run {
                    uploading = false
                    self.error = error.localizedDescription
                }
            }
        }
    }

    private func formatDuration(_ t: TimeInterval) -> String {
        let mins = Int(t) / 60
        let secs = Int(t) % 60
        return String(format: "%d:%02d", mins, secs)
    }
}

// MARK: - Media Preview Sheet (unified for photo/video)

struct MediaPreviewSheet: View {
    @EnvironmentObject var appState: AppState
    @Environment(\.dismiss) var dismiss
    let media: StagedMedia
    let channel: String

    @State private var caption: String = ""
    @State private var crossPost = false
    @State private var uploading = false
    @State private var uploadError: String? = nil
    @FocusState private var captionFocused: Bool

    var body: some View {
        NavigationView {
            ZStack {
                Theme.bgPrimary.ignoresSafeArea()

                VStack(spacing: 0) {
                    // Preview
                    if let thumb = media.thumbnail {
                        ZStack {
                            Image(uiImage: thumb)
                                .resizable()
                                .scaledToFit()
                                .frame(maxHeight: 400)
                                .clipShape(RoundedRectangle(cornerRadius: 12))

                            if media.isVideo {
                                // Play icon overlay
                                ZStack {
                                    Circle()
                                        .fill(.black.opacity(0.5))
                                        .frame(width: 56, height: 56)
                                    Image(systemName: "play.fill")
                                        .font(.system(size: 24))
                                        .foregroundColor(.white)
                                }

                                // Duration badge
                                if let dur = media.durationString {
                                    VStack {
                                        Spacer()
                                        HStack {
                                            Spacer()
                                            Text(dur)
                                                .font(.system(size: 12, weight: .medium, design: .monospaced))
                                                .foregroundColor(.white)
                                                .padding(.horizontal, 8)
                                                .padding(.vertical, 4)
                                                .background(.black.opacity(0.6))
                                                .cornerRadius(4)
                                                .padding(8)
                                        }
                                    }
                                }
                            }
                        }
                        .padding(16)
                    } else {
                        // No thumbnail (shouldn't happen for photo/video)
                        VStack(spacing: 8) {
                            Image(systemName: media.isVideo ? "film" : "photo")
                                .font(.system(size: 48))
                                .foregroundColor(Theme.textMuted)
                            Text(media.sizeString)
                                .font(.system(size: 14))
                                .foregroundColor(Theme.textMuted)
                        }
                        .frame(height: 200)
                        .padding(16)
                    }

                    // Size info
                    HStack(spacing: 12) {
                        Label(media.isVideo ? "Video" : "Photo", systemImage: media.isVideo ? "film" : "photo")
                            .font(.system(size: 12))
                            .foregroundColor(Theme.textMuted)
                        Text(media.sizeString)
                            .font(.system(size: 12))
                            .foregroundColor(Theme.textMuted)
                        if let dur = media.durationString {
                            Text(dur)
                                .font(.system(size: 12))
                                .foregroundColor(Theme.textMuted)
                        }
                        Spacer()
                    }
                    .padding(.horizontal, 20)

                    // Cross-post toggle
                    if appState.authenticatedDID != nil {
                        Toggle(isOn: $crossPost) {
                            HStack(spacing: 8) {
                                Image(systemName: "arrow.up.right.circle.fill")
                                    .font(.system(size: 16))
                                    .foregroundColor(Color(hex: "0085ff"))
                                Text("Also post to Bluesky")
                                    .font(.system(size: 14, weight: .medium))
                                    .foregroundColor(Theme.textPrimary)
                            }
                        }
                        .tint(Color(hex: "0085ff"))
                        .padding(.horizontal, 20)
                        .padding(.vertical, 8)
                    }

                    Spacer()

                    // Bottom bar
                    VStack(spacing: 0) {
                        Rectangle().fill(Theme.border).frame(height: 1)

                        if let error = uploadError {
                            HStack(spacing: 8) {
                                Image(systemName: "exclamationmark.triangle.fill")
                                    .font(.system(size: 12))
                                    .foregroundColor(Theme.danger)
                                Text(error)
                                    .font(.system(size: 13))
                                    .foregroundColor(Theme.danger)
                                    .lineLimit(2)
                                Spacer()
                                Button("Retry") {
                                    uploadError = nil
                                    upload()
                                }
                                .font(.system(size: 13, weight: .medium))
                                .foregroundColor(Theme.accent)
                            }
                            .padding(.horizontal, 16)
                            .padding(.vertical, 10)
                            .background(Theme.danger.opacity(0.1))
                        }

                        if uploading {
                            HStack(spacing: 12) {
                                ProgressView().tint(Theme.accent)
                                Text("Uploading \(media.sizeString)...")
                                    .font(.system(size: 14))
                                    .foregroundColor(Theme.textSecondary)
                                Spacer()
                            }
                            .padding(.horizontal, 16)
                            .padding(.vertical, 12)
                            .background(Theme.bgSecondary)
                        } else {
                            HStack(alignment: .bottom, spacing: 10) {
                                TextField("Add a caption...", text: $caption, axis: .vertical)
                                    .foregroundColor(Theme.textPrimary)
                                    .font(.system(size: 16))
                                    .lineLimit(1...4)
                                    .focused($captionFocused)
                                    .tint(Theme.accent)

                                Button(action: upload) {
                                    ZStack {
                                        Circle()
                                            .fill(Theme.accent)
                                            .frame(width: 36, height: 36)
                                        Image(systemName: "arrow.up")
                                            .font(.system(size: 15, weight: .bold))
                                            .foregroundColor(.white)
                                    }
                                }
                            }
                            .padding(.horizontal, 16)
                            .padding(.vertical, 12)
                            .background(Theme.bgSecondary)
                        }
                    }
                }
            }
            .navigationTitle(media.isVideo ? "Send Video" : "Send Photo")
            .navigationBarTitleDisplayMode(.inline)
            .toolbarBackground(Theme.bgSecondary, for: .navigationBar)
            .toolbarBackground(.visible, for: .navigationBar)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") { dismiss() }
                        .foregroundColor(Theme.accent)
                        .disabled(uploading)
                }
            }
        }
        .preferredColorScheme(.dark)
        .interactiveDismissDisabled(uploading)
    }

    private func upload() {
        uploading = true
        uploadError = nil

        Task {
            // Refresh broker session
            if let brokerToken = appState.brokerToken {
                let brokerBase = appState.authBrokerBase
                var req = URLRequest(url: URL(string: "\(brokerBase)/session")!)
                req.httpMethod = "POST"
                req.setValue("application/json", forHTTPHeaderField: "Content-Type")
                req.httpBody = try? JSONSerialization.data(withJSONObject: ["broker_token": brokerToken])
                _ = try? await URLSession.shared.data(for: req)
            }

            let did = appState.authenticatedDID ?? ""
            let boundary = UUID().uuidString
            var body = Data()

            body.append("--\(boundary)\r\n".data(using: .utf8)!)
            body.append("Content-Disposition: form-data; name=\"did\"\r\n\r\n\(did)\r\n".data(using: .utf8)!)
            body.append("--\(boundary)\r\n".data(using: .utf8)!)
            body.append("Content-Disposition: form-data; name=\"channel\"\r\n\r\n\(channel)\r\n".data(using: .utf8)!)

            if crossPost {
                body.append("--\(boundary)\r\n".data(using: .utf8)!)
                body.append("Content-Disposition: form-data; name=\"cross_post\"\r\n\r\ntrue\r\n".data(using: .utf8)!)
            }

            body.append("--\(boundary)\r\n".data(using: .utf8)!)
            body.append("Content-Disposition: form-data; name=\"file\"; filename=\"\(media.filename)\"\r\nContent-Type: \(media.contentType)\r\n\r\n".data(using: .utf8)!)
            body.append(media.data)
            body.append("\r\n--\(boundary)--\r\n".data(using: .utf8)!)

            var request = URLRequest(url: URL(string: "https://irc.freeq.at/api/v1/upload")!)
            request.httpMethod = "POST"
            request.timeoutInterval = 60
            request.setValue("multipart/form-data; boundary=\(boundary)", forHTTPHeaderField: "Content-Type")
            request.httpBody = body

            do {
                let (responseData, response) = try await URLSession.shared.data(for: request)
                let status = (response as? HTTPURLResponse)?.statusCode ?? 0
                if status == 200,
                   let json = try? JSONSerialization.jsonObject(with: responseData) as? [String: Any],
                   let url = json["url"] as? String {
                    await MainActor.run {
                        let text = caption.trimmingCharacters(in: .whitespacesAndNewlines)
                        if text.isEmpty {
                            appState.sendMessage(target: channel, text: url)
                        } else {
                            appState.sendMessage(target: channel, text: "\(url) \(text)")
                        }
                        UIImpactFeedbackGenerator(style: .medium).impactOccurred()
                        dismiss()
                    }
                } else {
                    let responseText = String(data: responseData, encoding: .utf8) ?? ""
                    await MainActor.run {
                        uploading = false
                        uploadError = "Upload failed: \(responseText.prefix(80))"
                    }
                }
            } catch {
                await MainActor.run {
                    uploading = false
                    uploadError = error.localizedDescription
                }
            }
        }
    }
}
