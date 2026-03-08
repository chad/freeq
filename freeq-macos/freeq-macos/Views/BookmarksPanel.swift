import SwiftUI

struct BookmarksPanel: View {
    @Environment(AppState.self) private var appState
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        VStack(spacing: 0) {
            HStack {
                Text("Bookmarks")
                    .font(.headline)
                Spacer()
                Button("Done") { dismiss() }
            }
            .padding(16)

            Divider()

            if appState.bookmarks.isEmpty {
                VStack(spacing: 12) {
                    Image(systemName: "bookmark")
                        .font(.system(size: 32))
                        .foregroundStyle(.tertiary)
                    Text("No bookmarks yet")
                        .foregroundStyle(.secondary)
                    Text("Right-click a message and select Bookmark")
                        .font(.caption)
                        .foregroundStyle(.tertiary)
                }
                .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else {
                List {
                    ForEach(appState.bookmarks.sorted(by: { $0.timestamp > $1.timestamp })) { bookmark in
                        Button {
                            appState.activeChannel = bookmark.channel
                            appState.scrollToMessageId = bookmark.msgId
                            dismiss()
                        } label: {
                            VStack(alignment: .leading, spacing: 4) {
                                HStack {
                                    Text(bookmark.from)
                                        .font(.caption.weight(.bold))
                                        .foregroundStyle(Theme.nickColor(for: bookmark.from))
                                    Text("in \(bookmark.channel)")
                                        .font(.caption2)
                                        .foregroundStyle(.tertiary)
                                    Spacer()
                                    Text(formatTime(bookmark.timestamp))
                                        .font(.caption2)
                                        .foregroundStyle(.tertiary)
                                }
                                Text(bookmark.text)
                                    .font(.caption)
                                    .lineLimit(2)
                            }
                        }
                        .buttonStyle(.plain)
                        .contextMenu {
                            Button("Remove Bookmark", role: .destructive) {
                                appState.removeBookmark(msgId: bookmark.msgId)
                            }
                        }
                    }
                }
            }
        }
        .frame(width: 400, height: 350)
    }
}
