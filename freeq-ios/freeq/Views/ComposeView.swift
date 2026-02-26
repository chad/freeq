import SwiftUI
import AVFoundation

struct ComposeView: View {
    @EnvironmentObject var appState: AppState
    @State private var text: String = ""
    @FocusState private var isFocused: Bool
    @State private var completions: [String] = []

    // Voice recording state
    @State private var isRecording = false
    @State private var recorder: AVAudioRecorder?
    @State private var recordingTime: TimeInterval = 0
    @State private var recordTimer: Timer?
    @State private var recordingCancelled = false
    @State private var dragOffset: CGFloat = 0
    @State private var holdStart: Date? = nil

    var body: some View {
        VStack(spacing: 0) {
            Rectangle()
                .fill(Theme.border)
                .frame(height: 1)

            // Nick autocomplete suggestions
            if !completions.isEmpty {
                ScrollView(.horizontal, showsIndicators: false) {
                    HStack(spacing: 6) {
                        ForEach(completions, id: \.self) { nick in
                            Button(action: { applyCompletion(nick) }) {
                                HStack(spacing: 4) {
                                    UserAvatar(nick: nick, size: 20)
                                    Text(nick)
                                        .font(.system(size: 13, weight: .medium))
                                        .foregroundColor(Theme.textPrimary)
                                }
                                .padding(.horizontal, 10)
                                .padding(.vertical, 6)
                                .background(Theme.bgTertiary)
                                .cornerRadius(16)
                            }
                            .buttonStyle(.plain)
                        }
                    }
                    .padding(.horizontal, 12)
                    .padding(.vertical, 8)
                }
                .background(Theme.bgSecondary)
            }

            // Reply / Edit context bar
            if let reply = appState.replyingTo {
                contextBar(
                    icon: "arrowshape.turn.up.left.fill",
                    label: "Replying to \(reply.from)",
                    preview: reply.text,
                    color: Theme.accent
                ) {
                    appState.replyingTo = nil
                }
            } else if let edit = appState.editingMessage {
                contextBar(
                    icon: "pencil",
                    label: "Editing message",
                    preview: edit.text,
                    color: Theme.warning
                ) {
                    appState.editingMessage = nil
                    text = ""
                }
                .onAppear { text = edit.text }
            }

            // Recording overlay
            if isRecording {
                recordingBar
            } else {
                // Normal compose bar
                HStack(alignment: .bottom, spacing: 8) {
                    // Media button (authenticated only)
                    if appState.authenticatedDID != nil, let target = appState.activeChannel {
                        MediaAttachmentButton(channel: target)
                    } else {
                        Image(systemName: "plus.circle.fill")
                            .font(.system(size: 24))
                            .foregroundColor(Theme.textMuted.opacity(0.5))
                    }

                    // Text input
                    HStack(alignment: .bottom, spacing: 6) {
                        TextField(
                            "",
                            text: $text,
                            prompt: Text(placeholder).foregroundColor(Theme.textMuted),
                            axis: .vertical
                        )
                        .foregroundColor(Theme.textPrimary)
                        .font(.system(size: 16))
                        .lineLimit(1...6)
                        .focused($isFocused)
                        .submitLabel(.send)
                        .onSubmit { send() }
                        .tint(Theme.accent)
                        .onChange(of: text) {
                            updateCompletions()
                            if let target = appState.activeChannel, !text.isEmpty {
                                appState.sendTyping(target: target)
                            }
                        }
                    }
                    .padding(.horizontal, 12)
                    .padding(.vertical, 8)
                    .background(Theme.bgTertiary)
                    .cornerRadius(20)

                    // Send or Mic button
                    if canSend {
                        Button(action: send) {
                            ZStack {
                                Circle()
                                    .fill(Theme.accent)
                                    .frame(width: 36, height: 36)
                                Image(systemName: appState.editingMessage != nil ? "checkmark" : "arrow.up")
                                    .font(.system(size: 15, weight: .bold))
                                    .foregroundColor(.white)
                            }
                        }
                    } else if appState.authenticatedDID != nil {
                        // Mic button — hold to record
                        micButton
                    } else {
                        // Disabled send for guests with no text
                        ZStack {
                            Circle()
                                .fill(Theme.textMuted.opacity(0.2))
                                .frame(width: 36, height: 36)
                            Image(systemName: "arrow.up")
                                .font(.system(size: 15, weight: .bold))
                                .foregroundColor(Theme.textMuted.opacity(0.4))
                        }
                    }
                }
                .padding(.horizontal, 10)
                .padding(.vertical, 8)
                .background(Theme.bgSecondary)
            }
        }
    }

    // MARK: - Mic Button (hold to record)

    @GestureState private var micPressed = false

    private var micButton: some View {
        ZStack {
            Circle()
                .fill(micPressed ? Theme.accent.opacity(0.3) : Theme.bgTertiary)
                .frame(width: 36, height: 36)
            Image(systemName: "mic.fill")
                .font(.system(size: 16))
                .foregroundColor(Theme.accent)
        }
        .gesture(
            DragGesture(minimumDistance: 0)
                .updating($micPressed) { _, state, _ in
                    state = true
                }
                .onChanged { value in
                    // Start recording after holding ~0.2s (detected by still being in gesture)
                    if !isRecording && abs(value.translation.width) < 10 && abs(value.translation.height) < 10 {
                        // Use a timer to start recording after brief hold
                        if holdStart == nil {
                            holdStart = Date()
                        }
                        if let start = holdStart, Date().timeIntervalSince(start) >= 0.2 && !isRecording {
                            startRecording()
                        }
                    }
                    if isRecording {
                        dragOffset = value.translation.width
                        recordingCancelled = dragOffset < -80
                    }
                }
                .onEnded { _ in
                    holdStart = nil
                    if isRecording {
                        stopRecording()
                    }
                }
        )
    }

    // MARK: - Recording Bar

    private var recordingBar: some View {
        HStack(spacing: 12) {
            // Red dot + timer
            HStack(spacing: 8) {
                Circle()
                    .fill(Theme.danger)
                    .frame(width: 10, height: 10)
                    .opacity(recordingTime.truncatingRemainder(dividingBy: 1) < 0.5 ? 1 : 0.3)

                Text(formatDuration(recordingTime))
                    .font(.system(size: 16, weight: .medium, design: .monospaced))
                    .foregroundColor(Theme.textPrimary)
            }

            Spacer()

            if recordingCancelled || dragOffset < -40 {
                Text("Release to cancel")
                    .font(.system(size: 14))
                    .foregroundColor(Theme.danger)
            } else {
                HStack(spacing: 4) {
                    Image(systemName: "chevron.left")
                        .font(.system(size: 11))
                    Text("Slide to cancel")
                        .font(.system(size: 14))
                }
                .foregroundColor(Theme.textMuted)
            }

            Spacer()

            // Stop and send
            Button(action: stopRecording) {
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
        .padding(.horizontal, 12)
        .padding(.vertical, 10)
        .background(Theme.danger.opacity(0.08))
        .background(Theme.bgSecondary)
    }

    // MARK: - Voice Recording

    private func startRecording() {
        // Check permission
        switch AVAudioApplication.shared.recordPermission {
        case .denied:
            ToastManager.shared.show("Microphone access denied", icon: "mic.slash")
            return
        case .undetermined:
            AVAudioApplication.requestRecordPermission { granted in
                if granted {
                    DispatchQueue.main.async { self.startRecording() }
                }
            }
            return
        default: break
        }

        let session = AVAudioSession.sharedInstance()
        do {
            try session.setCategory(.record, mode: .default)
            try session.setActive(true)
        } catch {
            ToastManager.shared.show("Audio error", icon: "exclamationmark.triangle")
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
            recordingCancelled = false
            dragOffset = 0
            UIImpactFeedbackGenerator(style: .heavy).impactOccurred()

            recordTimer = Timer.scheduledTimer(withTimeInterval: 0.1, repeats: true) { _ in
                recordingTime = recorder?.currentTime ?? 0
                if recordingTime >= 300 { stopRecording() }
            }
        } catch {
            ToastManager.shared.show("Recording failed", icon: "exclamationmark.triangle")
        }
    }

    private func stopRecording() {
        recordTimer?.invalidate()
        recordTimer = nil
        guard let recorder = recorder, isRecording else { return }
        recorder.stop()
        isRecording = false
        self.recorder = nil

        // Always deactivate audio session
        try? AVAudioSession.sharedInstance().setActive(false, options: .notifyOthersOnDeactivation)

        let cancelled = recordingCancelled || dragOffset < -80 || recordingTime < 0.5
        dragOffset = 0
        recordingCancelled = false

        if cancelled {
            // Discard
            try? FileManager.default.removeItem(at: recorder.url)
            UIImpactFeedbackGenerator(style: .light).impactOccurred()
            return
        }

        // Send the voice message
        guard let data = try? Data(contentsOf: recorder.url) else { return }
        let duration = formatDuration(recordingTime)

        guard let target = appState.activeChannel else { return }
        UIImpactFeedbackGenerator(style: .medium).impactOccurred()

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
            body.append("Content-Disposition: form-data; name=\"channel\"\r\n\r\n\(target)\r\n".data(using: .utf8)!)
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
                        appState.sendMessage(target: target, text: "🎤 Voice message (\(duration)) \(url)")
                    }
                } else {
                    await MainActor.run {
                        ToastManager.shared.show("Upload failed", icon: "exclamationmark.triangle")
                    }
                }
            } catch {
                await MainActor.run {
                    ToastManager.shared.show("Upload failed", icon: "exclamationmark.triangle")
                }
            }
        }
    }

    // MARK: - Nick Autocomplete

    private func updateCompletions() {
        guard let lastWord = text.split(separator: " ").last,
              lastWord.hasPrefix("@"),
              lastWord.count > 1 else {
            completions = []
            return
        }

        let prefix = String(lastWord.dropFirst()).lowercased()
        let members = appState.activeChannelState?.members ?? []
        completions = members
            .map { $0.nick }
            .filter { $0.lowercased().hasPrefix(prefix) && $0.lowercased() != appState.nick.lowercased() }
            .sorted()
            .prefix(5)
            .map { String($0) }
    }

    private func applyCompletion(_ nick: String) {
        var words = text.split(separator: " ", omittingEmptySubsequences: false).map(String.init)
        if let lastIdx = words.indices.last, words[lastIdx].hasPrefix("@") {
            words[lastIdx] = "@\(nick)"
        }
        text = words.joined(separator: " ") + " "
        completions = []
    }

    private var canSend: Bool {
        !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    }

    private var placeholder: String {
        if appState.replyingTo != nil { return "Reply..." }
        if appState.editingMessage != nil { return "Edit message..." }
        return "Message \(appState.activeChannel ?? "")"
    }

    private func contextBar(icon: String, label: String, preview: String, color: Color, onDismiss: @escaping () -> Void) -> some View {
        HStack(spacing: 8) {
            Rectangle()
                .fill(color)
                .frame(width: 3)

            Image(systemName: icon)
                .font(.system(size: 12))
                .foregroundColor(color)

            VStack(alignment: .leading, spacing: 1) {
                Text(label)
                    .font(.system(size: 12, weight: .semibold))
                    .foregroundColor(color)
                Text(preview)
                    .font(.system(size: 12))
                    .foregroundColor(Theme.textMuted)
                    .lineLimit(1)
            }

            Spacer()

            Button(action: onDismiss) {
                Image(systemName: "xmark.circle.fill")
                    .font(.system(size: 18))
                    .foregroundColor(Theme.textMuted)
            }
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 8)
        .background(Theme.bgSecondary)
    }

    private func send() {
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty, let target = appState.activeChannel else { return }

        completions = []

        if trimmed.hasPrefix("/") {
            handleCommand(trimmed)
            text = ""
        } else {
            if appState.sendMessage(target: target, text: trimmed) {
                text = ""
            } else {
                UINotificationFeedbackGenerator().notificationOccurred(.error)
                return
            }
        }
        UIImpactFeedbackGenerator(style: .light).impactOccurred()
    }

    private func handleCommand(_ input: String) {
        let parts = input.dropFirst().split(separator: " ", maxSplits: 1)
        guard let cmd = parts.first else { return }

        switch cmd.lowercased() {
        case "join":
            if let channel = parts.dropFirst().first {
                appState.joinChannel(String(channel))
            }
        case "part", "leave":
            if let channel = appState.activeChannel {
                appState.partChannel(channel)
            }
        case "nick":
            if let newNick = parts.dropFirst().first {
                appState.sendRaw("NICK \(newNick)")
            }
        case "me":
            if let action = parts.dropFirst().first, let target = appState.activeChannel {
                appState.sendRaw("PRIVMSG \(target) :\u{01}ACTION \(action)\u{01}")
            }
        case "msg":
            let msgParts = input.dropFirst(5).split(separator: " ", maxSplits: 1)
            if msgParts.count == 2 {
                appState.sendMessage(target: String(msgParts[0]), text: String(msgParts[1]))
            }
        case "topic":
            if let rest = parts.dropFirst().first, let channel = appState.activeChannel {
                appState.sendRaw("TOPIC \(channel) :\(rest)")
            }
        default:
            appState.sendRaw(String(input.dropFirst()))
        }
    }

    private func formatDuration(_ t: TimeInterval) -> String {
        let mins = Int(t) / 60
        let secs = Int(t) % 60
        return String(format: "%d:%02d", mins, secs)
    }
}
