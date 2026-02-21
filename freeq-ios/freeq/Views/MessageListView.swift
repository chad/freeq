import SwiftUI

struct MessageListView: View {
    @ObservedObject var channel: ChannelState

    var body: some View {
        ScrollViewReader { proxy in
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 0) {
                    ForEach(Array(channel.messages.enumerated()), id: \.element.id) { idx, msg in
                        let showHeader = shouldShowHeader(at: idx)
                        let showDate = shouldShowDateSeparator(at: idx)

                        if showDate {
                            dateSeparator(for: msg.timestamp)
                        }

                        if msg.from.isEmpty {
                            systemMessage(msg)
                        } else {
                            messageRow(msg, showHeader: showHeader)
                        }
                    }
                }
                .padding(.top, 8)
                .padding(.bottom, 4)
            }
            .background(Theme.bgPrimary)
            .onChange(of: channel.messages.count) {
                if let last = channel.messages.last {
                    withAnimation(.easeOut(duration: 0.15)) {
                        proxy.scrollTo(last.id, anchor: .bottom)
                    }
                }
            }
        }
    }

    // Group consecutive messages from same sender within 5 min
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
        let prev = channel.messages[idx - 1]
        let curr = channel.messages[idx]
        return !Calendar.current.isDate(prev.timestamp, inSameDayAs: curr.timestamp)
    }

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

    @ViewBuilder
    private func messageRow(_ msg: ChatMessage, showHeader: Bool) -> some View {
        VStack(alignment: .leading, spacing: 0) {
            if showHeader {
                HStack(alignment: .top, spacing: 12) {
                    // Avatar
                    ZStack {
                        Circle()
                            .fill(Theme.nickColor(for: msg.from).opacity(0.2))
                            .frame(width: 40, height: 40)
                        Text(String(msg.from.prefix(1)).uppercased())
                            .font(.system(size: 16, weight: .bold))
                            .foregroundColor(Theme.nickColor(for: msg.from))
                    }

                    VStack(alignment: .leading, spacing: 3) {
                        // Nick + time
                        HStack(alignment: .firstTextBaseline, spacing: 8) {
                            Text(msg.from)
                                .font(.system(size: 15, weight: .bold))
                                .foregroundColor(Theme.nickColor(for: msg.from))

                            Text(formatTime(msg.timestamp))
                                .font(.system(size: 11))
                                .foregroundColor(Theme.textMuted)
                        }

                        messageBody(msg)
                    }

                    Spacer(minLength: 0)
                }
                .padding(.horizontal, 16)
                .padding(.top, 6)
                .padding(.bottom, 2)
            } else {
                // Compact continuation — just the text, aligned under previous avatar
                messageBody(msg)
                    .padding(.leading, 68) // 16 + 40 + 12 = 68
                    .padding(.trailing, 16)
                    .padding(.vertical, 1)
            }
        }
        .id(msg.id)
    }

    @ViewBuilder
    private func messageBody(_ msg: ChatMessage) -> some View {
        if msg.isAction {
            Text("*\(msg.from) \(msg.text)*")
                .font(.system(size: 15))
                .italic()
                .foregroundColor(Theme.textSecondary)
        } else if let url = extractImageURL(msg.text) {
            VStack(alignment: .leading, spacing: 6) {
                if msg.text != url.absoluteString {
                    Text(msg.text)
                        .font(.system(size: 15))
                        .foregroundColor(Theme.textPrimary)
                        .textSelection(.enabled)
                }
                AsyncImage(url: url) { phase in
                    switch phase {
                    case .success(let image):
                        image
                            .resizable()
                            .aspectRatio(contentMode: .fit)
                            .frame(maxWidth: 300, maxHeight: 300)
                            .cornerRadius(8)
                    case .failure:
                        linkText(msg.text)
                    default:
                        RoundedRectangle(cornerRadius: 8)
                            .fill(Theme.bgTertiary)
                            .frame(width: 200, height: 120)
                            .overlay(ProgressView().tint(Theme.textMuted))
                    }
                }
            }
        } else {
            Text(attributedMessage(msg.text))
                .font(.system(size: 15))
                .foregroundColor(Theme.textPrimary)
                .textSelection(.enabled)
        }
    }

    // Detect image URLs in message text
    private func extractImageURL(_ text: String) -> URL? {
        let pattern = #"https?://\S+\.(?:png|jpg|jpeg|gif|webp)"#
        guard let range = text.range(of: pattern, options: .regularExpression) else { return nil }
        return URL(string: String(text[range]))
    }

    private func linkText(_ text: String) -> some View {
        Text(text)
            .font(.system(size: 15))
            .foregroundColor(Theme.accent)
            .textSelection(.enabled)
    }

    // Styled text with bold, italic, code
    private func attributedMessage(_ text: String) -> AttributedString {
        var result = AttributedString(text)
        // Inline code: `text`
        if let codeRange = text.range(of: "`[^`]+`", options: .regularExpression) {
            let nsRange = NSRange(codeRange, in: text)
            if let attrRange = Range(nsRange, in: result) {
                result[attrRange].font = .system(size: 14, design: .monospaced)
                result[attrRange].backgroundColor = Theme.bgTertiary
            }
        }
        return result
    }

    private func formatTime(_ date: Date) -> String {
        let formatter = DateFormatter()
        formatter.dateFormat = "h:mm a"
        return formatter.string(from: date)
    }

    private func formatDate(_ date: Date) -> String {
        let formatter = DateFormatter()
        if Calendar.current.isDateInToday(date) {
            return "Today"
        } else if Calendar.current.isDateInYesterday(date) {
            return "Yesterday"
        } else {
            formatter.dateFormat = "MMMM d, yyyy"
            return formatter.string(from: date)
        }
    }
}
