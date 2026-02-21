import SwiftUI

struct ComposeView: View {
    @EnvironmentObject var appState: AppState
    @State private var text: String = ""
    @FocusState private var isFocused: Bool

    var body: some View {
        VStack(spacing: 0) {
            Rectangle()
                .fill(Theme.border)
                .frame(height: 1)

            HStack(alignment: .bottom, spacing: 10) {
                // Compose area
                HStack(alignment: .bottom, spacing: 8) {
                    // Plus button for attachments (placeholder)
                    Button(action: {}) {
                        Image(systemName: "plus.circle.fill")
                            .font(.system(size: 24))
                            .foregroundColor(Theme.textMuted)
                    }

                    TextField(
                        "",
                        text: $text,
                        prompt: Text("Message \(appState.activeChannel ?? "")").foregroundColor(Theme.textMuted),
                        axis: .vertical
                    )
                    .foregroundColor(Theme.textPrimary)
                    .font(.system(size: 16))
                    .lineLimit(1...6)
                    .focused($isFocused)
                    .submitLabel(.send)
                    .onSubmit { send() }
                    .tint(Theme.accent)

                    // Send button
                    Button(action: send) {
                        ZStack {
                            Circle()
                                .fill(text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
                                      ? Theme.textMuted.opacity(0.3)
                                      : Theme.accent)
                                .frame(width: 32, height: 32)

                            Image(systemName: "arrow.up")
                                .font(.system(size: 14, weight: .bold))
                                .foregroundColor(.white)
                        }
                    }
                    .disabled(text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
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

    private func send() {
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty, let target = appState.activeChannel else { return }

        if trimmed.hasPrefix("/") {
            handleCommand(trimmed)
        } else {
            appState.sendMessage(target: target, text: trimmed)
        }
        text = ""
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
