import SwiftUI

struct MessageListView: View {
    @Environment(AppState.self) private var appState
    let channel: ChannelState?
    /// Which channel we last rendered for. A channel switch should snap
    /// the scroll to the bottom (no animation, no visible "scroll from
    /// top down" sweep). Only subsequent incremental adds in the SAME
    /// channel get the gentle scroll animation.
    @State private var lastRenderedChannel: String?

    private var messages: [ChatMessage] {
        channel?.messages ?? []
    }

    private var visibleMessages: [ChatMessage] {
        MessageVisibility.visibleMessages(from: messages)
    }

    private var shouldShowWelcome: Bool {
        MessageVisibility.shouldShowWelcome(messages: messages)
    }

    /// Stable sentinel ID for the bottom anchor — scrolling to a fixed
    /// invisible spacer below the last row is more reliable than
    /// scrolling to the last message's ID, because the message ID changes
    /// every time a new one arrives (forcing layout) and SwiftUI's
    /// LazyVStack sometimes lays out the last row partially below the
    /// viewport on first measurement.
    private let bottomAnchorID = "__bottom"

    var body: some View {
        ZStack {
            ScrollViewReader { proxy in
                ScrollView {
                    LazyVStack(alignment: .leading, spacing: 0) {
                        // Load more history button
                        if !visibleMessages.isEmpty {
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
                                .foregroundStyle(Theme.textSecondary)
                            }
                            .buttonStyle(.plain)
                            .padding(.vertical, 8)
                            .background(
                                Capsule()
                                    .fill(Theme.surfaceSoft)
                                    .padding(.horizontal, 220)
                            )
                            .id("load-more")
                        }

                        ForEach(visibleMessages) { msg in
                            if msg.from.isEmpty {
                                SystemMessageRow(message: msg)
                                    .id(msg.id)
                            } else {
                                MessageRow(message: msg)
                                    .id(msg.id)
                            }
                        }

                        // Invisible bottom-of-list anchor. Reserved height
                        // gives the last real message room so it doesn't hug
                        // the divider above the compose bar (the "half off
                        // the bottom" symptom was the last row landing at
                        // the very edge of the scroll viewport with no breathing
                        // room).
                        Color.clear
                            .frame(height: 12)
                            .id(bottomAnchorID)
                    }
                    .padding(.top, 8)
                }
                .onChange(of: visibleMessages.count) { oldCount, newCount in
                    // If this count change is the initial load for a newly-
                    // selected channel, snap with no animation — otherwise
                    // the user sees a fast visual scroll from top to bottom
                    // as the rows render.
                    let isInitialLoadForCurrentChannel =
                        lastRenderedChannel != appState.activeChannel
                    guard newCount > 0 else { return }
                    if isInitialLoadForCurrentChannel {
                        proxy.scrollTo(bottomAnchorID, anchor: .bottom)
                        lastRenderedChannel = appState.activeChannel
                    } else if newCount > oldCount {
                        withAnimation(.easeOut(duration: 0.15)) {
                            proxy.scrollTo(bottomAnchorID, anchor: .bottom)
                        }
                    }
                }
                .onChange(of: appState.activeChannel) { _, _ in
                    // Channel switch — snap to bottom on the next runloop
                    // tick so the LazyVStack has populated its rows. Without
                    // the deferral, scrollTo runs against an empty stack
                    // and the view appears at the top, then the count-onChange
                    // visibly catches up by animating down.
                    DispatchQueue.main.async {
                        proxy.scrollTo(bottomAnchorID, anchor: .bottom)
                        lastRenderedChannel = appState.activeChannel
                    }
                }
                .onAppear {
                    // First mount — snap immediately, no animation.
                    proxy.scrollTo(bottomAnchorID, anchor: .bottom)
                    lastRenderedChannel = appState.activeChannel
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

            if shouldShowWelcome {
                ChannelWelcomeView()
            }
        }
        .background(Theme.chatBackground)
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
                    .foregroundStyle(Theme.textTertiary)
                Text(message.text)
                    .font(.caption)
                    .foregroundStyle(Theme.textSecondary)
                Text(formatTime(message.timestamp))
                    .font(.caption2)
                    .foregroundStyle(Theme.textTertiary.opacity(0.75))
            }
            .padding(.horizontal, 10)
            .padding(.vertical, 4)
            .background(Capsule().fill(Theme.systemPill))
            .padding(.horizontal, 16)
            .padding(.vertical, 3)
            .frame(maxWidth: .infinity, alignment: .center)
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
        // Break across a provenance boundary: a federated message (origin set)
        // must not collapse under a local sender's header.
        if prev.origin != message.origin { return true }
        return message.timestamp.timeIntervalSince(prev.timestamp) > 300
    }

    private var profile: ProfileCache.Profile? {
        ProfileCache.shared.profile(for: message.from)
    }

    private var hasDid: Bool {
        // A federated message is peer-vouched, not verified here — withhold the
        // local verified badge even if a same-nick local member has a DID.
        message.origin == nil && ProfileCache.shared.did(for: message.from) != nil
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            if compactMode {
                // Compact: inline nick + time + text on one line
                HStack(alignment: .firstTextBaseline, spacing: 4) {
                    Text(formatTime(message.timestamp))
                        .font(.system(size: 10, design: .monospaced))
                        .foregroundStyle(Theme.textTertiary.opacity(0.75))
                        .frame(width: 36, alignment: .trailing)
                    Text(message.from)
                        .font(.system(.caption, weight: .bold))
                        .foregroundStyle(isSystem ? Theme.textSecondary : Theme.nickColor(for: message.from))
                    if hasDid {
                        Image(systemName: "checkmark.seal.fill")
                            .font(.system(size: 8))
                            .foregroundStyle(Theme.verified)
                    }
                    if let origin = message.origin {
                        Text("via \(origin)")
                            .font(.system(size: 9))
                            .foregroundStyle(Theme.textTertiary)
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
                                    .foregroundStyle(Theme.textTertiary)
                            } else {
                                Text(message.from)
                                    .font(.system(.body, weight: .semibold))
                                    .foregroundStyle(isSystem ? Theme.textSecondary : Theme.nickColor(for: message.from))
                            }

                            if hasDid {
                                Image(systemName: "checkmark.seal.fill")
                                    .font(.caption2)
                                    .foregroundStyle(Theme.verified)
                                    .help("AT Protocol verified identity")
                            }

                            if message.origin == nil && message.isSigned {
                                Image(systemName: "lock.fill")
                                    .font(.system(size: 9))
                                    .foregroundStyle(Theme.success)
                                    .help("Cryptographically signed message")
                            }

                            // Federated: relayed from another server — peer-vouched,
                            // not verified here. Show provenance instead of the local
                            // verified/signed badges (which would overstate trust).
                            if let origin = message.origin {
                                Text("via \(origin)")
                                    .font(.caption2)
                                    .foregroundStyle(Theme.textTertiary)
                                    .help("Relayed from \(origin). This server didn't verify the sender — \(origin) vouches for it.")
                            }

                            Text(formatTime(message.timestamp))
                                .font(.caption)
                                .foregroundStyle(Theme.textTertiary)
                                .help(fullTimestamp(message.timestamp))

                            if message.isEdited {
                                Text("(edited)")
                                    .font(.caption2)
                                    .foregroundStyle(Theme.textTertiary)
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
                    .foregroundStyle(Theme.textSecondary)
                    .padding(.leading, 2)
                }
                .buttonStyle(.plain)
            }

            // Message text + media
            if message.isAction {
                Text("• \(message.from) \(message.text)")
                    .italic()
                    .foregroundStyle(Theme.textSecondary)
                    .textSelection(.enabled)
            } else if isSystem {
                Text(message.text)
                    .font(.system(.body, design: .monospaced).weight(.light))
                    .foregroundStyle(Theme.textSecondary)
                    .textSelection(.enabled)
            } else {
                let imageURLs = extractImageURLs(from: message.text)
                let videoURLs = extractVideoURLs(from: message.text)
                let audioURLs = extractAudioURLs(from: message.text)
                let ytId = extractYouTubeID(from: message.text)
                let isVoice = isVoiceMessage(message.text)
                let mediaURLs = imageURLs + videoURLs + audioURLs
                let cleanText = mediaURLs.isEmpty ? message.text : textWithoutImages(message.text, imageURLs: mediaURLs)

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

                // Inline video
                if !videoURLs.isEmpty {
                    ForEach(videoURLs, id: \.self) { url in
                        InlineVideoView(url: url)
                    }
                }

                // Inline audio / voice messages
                if !audioURLs.isEmpty {
                    ForEach(audioURLs, id: \.self) { url in
                        InlineAudioView(url: url, isVoice: isVoice)
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

                // Link preview (only if no other media)
                if mediaURLs.isEmpty && ytId == nil && extractBskyPost(from: message.text) == nil,
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
                ? Theme.accent.opacity(0.10)
                : isHovered ? Theme.surfaceSoft.opacity(0.80) : Color.clear
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
                    if isPinned { appState.unpin(msgId: message.id, in: target) }
                    else { appState.pin(msgId: message.id, in: target) }
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
        // Parse inline markdown (**bold**, *italic*, _italic_, `code`,
        // ~~strike~~, [label](url)) — this STRIPS the delimiters so they don't
        // show literally, matching the web/iOS clients. Falls back to plain
        // text if the string isn't valid markdown.
        var options = AttributedString.MarkdownParsingOptions()
        options.interpretedSyntax = .inlineOnlyPreservingWhitespace
        options.failurePolicy = .returnPartiallyParsedIfPossible
        var result = (try? AttributedString(markdown: text, options: options)) ?? AttributedString(text)

        // Give inline code a monospaced look (markdown marks it with an
        // inlinePresentationIntent but applies no visible style on its own).
        for run in result.runs where run.inlinePresentationIntent?.contains(.code) == true {
            result[run.range].font = .system(.body, design: .monospaced)
            result[run.range].backgroundColor = Color(nsColor: .quaternaryLabelColor)
        }

        // Markdown only links [label](url) / <url>; detect bare URLs too, on the
        // delimiter-stripped plain text so indices line up.
        let plain = String(result.characters)
        let detector = try? NSDataDetector(types: NSTextCheckingResult.CheckingType.link.rawValue)
        if let matches = detector?.matches(in: plain, range: NSRange(plain.startIndex..., in: plain)) {
            for match in matches.reversed() {
                guard let r = Range(match.range, in: plain),
                      let attrRange = Range(r, in: result),
                      let url = match.url else { continue }
                if result[attrRange].link == nil { result[attrRange].link = url }
            }
        }

        // Color every link with the accent.
        for run in result.runs where run.link != nil {
            result[run.range].foregroundColor = .accentColor
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
    formatter.locale = .current
    formatter.dateStyle = .full
    formatter.timeStyle = .medium
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
    formatter.locale = .current
    let calendar = Calendar.current
    if calendar.isDateInToday(date) {
        formatter.dateStyle = .none
        formatter.timeStyle = .short
    } else {
        formatter.dateStyle = .medium
        formatter.timeStyle = .short
    }
    return formatter.string(from: date)
}
