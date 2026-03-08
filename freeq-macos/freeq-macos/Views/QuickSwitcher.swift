import SwiftUI

/// ⌘K Quick Switcher — fuzzy search channels and DMs.
struct QuickSwitcher: View {
    @Environment(AppState.self) private var appState
    @Environment(\.dismiss) private var dismiss
    @State private var query: String = ""
    @FocusState private var isFocused: Bool
    @State private var selectedIndex: Int = 0

    private var results: [ChannelState] {
        let all = appState.allBuffers
        if query.isEmpty { return all }
        let q = query.lowercased()
        return all.filter { $0.name.lowercased().contains(q) }
    }

    var body: some View {
        VStack(spacing: 0) {
            // Search field
            HStack(spacing: 8) {
                Image(systemName: "magnifyingglass")
                    .foregroundStyle(.secondary)
                TextField("Switch to channel or DM…", text: $query)
                    .textFieldStyle(.plain)
                    .font(.title3)
                    .focused($isFocused)
                    .onSubmit { select() }
            }
            .padding(16)

            Divider()

            // Results
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 0) {
                    ForEach(Array(results.enumerated()), id: \.element.id) { index, item in
                        HStack(spacing: 10) {
                            if item.isChannel {
                                Image(systemName: "number")
                                    .foregroundStyle(.secondary)
                                    .frame(width: 20)
                            } else {
                                Circle()
                                    .fill(appState.isNickOnline(item.name) ? .green : Color.secondary.opacity(0.3))
                                    .frame(width: 10, height: 10)
                                    .frame(width: 20)
                            }
                            Text(item.name)
                                .lineLimit(1)
                            Spacer()
                            if let unread = appState.unreadCounts[item.name.lowercased()], unread > 0 {
                                Text("\(unread)")
                                    .font(.caption2.weight(.bold))
                                    .foregroundStyle(.white)
                                    .padding(.horizontal, 6)
                                    .padding(.vertical, 2)
                                    .background(Capsule().fill(.red))
                            }
                        }
                        .padding(.horizontal, 16)
                        .padding(.vertical, 8)
                        .background(index == selectedIndex ? Color.accentColor.opacity(0.15) : .clear)
                        .contentShape(Rectangle())
                        .onTapGesture {
                            appState.activeChannel = item.name
                            dismiss()
                        }
                    }
                }
            }
            .frame(maxHeight: 300)
        }
        .frame(width: 400)
        .background(.regularMaterial)
        .clipShape(RoundedRectangle(cornerRadius: 12))
        .shadow(radius: 20)
        .onAppear { isFocused = true }
        .onKeyPress(.upArrow) {
            selectedIndex = max(0, selectedIndex - 1)
            return .handled
        }
        .onKeyPress(.downArrow) {
            selectedIndex = min(results.count - 1, selectedIndex + 1)
            return .handled
        }
        .onKeyPress(.escape) {
            dismiss()
            return .handled
        }
        .onChange(of: query) { _, _ in
            selectedIndex = 0
        }
    }

    private func select() {
        guard selectedIndex < results.count else { return }
        appState.activeChannel = results[selectedIndex].name
        dismiss()
    }
}
