import SwiftUI

/// Thread view — shows a reply chain for a given message.
struct ThreadView: View {
    @EnvironmentObject var appState: AppState
    @Environment(\.dismiss) var dismiss
    let rootMessage: ChatMessage
    let channelName: String

    private var channel: ChannelState? {
        appState.channels.first { $0.name == channelName }
            ?? appState.dmBuffers.first { $0.name == channelName }
    }

    /// Build the reply chain: walk up from rootMessage via replyTo, then show all replies to root.
    private var thread: [ChatMessage] {
        guard let ch = channel else { return [rootMessage] }

        var chain: [ChatMessage] = []

        // Walk up the parent chain
        var current: ChatMessage? = rootMessage
        while let msg = current, let replyId = msg.replyTo {
            if let idx = ch.findMessage(byId: replyId) {
                current = ch.messages[idx]
                chain.insert(current!, at: 0)
            } else {
                break
            }
        }

        // Add the root message itself
        chain.append(rootMessage)

        // Find all direct replies to the root message
        let rootId = rootMessage.id
        let replies = ch.messages.filter { $0.replyTo == rootId && $0.id != rootId }
        chain.append(contentsOf: replies)

        return chain
    }

    @StateObject private var avatarCache = AvatarCache.shared

    var body: some View {
        NavigationView {
            ZStack {
                Theme.bgPrimary.ignoresSafeArea()

                ScrollView {
                    LazyVStack(alignment: .leading, spacing: 0) {
                        ForEach(Array(thread.enumerated()), id: \.element.id) { idx, msg in
                            let isRoot = msg.id == rootMessage.id

                            VStack(alignment: .leading, spacing: 0) {
                                // Thread connector line
                                if idx > 0 {
                                    HStack(spacing: 0) {
                                        Rectangle()
                                            .fill(Theme.accent.opacity(0.3))
                                            .frame(width: 2, height: 16)
                                            .padding(.leading, 35)
                                        Spacer()
                                    }
                                }

                                HStack(alignment: .top, spacing: 12) {
                                    // Avatar with thread line
                                    VStack(spacing: 0) {
                                        UserAvatar(nick: msg.from, size: 36)

                                        if idx < thread.count - 1 {
                                            Rectangle()
                                                .fill(Theme.accent.opacity(0.3))
                                                .frame(width: 2)
                                                .frame(maxHeight: .infinity)
                                        }
                                    }
                                    .frame(width: 36)

                                    VStack(alignment: .leading, spacing: 4) {
                                        HStack(alignment: .firstTextBaseline, spacing: 6) {
                                            Text(((channel?.memberInfo(for: msg.from)?.prefix ?? "") + msg.from))
                                                .font(.system(size: 14, weight: .bold))
                                                .foregroundColor(Theme.nickColor(for: msg.from))

                                            if avatarCache.avatarURL(for: msg.from.lowercased()) != nil {
                                                VerifiedBadge(size: 11)
                                            }

                                            Text(formatTime(msg.timestamp))
                                                .font(.system(size: 11))
                                                .foregroundColor(Theme.textMuted)
                                        }

                                        Text(msg.text)
                                            .font(.system(size: 15))
                                            .foregroundColor(Theme.textPrimary)
                                            .textSelection(.enabled)

                                        if msg.isEdited {
                                            Text("(edited)")
                                                .font(.system(size: 11))
                                                .foregroundColor(Theme.textMuted)
                                        }

                                        // Reactions
                                        if !msg.reactions.isEmpty {
                                            HStack(spacing: 4) {
                                                ForEach(Array(msg.reactions.keys.sorted()), id: \.self) { emoji in
                                                    let nicks = msg.reactions[emoji] ?? []
                                                    HStack(spacing: 2) {
                                                        Text(emoji).font(.system(size: 13))
                                                        if nicks.count > 1 {
                                                            Text("\(nicks.count)")
                                                                .font(.system(size: 10, weight: .medium))
                                                                .foregroundColor(Theme.textSecondary)
                                                        }
                                                    }
                                                    .padding(.horizontal, 5)
                                                    .padding(.vertical, 2)
                                                    .background(Theme.bgTertiary)
                                                    .cornerRadius(4)
                                                }
                                            }
                                            .padding(.top, 2)
                                        }
                                    }

                                    Spacer(minLength: 0)
                                }
                                .padding(.horizontal, 16)
                                .padding(.vertical, 6)
                                .background(isRoot ? Theme.accent.opacity(0.05) : Color.clear)
                            }
                        }
                    }
                    .padding(.top, 8)

                    // Reply action
                    Button(action: {
                        appState.replyingTo = rootMessage
                        dismiss()
                    }) {
                        HStack(spacing: 8) {
                            Image(systemName: "arrowshape.turn.up.left.fill")
                                .font(.system(size: 13))
                            Text("Reply to thread")
                                .font(.system(size: 15, weight: .medium))
                        }
                        .frame(maxWidth: .infinity)
                        .padding(.vertical, 12)
                        .background(Theme.accent)
                        .foregroundColor(.white)
                        .cornerRadius(10)
                    }
                    .padding(.horizontal, 16)
                    .padding(.vertical, 16)
                }
            }
            .navigationTitle("Thread")
            .navigationBarTitleDisplayMode(.inline)
            .toolbarBackground(Theme.bgSecondary, for: .navigationBar)
            .toolbarBackground(.visible, for: .navigationBar)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Close") { dismiss() }
                        .foregroundColor(Theme.accent)
                }
            }
        }
        .preferredColorScheme(appState.isDarkTheme ? .dark : .light)
    }

    private func formatTime(_ date: Date) -> String {
        let fmt = DateFormatter()
        if Calendar.current.isDateInToday(date) {
            fmt.dateFormat = "h:mm a"
        } else {
            fmt.dateFormat = "MMM d, h:mm a"
        }
        return fmt.string(from: date)
    }
}
