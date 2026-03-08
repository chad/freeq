import SwiftUI

/// ⌘F Search — find messages in current channel.
struct SearchBar: View {
    @Environment(AppState.self) private var appState
    @Binding var isPresented: Bool
    @State private var query: String = ""
    @FocusState private var isFocused: Bool

    private var results: [ChatMessage] {
        guard !query.isEmpty, let ch = appState.activeChannelState else { return [] }
        let q = query.lowercased()
        return ch.messages.filter {
            !$0.isDeleted && ($0.text.lowercased().contains(q) || $0.from.lowercased().contains(q))
        }
    }

    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: 8) {
                Image(systemName: "magnifyingglass")
                    .foregroundStyle(.secondary)
                TextField("Search messages…", text: $query)
                    .textFieldStyle(.plain)
                    .focused($isFocused)
                Text("\(results.count) results")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                Button { isPresented = false } label: {
                    Image(systemName: "xmark.circle.fill")
                        .foregroundStyle(.secondary)
                }
                .buttonStyle(.plain)
                .keyboardShortcut(.cancelAction)
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 8)
            .background(.bar)

            if !results.isEmpty {
                Divider()
                ScrollView {
                    LazyVStack(alignment: .leading, spacing: 0) {
                        ForEach(results) { msg in
                            Button {
                                appState.scrollToMessageId = msg.id
                                isPresented = false
                            } label: {
                                VStack(alignment: .leading, spacing: 2) {
                                    HStack(spacing: 4) {
                                        Text(msg.from)
                                            .font(.caption.weight(.semibold))
                                            .foregroundStyle(Theme.nickColor(for: msg.from))
                                        Text(formatTime(msg.timestamp))
                                            .font(.caption2)
                                            .foregroundStyle(.tertiary)
                                    }
                                    Text(msg.text)
                                        .font(.caption)
                                        .foregroundStyle(.primary)
                                        .lineLimit(2)
                                }
                                .padding(.horizontal, 12)
                                .padding(.vertical, 6)
                                .frame(maxWidth: .infinity, alignment: .leading)
                                .contentShape(Rectangle())
                            }
                            .buttonStyle(.plain)
                            Divider().padding(.leading, 12)
                        }
                    }
                }
                .frame(maxHeight: 200)
            }
        }
    }
}
