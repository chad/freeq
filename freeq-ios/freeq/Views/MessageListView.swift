import SwiftUI

struct MessageListView: View {
    @ObservedObject var channel: ChannelState

    var body: some View {
        ScrollViewReader { proxy in
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 0) {
                    ForEach(channel.messages) { msg in
                        MessageRow(message: msg)
                            .id(msg.id)
                    }
                }
                .padding(.vertical, 8)
            }
            .onChange(of: channel.messages.count) {
                if let last = channel.messages.last {
                    withAnimation(.easeOut(duration: 0.15)) {
                        proxy.scrollTo(last.id, anchor: .bottom)
                    }
                }
            }
        }
    }
}

struct MessageRow: View {
    let message: ChatMessage

    private var isSystem: Bool { message.from.isEmpty }

    var body: some View {
        if isSystem {
            // System message (join/part/kick)
            HStack {
                Spacer()
                Text(message.text)
                    .font(.caption)
                    .foregroundColor(.secondary)
                    .padding(.vertical, 4)
                Spacer()
            }
        } else {
            HStack(alignment: .top, spacing: 10) {
                // Avatar placeholder
                Circle()
                    .fill(avatarColor(for: message.from))
                    .frame(width: 36, height: 36)
                    .overlay(
                        Text(String(message.from.prefix(1)).uppercased())
                            .font(.system(size: 15, weight: .bold))
                            .foregroundColor(.white)
                    )

                VStack(alignment: .leading, spacing: 3) {
                    // Nick + timestamp
                    HStack(alignment: .firstTextBaseline, spacing: 6) {
                        Text(message.from)
                            .font(.system(size: 15, weight: .bold))

                        Text(formatTime(message.timestamp))
                            .font(.system(size: 11))
                            .foregroundColor(.secondary)
                    }

                    // Message text
                    if message.isAction {
                        Text("*\(message.from) \(message.text)*")
                            .font(.system(size: 15))
                            .italic()
                            .foregroundColor(.secondary)
                    } else {
                        Text(message.text)
                            .font(.system(size: 15))
                            .textSelection(.enabled)
                    }
                }

                Spacer(minLength: 0)
            }
            .padding(.horizontal, 16)
            .padding(.vertical, 4)
        }
    }

    private func formatTime(_ date: Date) -> String {
        let formatter = DateFormatter()
        formatter.dateFormat = "h:mm a"
        return formatter.string(from: date)
    }

    private func avatarColor(for nick: String) -> Color {
        let colors: [Color] = [
            .blue, .green, .orange, .purple, .pink, .teal, .indigo, .red
        ]
        let hash = nick.unicodeScalars.reduce(0) { $0 &+ Int($1.value) }
        return colors[abs(hash) % colors.count]
    }
}
