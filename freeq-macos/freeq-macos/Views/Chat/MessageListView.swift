import SwiftUI

struct MessageListView: View {
    @Environment(AppState.self) private var appState

    private var messages: [ChatMessage] {
        appState.activeChannelState?.messages ?? []
    }

    var body: some View {
        ScrollViewReader { proxy in
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 0) {
                    // Load more history button
                    if !messages.isEmpty {
                        Button {
                            loadOlderHistory()
                        } label: {
                            HStack {
                                Spacer()
                                Image(systemName: "arrow.up.circle")
                                Text("Load older messages")
                                Spacer()
                            }
                            .font(.caption)
                            .foregroundStyle(.secondary)
                        }
                        .buttonStyle(.plain)
                        .padding(.vertical, 8)
                        .id("load-more")
                    }

                    ForEach(messages) { msg in
                        if !msg.isDeleted {
                            if msg.from.isEmpty {
                                SystemMessageRow(message: msg)
                                    .id(msg.id)
                            } else {
                                MessageRow(message: msg)
                                    .id(msg.id)
                            }
                        }
                    }
                }
                .padding(.vertical, 8)
            }
            .onChange(of: messages.count) { oldCount, newCount in
                // Only auto-scroll if messages were added at the end (not prepended history)
                if newCount > oldCount, let last = messages.last {
                    withAnimation(.easeOut(duration: 0.15)) {
                        proxy.scrollTo(last.id, anchor: .bottom)
                    }
                }
            }
            .onAppear {
                if let last = messages.last {
                    proxy.scrollTo(last.id, anchor: .bottom)
                }
            }
            .onChange(of: appState.scrollToMessageId) { _, newId in
                if let id = newId {
                    withAnimation(.easeInOut(duration: 0.3)) {
                        proxy.scrollTo(id, anchor: .center)
                    }
                    // Flash highlight
                    DispatchQueue.main.asyncAfter(deadline: .now() + 0.5) {
                        appState.scrollToMessageId = nil
                    }
                }
            }
        }
        .background(Color(nsColor: .textBackgroundColor))
    }

    private func loadOlderHistory() {
        guard let target = appState.activeChannel,
              let oldest = messages.first else { return }
        appState.requestHistory(channel: target, before: oldest.timestamp)
    }
}

// MARK: - System Messages (join/part/quit/kick)

struct SystemMessageRow: View {
    let message: ChatMessage
    @AppStorage("freeq.showJoinPart") private var showJoinPart = true

    var body: some View {
        if showJoinPart {
            HStack(spacing: 4) {
                Image(systemName: systemIcon)
                    .font(.caption2)
                    .foregroundStyle(.tertiary)
                Text(message.text)
                    .font(.caption)
                    .foregroundStyle(.tertiary)
                Text(formatTime(message.timestamp))
                    .font(.caption2)
                    .foregroundStyle(.quaternary)
            }
            .padding(.horizontal, 16)
            .padding(.vertical, 2)
            .frame(maxWidth: .infinity, alignment: .leading)
        }
    }

    private var systemIcon: String {
        if message.text.contains("joined") { return "arrow.right.circle" }
        if message.text.contains("left") || message.text.contains("quit") { return "arrow.left.circle" }
        if message.text.contains("kicked") { return "xmark.circle" }
        return "info.circle"
    }
}

// MARK: - Message Row

struct MessageRow: View {
    @Environment(AppState.self) private var appState
    @AppStorage("freeq.compactMode") private var compactMode = false
    let message: ChatMessage
    @State private var isHovered = false

    private var isSelf: Bool {
        message.from.lowercased() == appState.nick.lowercased()
    }

    private var isSystem: Bool {
        message.from == "server" || message.from == "system"
    }

    private var showHeader: Bool {
        guard let ch = appState.activeChannelState,
              let idx = ch.messages.firstIndex(where: { $0.id == message.id }),
              idx > 0 else { return true }
        let prev = ch.messages[idx - 1]
        if prev.from.isEmpty { return true }  // After system message
        if prev.from != message.from { return true }
        return message.timestamp.timeIntervalSince(prev.timestamp) > 300
    }

    private var profile: ProfileCache.Profile? {
        ProfileCache.shared.profile(for: message.from)
    }

    private var hasDid: Bool {
        ProfileCache.shared.did(for: message.from) != nil
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            if compactMode {
                // Compact: inline nick + time + text on one line
                HStack(alignment: .firstTextBaseline, spacing: 4) {
                    Text(formatTime(message.timestamp))
                        .font(.system(size: 10, design: .monospaced))
                        .foregroundStyle(.quaternary)
                        .frame(width: 36, alignment: .trailing)
                    Text(message.from)
                        .font(.system(.caption, weight: .bold))
                        .foregroundStyle(isSystem ? .secondary : Theme.nickColor(for: message.from))
                    if hasDid {
                        Image(systemName: "checkmark.seal.fill")
                            .font(.system(size: 8))
                            .foregroundStyle(.blue)
                    }
                }
            } else if showHeader {
                HStack(alignment: .top, spacing: 8) {
                    if !isSystem {
                        AvatarView(nick: message.from, size: 24)
                            .padding(.top, 2)
                    }
                    VStack(alignment: .leading, spacing: 0) {
                        HStack(alignment: .firstTextBaseline, spacing: 4) {
                            if let displayName = profile?.displayName, !displayName.isEmpty {
                                Text(displayName)
                                    .font(.system(.body, weight: .semibold))
                                    .foregroundStyle(Theme.nickColor(for: message.from))
                                Text(message.from)
                                    .font(.caption)
                                    .foregroundStyle(.tertiary)
                            } else {
                                Text(message.from)
                                    .font(.system(.body, weight: .semibold))
                                    .foregroundStyle(isSystem ? .secondary : Theme.nickColor(for: message.from))
                            }

                            if hasDid {
                                Image(systemName: "checkmark.seal.fill")
                                    .font(.caption2)
                                    .foregroundStyle(.blue)
                                    .help("AT Protocol verified identity")
                            }

                            if message.isSigned {
                                Image(systemName: "lock.fill")
                                    .font(.system(size: 9))
                                    .foregroundStyle(.green)
                                    .help("Cryptographically signed message")
                            }

                            Text(formatTime(message.timestamp))
                                .font(.caption)
                                .foregroundStyle(.tertiary)
                                .help(fullTimestamp(message.timestamp))

                            if message.isEdited {
                                Text("(edited)")
                                    .font(.caption2)
                                    .foregroundStyle(.tertiary)
                            }
                        }
                    }
                }
                .padding(.top, 6)
            }

            // Reply indicator (click → scroll + option to open thread)
            if let replyTo = message.replyTo {
                Button {
                    appState.scrollToMessageId = replyTo
                    // Also open the thread if the original message exists
                    if let original = appState.activeChannelState?.messages.first(where: { $0.id == replyTo }) {
                        appState.threadRootMessage = original
                    }
                } label: {
                    HStack(spacing: 4) {
                        Image(systemName: "arrowshape.turn.up.left.fill")
                            .font(.caption2)
                        if let original = appState.activeChannelState?.messages.first(where: { $0.id == replyTo }) {
                            Text("\(original.from): \(original.text)")
                                .font(.caption2)
                                .lineLimit(1)
                        } else {
                            Text("replying to message")
                                .font(.caption2)
                        }
                    }
                    .foregroundStyle(.secondary)
                    .padding(.leading, 2)
                }
                .buttonStyle(.plain)
            }

            // Message text + media
            if message.isAction {
                Text("• \(message.from) \(message.text)")
                    .italic()
                    .foregroundStyle(.secondary)
                    .textSelection(.enabled)
            } else if isSystem {
                Text(message.text)
                    .font(.system(.body, design: .monospaced).weight(.light))
                    .foregroundStyle(.secondary)
                    .textSelection(.enabled)
            } else {
                let imageURLs = extractImageURLs(from: message.text)
                let ytId = extractYouTubeID(from: message.text)
                let cleanText = imageURLs.isEmpty ? message.text : textWithoutImages(message.text, imageURLs: imageURLs)

                if !cleanText.isEmpty {
                    Text(parseMessageText(cleanText))
                        .textSelection(.enabled)
                }

                // Inline images
                if !imageURLs.isEmpty {
                    ForEach(imageURLs, id: \.self) { url in
                        InlineImageView(url: url)
                    }
                }

                // Bluesky post embed
                if let bsky = extractBskyPost(from: message.text) {
                    BlueskyEmbed(handle: bsky.handle, rkey: bsky.rkey)
                }

                // YouTube embed
                if let ytId {
                    YouTubeThumbnail(videoId: ytId)
                }

                // Link preview (only if no images/YouTube/Bluesky)
                if imageURLs.isEmpty && ytId == nil && extractBskyPost(from: message.text) == nil,
                   let url = extractFirstURL(from: message.text) {
                    LinkPreviewView(url: url)
                }
            }

            // Reactions
            if !message.reactions.isEmpty {
                FlowLayout(spacing: 4) {
                    ForEach(Array(message.reactions.keys.sorted()), id: \.self) { emoji in
                        if let nicks = message.reactions[emoji] {
                            ReactionBadge(
                                emoji: emoji,
                                count: nicks.count,
                                isSelfReacted: nicks.contains(appState.nick),
                                action: {
                                    if let target = appState.activeChannel {
                                        appState.sendReaction(target: target, msgId: message.id, emoji: emoji)
                                    }
                                }
                            )
                        }
                    }
                }
                .padding(.top, 4)
            }
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 1)
        .frame(maxWidth: .infinity, alignment: .leading)
        .contentShape(Rectangle())
        .background(
            appState.scrollToMessageId == message.id
                ? Color.accentColor.opacity(0.1)
                : isHovered ? Color(nsColor: .textBackgroundColor).opacity(0.5) : Color.clear
        )
        .onHover { isHovered = $0 }
        .overlay(alignment: .topTrailing) {
            if isHovered && !isSystem {
                HoverActionBar(message: message)
                    .padding(.trailing, 8)
                    .offset(y: -12)
            }
        }
        .contextMenu { messageContextMenu }
    }

    @ViewBuilder
    private var messageContextMenu: some View {
        // React
        if !isSystem {
            Menu("React") {
                ForEach(["👍", "❤️", "😂", "🎉", "👀", "🔥"], id: \.self) { emoji in
                    Button(emoji) {
                        if let target = appState.activeChannel {
                            appState.sendReaction(target: target, msgId: message.id, emoji: emoji)
                        }
                    }
                }
            }
        }

        // Reply
        if !isSystem {
            Button("Reply") {
                appState.replyingToMessage = message
            }
        }

        Button("Copy Text") {
            NSPasteboard.general.clearContents()
            NSPasteboard.general.setString(message.text, forType: .string)
        }

        if !isSystem {
            Button("Open Thread") {
                appState.threadRootMessage = message
            }
        }

        if let msgId = Optional(message.id) {
            Button("Copy Message ID") {
                NSPasteboard.general.clearContents()
                NSPasteboard.general.setString(msgId, forType: .string)
            }
        }

        if !isSystem {
            if appState.bookmarks.contains(where: { $0.msgId == message.id }) {
                Button("Remove Bookmark") {
                    appState.removeBookmark(msgId: message.id)
                }
            } else {
                Button("Bookmark") {
                    if let target = appState.activeChannel {
                        appState.addBookmark(channel: target, msg: message)
                    }
                }
            }
            if let target = appState.activeChannel, target.hasPrefix("#") {
                let isPinned = appState.activeChannelState?.pinnedMessages.contains(where: { $0.id == message.id }) ?? false
                Button(isPinned ? "Unpin Message" : "Pin Message") {
                    appState.sendRaw("\(isPinned ? "UNPIN" : "PIN") \(target) \(message.id)")
                }
            }
        }

        if isSelf {
            Divider()
            Button("Edit") {
                appState.editingMessageId = message.id
                appState.editingText = message.text
            }
            Button("Delete", role: .destructive) {
                if let target = appState.activeChannel {
                    appState.deleteMessage(target: target, msgId: message.id)
                }
            }
        }
    }

    /// Parse message text into AttributedString with formatting.
    private func parseMessageText(_ text: String) -> AttributedString {
        var result = AttributedString(text)

        // URLs
        let detector = try? NSDataDetector(types: NSTextCheckingResult.CheckingType.link.rawValue)
        if let matches = detector?.matches(in: text, range: NSRange(text.startIndex..., in: text)) {
            for match in matches.reversed() {
                guard let range = Range(match.range, in: text),
                      let attrRange = Range(range, in: result),
                      let url = match.url else { continue }
                result[attrRange].link = url
                result[attrRange].foregroundColor = .accentColor
            }
        }

        // Bold: **text**
        result = applyFormatting(result, pattern: "\\*\\*(.+?)\\*\\*", text: text) { attributed, range in
            attributed[range].inlinePresentationIntent = .stronglyEmphasized
        }

        // Italic: *text*
        result = applyFormatting(result, pattern: "(?<![*])\\*([^*]+?)\\*(?![*])", text: text) { attributed, range in
            attributed[range].inlinePresentationIntent = .emphasized
        }

        // Code: `text`
        result = applyFormatting(result, pattern: "`([^`]+?)`", text: text) { attributed, range in
            attributed[range].font = .system(.body, design: .monospaced)
            attributed[range].backgroundColor = Color(nsColor: .quaternaryLabelColor)
        }

        return result
    }

    private func applyFormatting(
        _ attributed: AttributedString,
        pattern: String,
        text: String,
        apply: (inout AttributedString, Range<AttributedString.Index>) -> Void
    ) -> AttributedString {
        var result = attributed
        guard let regex = try? NSRegularExpression(pattern: pattern) else { return result }
        let matches = regex.matches(in: text, range: NSRange(text.startIndex..., in: text))
        for match in matches {
            guard let range = Range(match.range, in: text),
                  let attrRange = Range(range, in: result) else { continue }
            apply(&result, attrRange)
        }
        return result
    }
}

// MARK: - Hover Action Bar (Slack/Discord style)

struct HoverActionBar: View {
    @Environment(AppState.self) private var appState
    let message: ChatMessage
    @State private var showEmojiPicker = false

    private let quickEmoji = ["👍", "❤️", "😂", "🎉", "👀", "🔥"]

    var body: some View {
        HStack(spacing: 2) {
            ForEach(quickEmoji, id: \.self) { emoji in
                Button {
                    if let target = appState.activeChannel {
                        appState.sendReaction(target: target, msgId: message.id, emoji: emoji)
                    }
                } label: {
                    Text(emoji)
                        .font(.system(size: 14))
                        .frame(width: 28, height: 26)
                }
                .buttonStyle(.plain)
                .help("React with \(emoji)")
            }

            Divider().frame(height: 16)

            // Reply
            Button {
                appState.replyingToMessage = message
            } label: {
                Image(systemName: "arrowshape.turn.up.left")
                    .font(.system(size: 11))
                    .frame(width: 28, height: 26)
                    .foregroundStyle(.secondary)
            }
            .buttonStyle(.plain)
            .help("Reply")

            // Thread
            Button {
                appState.threadRootMessage = message
            } label: {
                Image(systemName: "bubble.left.and.bubble.right")
                    .font(.system(size: 11))
                    .frame(width: 28, height: 26)
                    .foregroundStyle(.secondary)
            }
            .buttonStyle(.plain)
            .help("Open Thread")

            // More emoji (opens system picker)
            Button {
                NSApp.orderFrontCharacterPalette(nil)
            } label: {
                Image(systemName: "face.smiling")
                    .font(.system(size: 11))
                    .frame(width: 28, height: 26)
                    .foregroundStyle(.secondary)
            }
            .buttonStyle(.plain)
            .help("More emoji…")
        }
        .padding(.horizontal, 4)
        .padding(.vertical, 2)
        .background(.regularMaterial)
        .clipShape(RoundedRectangle(cornerRadius: 6))
        .shadow(color: .black.opacity(0.1), radius: 4, y: 2)
    }
}

// MARK: - Reaction Badge

struct ReactionBadge: View {
    let emoji: String
    let count: Int
    let isSelfReacted: Bool
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            HStack(spacing: 3) {
                Text(emoji)
                    .font(.caption)
                if count > 1 {
                    Text("\(count)")
                        .font(.caption2.weight(.medium))
                        .foregroundColor(isSelfReacted ? .accentColor : .secondary)
                }
            }
            .padding(.horizontal, 6)
            .padding(.vertical, 2)
            .background(
                RoundedRectangle(cornerRadius: 6)
                    .fill(isSelfReacted ? Color.accentColor.opacity(0.15) : Color(nsColor: .quaternaryLabelColor))
            )
            .overlay(
                RoundedRectangle(cornerRadius: 6)
                    .strokeBorder(isSelfReacted ? Color.accentColor.opacity(0.3) : .clear, lineWidth: 1)
            )
        }
        .buttonStyle(.plain)
    }
}

// MARK: - Flow Layout for reactions

struct FlowLayout: Layout {
    var spacing: CGFloat = 4

    func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) -> CGSize {
        let maxWidth = proposal.width ?? .infinity
        var x: CGFloat = 0
        var y: CGFloat = 0
        var rowHeight: CGFloat = 0

        for subview in subviews {
            let size = subview.sizeThatFits(.unspecified)
            if x + size.width > maxWidth && x > 0 {
                x = 0
                y += rowHeight + spacing
                rowHeight = 0
            }
            x += size.width + spacing
            rowHeight = max(rowHeight, size.height)
        }
        return CGSize(width: maxWidth, height: y + rowHeight)
    }

    func placeSubviews(in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) {
        var x = bounds.minX
        var y = bounds.minY
        var rowHeight: CGFloat = 0

        for subview in subviews {
            let size = subview.sizeThatFits(.unspecified)
            if x + size.width > bounds.maxX && x > bounds.minX {
                x = bounds.minX
                y += rowHeight + spacing
                rowHeight = 0
            }
            subview.place(at: CGPoint(x: x, y: y), proposal: .unspecified)
            x += size.width + spacing
            rowHeight = max(rowHeight, size.height)
        }
    }
}

// MARK: - Full timestamp for hover

private func fullTimestamp(_ date: Date) -> String {
    let formatter = DateFormatter()
    formatter.dateFormat = "EEEE, MMM d, yyyy 'at' HH:mm:ss"
    return formatter.string(from: date)
}

// MARK: - URL extraction

private func extractFirstURL(from text: String) -> String? {
    let detector = try? NSDataDetector(types: NSTextCheckingResult.CheckingType.link.rawValue)
    if let match = detector?.firstMatch(in: text, range: NSRange(text.startIndex..., in: text)),
       let range = Range(match.range, in: text) {
        return String(text[range])
    }
    return nil
}

// MARK: - Time formatting (shared)

func formatTime(_ date: Date) -> String {
    let formatter = DateFormatter()
    let calendar = Calendar.current
    if calendar.isDateInToday(date) {
        formatter.dateFormat = "HH:mm"
    } else {
        formatter.dateFormat = "MMM d, HH:mm"
    }
    return formatter.string(from: date)
}
