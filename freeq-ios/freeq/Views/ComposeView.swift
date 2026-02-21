import SwiftUI

struct ComposeView: View {
    @EnvironmentObject var appState: AppState
    @State private var text: String = ""
    @FocusState private var isFocused: Bool

    var body: some View {
        VStack(spacing: 0) {
            Divider()

            HStack(alignment: .bottom, spacing: 10) {
                // Text field
                TextField("Message \(appState.activeChannel ?? "")", text: $text, axis: .vertical)
                    .textFieldStyle(.plain)
                    .font(.body)
                    .lineLimit(1...5)
                    .focused($isFocused)
                    .submitLabel(.send)
                    .onSubmit {
                        send()
                    }

                // Send button
                Button(action: send) {
                    Image(systemName: "arrow.up.circle.fill")
                        .font(.system(size: 32))
                        .foregroundColor(text.isEmpty ? .gray : .accentColor)
                }
                .disabled(text.isEmpty)
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 8)
            .background(Color(.systemBackground))
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
        default:
            appState.sendRaw(String(input.dropFirst()))
        }
    }
}
