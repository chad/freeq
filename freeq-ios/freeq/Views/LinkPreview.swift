import SwiftUI

/// Simple link preview — shows domain with icon. Opens in Safari on tap.
struct LinkPreviewCard: View {
    let url: URL

    private var domain: String {
        url.host?.replacingOccurrences(of: "www.", with: "") ?? url.absoluteString
    }

    private var icon: String {
        let d = domain.lowercased()
        if d.contains("github.com") { return "chevron.left.forwardslash.chevron.right" }
        if d.contains("twitter.com") || d.contains("x.com") { return "at" }
        if d.contains("bsky.app") { return "bird" }
        if d.contains("reddit.com") { return "bubble.left.and.bubble.right" }
        if d.contains("wikipedia.org") { return "book" }
        if d.contains("apple.com") { return "apple.logo" }
        return "link"
    }

    var body: some View {
        Link(destination: url) {
            HStack(spacing: 8) {
                Image(systemName: icon)
                    .font(.system(size: 13))
                    .foregroundColor(Theme.accent)
                    .frame(width: 28, height: 28)
                    .background(Theme.accent.opacity(0.1))
                    .cornerRadius(6)

                VStack(alignment: .leading, spacing: 1) {
                    Text(domain)
                        .font(.system(size: 13, weight: .medium))
                        .foregroundColor(Theme.textPrimary)
                    Text(url.path.count > 1 ? String(url.path.prefix(50)) : "")
                        .font(.system(size: 11))
                        .foregroundColor(Theme.textMuted)
                        .lineLimit(1)
                }

                Spacer()

                Image(systemName: "arrow.up.right.square")
                    .font(.system(size: 12))
                    .foregroundColor(Theme.textMuted)
            }
            .padding(10)
            .background(Theme.bgTertiary)
            .cornerRadius(10)
            .overlay(
                RoundedRectangle(cornerRadius: 10)
                    .stroke(Theme.border, lineWidth: 1)
            )
        }
        .buttonStyle(.plain)
        .frame(maxWidth: 300)
    }
}
