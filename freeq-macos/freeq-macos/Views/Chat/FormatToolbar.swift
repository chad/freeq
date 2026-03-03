import SwiftUI

/// Inline formatting toolbar — bold, italic, code, strikethrough, link.
struct FormatToolbar: View {
    @Binding var text: String

    var body: some View {
        HStack(spacing: 2) {
            FormatButton(icon: "bold", tooltip: "Bold") {
                wrapSelection(prefix: "**", suffix: "**")
            }
            FormatButton(icon: "italic", tooltip: "Italic") {
                wrapSelection(prefix: "_", suffix: "_")
            }
            FormatButton(icon: "chevron.left.forwardslash.chevron.right", tooltip: "Code") {
                wrapSelection(prefix: "`", suffix: "`")
            }
            FormatButton(icon: "strikethrough", tooltip: "Strikethrough") {
                wrapSelection(prefix: "~~", suffix: "~~")
            }
            FormatButton(icon: "link", tooltip: "Link") {
                wrapSelection(prefix: "[", suffix: "](url)")
            }
        }
    }

    private func wrapSelection(prefix: String, suffix: String) {
        // Simple: append format markers at cursor (no selection tracking in SwiftUI)
        text += "\(prefix)text\(suffix)"
    }
}

struct FormatButton: View {
    let icon: String
    let tooltip: String
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            Image(systemName: icon)
                .font(.caption)
                .foregroundStyle(.secondary)
                .frame(width: 24, height: 24)
        }
        .buttonStyle(.plain)
        .help(tooltip)
    }
}
