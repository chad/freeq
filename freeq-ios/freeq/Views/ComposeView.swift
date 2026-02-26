import SwiftUI

struct ComposeView: View {
    @EnvironmentObject var appState: AppState
    @State private var text: String = ""
    @FocusState private var isFocused: Bool
    @State private var completions: [String] = []

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

            HStack(alignment: .bottom, spacing: 10) {
                HStack(alignment: .bottom, spacing: 8) {
                    // Photo upload (AT-authenticated only)
                    if appState.authenticatedDID != nil, let target = appState.activeChannel {
                        PhotoPickerButton(channel: target)
                    } else {
                        Image(systemName: "plus.circle.fill")
                            .font(.system(size: 24))
                            .foregroundColor(Theme.textMuted.opacity(0.5))
                    }

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

                    // Send button
                    Button(action: send) {
                        ZStack {
                            Circle()
                                .fill(canSend ? Theme.accent : Theme.textMuted.opacity(0.3))
                                .frame(width: 32, height: 32)

                            Image(systemName: appState.editingMessage != nil ? "checkmark" : "arrow.up")
                                .font(.system(size: 14, weight: .bold))
                                .foregroundColor(.white)
                        }
                    }
                    .disabled(!canSend)
                }
                .padding(.horizontal, 12)
                .padding(.vertical, 8)
                .background(Theme.bgTertiary)
                .cornerRadius(22)
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 10)
            .background(Theme.bgSecondary)
        }
    }

    // MARK: - Nick Autocomplete

    private func updateCompletions() {
        // Check if the user is typing @someone
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
        // Replace the @partial with @nick
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
        if appState.replyingTo != nil {
            return "Reply..."
        }
        if appState.editingMessage != nil {
            return "Edit message..."
        }
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
                // Send failed — keep text so user doesn't lose their message
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
}
