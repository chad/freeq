import SwiftUI

struct MessageListView: View {
    @EnvironmentObject var appState: AppState
    @ObservedObject var channel: ChannelState
    @State private var emojiPickerMessage: ChatMessage? = nil
    @State private var profileNick: String? = nil
    @State private var threadMessage: ChatMessage? = nil
    @StateObject private var avatarCache = AvatarCache.shared

    @State private var showScrollButton = false
    @State private var lastReadId: String? = nil

    var body: some View {
        ScrollViewReader { proxy in
            ZStack(alignment: .bottom) {
                ScrollView {
                    // Pull to load older messages
                    Button(action: {
                        let oldest = channel.messages.first?.timestamp
                        appState.requestHistory(channel: channel.name, before: oldest)
                        UIImpactFeedbackGenerator(style: .light).impactOccurred()
                    }) {
                        HStack(spacing: 6) {
                            Image(systemName: "arrow.up.circle")
                                .font(.system(size: 13))
                            Text("Load older messages")
                                .font(.system(size: 13))
                        }
                        .foregroundColor(Theme.textMuted)
                        .frame(maxWidth: .infinity)
                        .padding(.vertical, 8)
                    }
                    .buttonStyle(.plain)

                    LazyVStack(alignment: .leading, spacing: 0) {
                        ForEach(Array(channel.messages.enumerated()), id: \.element.id) { idx, msg in
                            let showHeader = shouldShowHeader(at: idx)
                            let showDate = shouldShowDateSeparator(at: idx)

                            if showDate {
                                dateSeparator(for: msg.timestamp)
                            }

                            // Unread separator
                            if let readId = lastReadId, idx > 0,
                               channel.messages[idx - 1].id == readId,
                               msg.from.lowercased() != appState.nick.lowercased() {
                                unreadSeparator
                            }

                            if msg.from.isEmpty {
                                systemMessage(msg)
                            } else if msg.isDeleted {
                                deletedMessage(msg, showHeader: showHeader)
                            } else {
                                messageRow(msg, showHeader: showHeader)
                                    .swipeActions(edge: .leading, allowsFullSwipe: true) {
                                        Button {
                                            appState.replyingTo = msg
                                            UIImpactFeedbackGenerator(style: .light).impactOccurred()
                                        } label: {
                                            Label("Reply", systemImage: "arrowshape.turn.up.left")
                                        }
                                        .tint(Theme.accent)
                                    }
                                    .contextMenu { messageContextMenu(msg) }
                            }
                        }
                    }
                    .padding(.top, 8)
                    .padding(.bottom, 4)

                    // Typing indicator
                    if !channel.activeTypers.isEmpty {
                        typingIndicator
                            .padding(.horizontal, 16)
                            .padding(.bottom, 4)
                    }

                    // Invisible anchor for scroll detection
                    GeometryReader { geo in
                        Color.clear
                            .preference(key: ScrollOffsetKey.self, value: geo.frame(in: .global).minY)
                    }
                    .frame(height: 1)
                    .id("bottom-anchor")
                }
                .background(Theme.bgPrimary)
                .onPreferenceChange(ScrollOffsetKey.self) { value in
                    // value is the minY of the bottom anchor in global coords
                    // When at bottom, it's near screen height; when scrolled up, it goes large/positive
                    let screenHeight = UIScreen.main.bounds.height
                    // If the bottom anchor is more than 150pt below the screen, user has scrolled up
                    showScrollButton = value > screenHeight + 150
                }

                // Scroll to bottom button
                if showScrollButton {
                    Button(action: {
                        if let last = channel.messages.last {
                            withAnimation(.easeOut(duration: 0.2)) {
                                proxy.scrollTo(last.id, anchor: .bottom)
                            }
                        }
                        UIImpactFeedbackGenerator(style: .light).impactOccurred()
                    }) {
                        HStack(spacing: 6) {
                            Image(systemName: "chevron.down")
                                .font(.system(size: 12, weight: .bold))
                            Text("New messages")
                                .font(.system(size: 13, weight: .medium))
                        }
                        .foregroundColor(Theme.accent)
                        .padding(.horizontal, 16)
                        .padding(.vertical, 8)
                        .background(.ultraThinMaterial)
                        .cornerRadius(20)
                        .shadow(color: .black.opacity(0.3), radius: 8, y: 4)
                    }
                    .padding(.bottom, 8)
                    .transition(.move(edge: .bottom).combined(with: .opacity))
                    .animation(.spring(response: 0.3), value: showScrollButton)
                }
            }
            .onChange(of: channel.messages.count) {
                // Auto-scroll on new messages if already near bottom
                if !showScrollButton, let last = channel.messages.last {
                    withAnimation(.easeOut(duration: 0.15)) {
                        proxy.scrollTo(last.id, anchor: .bottom)
                    }
                }
                // Mark read if this is the active channel
                if appState.activeChannel == channel.name {
                    appState.markRead(channel.name)
                }
            }
            .onAppear {
                // Capture current read position before marking read
                lastReadId = UserDefaults.standard.string(forKey: "freeq.lastRead.\(channel.name)")
                appState.markRead(channel.name)
                scrollToBottom(proxy: proxy)
            }
            .onChange(of: appState.activeChannel) {
                if appState.activeChannel == channel.name {
                    appState.markRead(channel.name)
                    scrollToBottom(proxy: proxy)
                }
            }
        }
        .sheet(item: $emojiPickerMessage) { msg in
            EmojiPickerSheet(message: msg, channel: channel.name)
                .presentationDetents([.medium])
                .presentationDragIndicator(.visible)
        }
        .sheet(item: Binding(
            get: { profileNick.map { ProfileNickTarget(nick: $0) } },
            set: { profileNick = $0?.nick }
        )) { target in
            UserProfileSheet(nick: target.nick)
                .presentationDetents([.medium, .large])
                .presentationDragIndicator(.visible)
        }
        .sheet(item: $threadMessage) { msg in
            ThreadView(rootMessage: msg, channelName: channel.name)
                .presentationDetents([.large])
                .presentationDragIndicator(.visible)
        }
    }

    // MARK: - Scroll

    private func scrollToBottom(proxy: ScrollViewProxy) {
        // Triple-scroll: immediate + short delay + after CHATHISTORY arrives
        if let last = channel.messages.last {
            proxy.scrollTo(last.id, anchor: .bottom)
        }
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.15) {
            if let last = channel.messages.last {
                proxy.scrollTo(last.id, anchor: .bottom)
            }
        }
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.5) {
            if let last = channel.messages.last {
                proxy.scrollTo(last.id, anchor: .bottom)
            }
        }
    }

    // MARK: - Context Menu

    @ViewBuilder
    private func messageContextMenu(_ msg: ChatMessage) -> some View {
        Button(action: {
            appState.replyingTo = msg
            UIImpactFeedbackGenerator(style: .light).impactOccurred()
        }) {
            Label("Reply", systemImage: "arrowshape.turn.up.left")
        }

        Button(action: {
            threadMessage = msg
        }) {
            Label("Thread", systemImage: "text.bubble")
        }

        Button(action: {
            emojiPickerMessage = msg
        }) {
            Label("React", systemImage: "face.smiling")
        }

        // Quick reactions
        ForEach(["👍", "❤️", "😂", "🎉"], id: \.self) { emoji in
            Button(action: {
                appState.sendReaction(target: channel.name, msgId: msg.id, emoji: emoji)
                UIImpactFeedbackGenerator(style: .light).impactOccurred()
            }) {
                Text(emoji)
            }
        }

        if msg.from.lowercased() == appState.nick.lowercased() {
            Divider()

            Button(action: {
                appState.editingMessage = msg
            }) {
                Label("Edit", systemImage: "pencil")
            }

            Button(role: .destructive, action: {
                appState.deleteMessage(target: channel.name, msgId: msg.id)
                UIImpactFeedbackGenerator(style: .medium).impactOccurred()
            }) {
                Label("Delete", systemImage: "trash")
            }
        }

        Divider()

        Button(action: {
            UIPasteboard.general.string = msg.text
            UINotificationFeedbackGenerator().notificationOccurred(.success)
        }) {
            Label("Copy Text", systemImage: "doc.on.doc")
        }
    }

    // MARK: - Typing Indicator

    private var typingIndicator: some View {
        HStack(spacing: 8) {
            // Animated dots
            HStack(spacing: 3) {
                ForEach(0..<3, id: \.self) { i in
                    Circle()
                        .fill(Theme.textMuted)
                        .frame(width: 6, height: 6)
                        .opacity(0.6)
                }
            }

            let typers = channel.activeTypers
            if typers.count == 1 {
                Text("\(typers[0]) is typing...")
                    .font(.system(size: 12))
                    .foregroundColor(Theme.textMuted)
            } else if typers.count == 2 {
                Text("\(typers[0]) and \(typers[1]) are typing...")
                    .font(.system(size: 12))
                    .foregroundColor(Theme.textMuted)
            } else if typers.count > 2 {
                Text("\(typers.count) people are typing...")
                    .font(.system(size: 12))
                    .foregroundColor(Theme.textMuted)
            }
        }
        .padding(.leading, 68)
    }

    // MARK: - Unread Separator

    private var unreadSeparator: some View {
        HStack(spacing: 8) {
            Rectangle().fill(Color.red.opacity(0.4)).frame(height: 1)
            Text("NEW")
                .font(.system(size: 10, weight: .heavy))
                .foregroundColor(.red.opacity(0.7))
                .tracking(1)
            Rectangle().fill(Color.red.opacity(0.4)).frame(height: 1)
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 6)
    }

    // MARK: - Message Grouping

    private func shouldShowHeader(at idx: Int) -> Bool {
        guard idx > 0 else { return true }
        let prev = channel.messages[idx - 1]
        let curr = channel.messages[idx]
        if curr.from.isEmpty || prev.from.isEmpty { return true }
        if prev.from != curr.from { return true }
        return curr.timestamp.timeIntervalSince(prev.timestamp) > 300
    }

    private func shouldShowDateSeparator(at idx: Int) -> Bool {
        guard idx > 0 else { return true }
        return !Calendar.current.isDate(
            channel.messages[idx - 1].timestamp,
            inSameDayAs: channel.messages[idx].timestamp
        )
    }

    // MARK: - System Messages

    private func dateSeparator(for date: Date) -> some View {
        HStack {
            Rectangle().fill(Theme.border).frame(height: 1)
            Text(formatDate(date))
                .font(.system(size: 11, weight: .semibold))
                .foregroundColor(Theme.textMuted)
                .padding(.horizontal, 8)
            Rectangle().fill(Theme.border).frame(height: 1)
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 8)
    }

    private func systemMessage(_ msg: ChatMessage) -> some View {
        HStack(spacing: 6) {
            Image(systemName: "arrow.right.arrow.left")
                .font(.system(size: 9))
                .foregroundColor(Theme.textMuted)
            Text(msg.text)
                .font(.system(size: 12))
                .foregroundColor(Theme.textMuted)
        }
        .padding(.horizontal, 20)
        .padding(.vertical, 3)
        .frame(maxWidth: .infinity, alignment: .center)
        .id(msg.id)
    }

    private func deletedMessage(_ msg: ChatMessage, showHeader: Bool) -> some View {
        HStack(spacing: 6) {
            if showHeader {
                Spacer().frame(width: 52) // avatar space
            }
            Image(systemName: "trash")
                .font(.system(size: 11))
                .foregroundColor(Theme.textMuted)
            Text("Message deleted")
                .font(.system(size: 13))
                .foregroundColor(Theme.textMuted)
                .italic()
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 2)
        .id(msg.id)
    }

    // MARK: - Message Rows

    @ViewBuilder
    private func messageRow(_ msg: ChatMessage, showHeader: Bool) -> some View {
        VStack(alignment: .leading, spacing: 0) {
            // Reply context — tap to open thread
            if let replyId = msg.replyTo,
               let originalIdx = channel.findMessage(byId: replyId) {
                let original = channel.messages[originalIdx]
                Button(action: { threadMessage = msg }) {
                    replyContext(original)
                }
                .buttonStyle(.plain)
                .padding(.leading, 68)
                .padding(.trailing, 16)
                .padding(.top, 4)
            }

            if showHeader {
                HStack(alignment: .top, spacing: 12) {
                    // Avatar
                    UserAvatar(nick: msg.from, size: 40)

                    VStack(alignment: .leading, spacing: 3) {
                        HStack(alignment: .firstTextBaseline, spacing: 8) {
                            Button(action: { profileNick = msg.from }) {
                                HStack(spacing: 4) {
                                    Text((channel.memberInfo(for: msg.from)?.prefix ?? "") + msg.from)
                                        .font(.system(size: 15, weight: .bold))
                                        .foregroundColor(Theme.nickColor(for: msg.from))

                                    // Verified badge — shown if we have a cached avatar (means Bluesky profile found)
                                    if avatarCache.avatarURL(for: msg.from.lowercased()) != nil {
                                        VerifiedBadge(size: 12)
                                    }
                                }
                            }
                            .buttonStyle(.plain)

                            Text(formatTime(msg.timestamp))
                                .font(.system(size: 11))
                                .foregroundColor(Theme.textMuted)

                            if msg.isEdited {
                                Text("(edited)")
                                    .font(.system(size: 11))
                                    .foregroundColor(Theme.textMuted)
                            }
                        }

                        messageBody(msg)
                    }

                    Spacer(minLength: 0)
                }
                .padding(.horizontal, 16)
                .padding(.top, 6)
                .padding(.bottom, 2)
            } else {
                messageBody(msg)
                    .padding(.leading, 68)
                    .padding(.trailing, 16)
                    .padding(.vertical, 1)
            }

            // Reactions
            if !msg.reactions.isEmpty {
                reactionsView(msg)
                    .padding(.leading, 68)
                    .padding(.trailing, 16)
                    .padding(.top, 4)
            }
        }
        .id(msg.id)
    }

    // MARK: - Reply Context

    private func replyContext(_ original: ChatMessage) -> some View {
        HStack(spacing: 6) {
            Rectangle()
                .fill(Theme.accent)
                .frame(width: 2)

            Image(systemName: "arrowshape.turn.up.left.fill")
                .font(.system(size: 9))
                .foregroundColor(Theme.textMuted)

            Text(original.from)
                .font(.system(size: 12, weight: .semibold))
                .foregroundColor(Theme.nickColor(for: original.from))

            Text(original.text)
                .font(.system(size: 12))
                .foregroundColor(Theme.textMuted)
                .lineLimit(1)
        }
        .padding(.vertical, 4)
        .padding(.horizontal, 8)
        .background(Theme.bgTertiary.opacity(0.5))
        .cornerRadius(4)
    }

    // MARK: - Reactions

    private func reactionsView(_ msg: ChatMessage) -> some View {
        HStack(spacing: 4) {
            ForEach(Array(msg.reactions.keys.sorted()), id: \.self) { emoji in
                let nicks = msg.reactions[emoji] ?? []
                let isMine = nicks.contains(where: { $0.lowercased() == appState.nick.lowercased() })

                Button(action: {
                    appState.sendReaction(target: channel.name, msgId: msg.id, emoji: emoji)
                    UIImpactFeedbackGenerator(style: .light).impactOccurred()
                }) {
                    HStack(spacing: 3) {
                        Text(emoji)
                            .font(.system(size: 14))
                        if nicks.count > 1 {
                            Text("\(nicks.count)")
                                .font(.system(size: 11, weight: .medium))
                                .foregroundColor(isMine ? Theme.accent : Theme.textSecondary)
                        }
                    }
                    .padding(.horizontal, 6)
                    .padding(.vertical, 3)
                    .background(isMine ? Theme.accent.opacity(0.15) : Theme.bgTertiary)
                    .cornerRadius(6)
                    .overlay(
                        RoundedRectangle(cornerRadius: 6)
                            .stroke(isMine ? Theme.accent.opacity(0.4) : Color.clear, lineWidth: 1)
                    )
                }
                .buttonStyle(.plain)
            }
        }
    }

    // MARK: - Message Body

    // Bluesky URL pattern: bsky.app/profile/{handle}/post/{rkey}
    private static let bskyPattern = try! NSRegularExpression(
        pattern: #"https?://bsky\.app/profile/([^/]+)/post/([a-zA-Z0-9]+)"#
    )
    // YouTube URL pattern
    private static let ytPattern = try! NSRegularExpression(
        pattern: #"(?:youtube\.com/watch\?v=|youtu\.be/)([a-zA-Z0-9_-]{11})"#
    )

    @ViewBuilder
    private func messageBody(_ msg: ChatMessage) -> some View {
        if msg.isAction {
            Text("*\(msg.from) \(msg.text)*")
                .font(.system(size: 15))
                .italic()
                .foregroundColor(Theme.textSecondary)
        } else if let url = extractImageURL(msg.text) {
            VStack(alignment: .leading, spacing: 6) {
                let remainingText = msg.text.replacingOccurrences(of: url.absoluteString, with: "").trimmingCharacters(in: .whitespaces)
                if !remainingText.isEmpty {
                    styledText(remainingText)
                }
                AsyncImage(url: url) { phase in
                    switch phase {
                    case .success(let image):
                        image
                            .resizable()
                            .aspectRatio(contentMode: .fit)
                            .frame(maxWidth: 280, maxHeight: 280)
                            .cornerRadius(8)
                            .onTapGesture {
                                appState.lightboxURL = url
                                UIImpactFeedbackGenerator(style: .light).impactOccurred()
                            }
                    case .failure:
                        linkButton(url)
                    default:
                        RoundedRectangle(cornerRadius: 8)
                            .fill(Theme.bgTertiary)
                            .frame(width: 200, height: 120)
                            .overlay(ProgressView().tint(Theme.textMuted))
                    }
                }
            }
        } else if let (handle, rkey) = extractBskyPost(msg.text) {
            VStack(alignment: .leading, spacing: 6) {
                styledText(msg.text)
                BlueskyEmbed(handle: handle, rkey: rkey)
            }
        } else if let videoId = extractYouTubeId(msg.text) {
            VStack(alignment: .leading, spacing: 6) {
                styledText(msg.text)
                YouTubeThumb(videoId: videoId)
            }
        } else if let url = extractURL(msg.text) {
            VStack(alignment: .leading, spacing: 6) {
                styledText(msg.text)
                LinkPreviewCard(url: url)
            }
        } else {
            styledText(msg.text)
        }
    }

    private func extractBskyPost(_ text: String) -> (String, String)? {
        let range = NSRange(text.startIndex..., in: text)
        guard let match = Self.bskyPattern.firstMatch(in: text, range: range) else { return nil }
        guard let handleRange = Range(match.range(at: 1), in: text),
              let rkeyRange = Range(match.range(at: 2), in: text) else { return nil }
        return (String(text[handleRange]), String(text[rkeyRange]))
    }

    private func extractYouTubeId(_ text: String) -> String? {
        let range = NSRange(text.startIndex..., in: text)
        guard let match = Self.ytPattern.firstMatch(in: text, range: range) else { return nil }
        guard let idRange = Range(match.range(at: 1), in: text) else { return nil }
        return String(text[idRange])
    }

    private func styledText(_ text: String) -> some View {
        let isMention = text.lowercased().contains(appState.nick.lowercased())
        return Text(attributedMessage(text))
            .font(.system(size: 15))
            .foregroundColor(Theme.textPrimary)
            .textSelection(.enabled)
            .padding(.horizontal, isMention ? 4 : 0)
            .padding(.vertical, isMention ? 2 : 0)
            .background(isMention ? Theme.accent.opacity(0.1) : Color.clear)
            .cornerRadius(4)
    }

    private func linkButton(_ url: URL) -> some View {
        Link(destination: url) {
            HStack(spacing: 6) {
                Image(systemName: "link")
                    .font(.system(size: 11))
                Text(url.host ?? url.absoluteString)
                    .font(.system(size: 13))
                    .lineLimit(1)
            }
            .foregroundColor(Theme.accent)
            .padding(.horizontal, 10)
            .padding(.vertical, 6)
            .background(Theme.accent.opacity(0.1))
            .cornerRadius(6)
        }
    }

    // MARK: - URL Detection

    private func extractImageURL(_ text: String) -> URL? {
        // Match explicit image file extensions
        let extPattern = #"https?://\S+\.(?:png|jpg|jpeg|gif|webp)(?:\?\S*)?"#
        if let range = text.range(of: extPattern, options: .regularExpression) {
            return URL(string: String(text[range]))
        }
        // Match AT Protocol CDN image URLs (cdn.bsky.app/img/...)
        let cdnPattern = #"https?://cdn\.bsky\.app/img/[^\s<]+"#
        if let range = text.range(of: cdnPattern, options: .regularExpression) {
            return URL(string: String(text[range]))
        }
        return nil
    }

    private func extractURL(_ text: String) -> URL? {
        let pattern = #"https?://\S+"#
        guard let range = text.range(of: pattern, options: .regularExpression) else { return nil }
        let urlStr = String(text[range])
        return URL(string: urlStr)
    }

    // MARK: - Styled Text

    private func attributedMessage(_ text: String) -> AttributedString {
        var result = AttributedString(text)

        // Bold: **text**
        let boldPattern = #"\*\*(.+?)\*\*"#
        if let regex = try? NSRegularExpression(pattern: boldPattern),
           let match = regex.firstMatch(in: text, range: NSRange(text.startIndex..., in: text)),
           let range = Range(match.range, in: result) {
            result[range].font = .system(size: 15, weight: .bold)
        }

        // Inline code: `text`
        let codePattern = #"`([^`]+)`"#
        if let regex = try? NSRegularExpression(pattern: codePattern),
           let match = regex.firstMatch(in: text, range: NSRange(text.startIndex..., in: text)),
           let range = Range(match.range, in: result) {
            result[range].font = .system(size: 14, design: .monospaced)
            result[range].backgroundColor = Theme.bgTertiary
        }

        return result
    }

    // MARK: - Formatting

    private func formatTime(_ date: Date) -> String {
        let formatter = DateFormatter()
        formatter.dateFormat = "h:mm a"
        return formatter.string(from: date)
    }

    private func formatDate(_ date: Date) -> String {
        if Calendar.current.isDateInToday(date) { return "Today" }
        if Calendar.current.isDateInYesterday(date) { return "Yesterday" }
        let formatter = DateFormatter()
        formatter.dateFormat = "MMMM d, yyyy"
        return formatter.string(from: date)
    }
}

// MARK: - Emoji Picker Sheet

struct EmojiPickerSheet: View {
    @EnvironmentObject var appState: AppState
    @Environment(\.dismiss) var dismiss
    let message: ChatMessage
    let channel: String

    let commonEmoji = ["👍", "👎", "❤️", "😂", "😮", "😢", "🎉", "🔥",
                       "👀", "💯", "✅", "❌", "🙏", "💪", "🤔", "😍",
                       "🚀", "⭐", "🌈", "🎵", "☕", "🍕", "🐛", "💡"]

    var body: some View {
        VStack(spacing: 16) {
            Text("React to message")
                .font(.system(size: 15, weight: .semibold))
                .foregroundColor(Theme.textPrimary)
                .padding(.top, 8)

            // Original message preview
            HStack(spacing: 8) {
                Text(message.from)
                    .font(.system(size: 13, weight: .bold))
                    .foregroundColor(Theme.nickColor(for: message.from))
                Text(message.text)
                    .font(.system(size: 13))
                    .foregroundColor(Theme.textSecondary)
                    .lineLimit(2)
            }
            .padding(12)
            .background(Theme.bgTertiary)
            .cornerRadius(8)
            .padding(.horizontal, 16)

            // Emoji grid
            LazyVGrid(columns: Array(repeating: GridItem(.flexible()), count: 8), spacing: 8) {
                ForEach(commonEmoji, id: \.self) { emoji in
                    Button(action: {
                        appState.sendReaction(target: channel, msgId: message.id, emoji: emoji)
                        UIImpactFeedbackGenerator(style: .light).impactOccurred()
                        dismiss()
                    }) {
                        Text(emoji)
                            .font(.system(size: 28))
                            .frame(width: 40, height: 40)
                    }
                }
            }
            .padding(.horizontal, 16)

            Spacer()
        }
        .background(Theme.bgPrimary)
        .preferredColorScheme(.dark)
    }
}

// Helper for profile sheet binding
private struct ProfileNickTarget: Identifiable {
    let nick: String
    var id: String { nick }
}

// Preference key for scroll offset detection
private struct ScrollOffsetKey: PreferenceKey {
    static var defaultValue: CGFloat = 0
    static func reduce(value: inout CGFloat, nextValue: () -> CGFloat) {
        value = nextValue()
    }
}
