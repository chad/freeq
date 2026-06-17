import SwiftUI

/// ⌘K Quick Switcher — fuzzy search channels and DMs.
struct QuickSwitcher: View {
    @Environment(AppState.self) private var appState
    @Environment(\.dismiss) private var dismiss
    @State private var query: String = ""
    @FocusState private var isFocused: Bool
    @State private var selectedIndex: Int = 0

    private var results: [QuickSwitchItem] {
        QuickSwitchPlanner.items(query: query, buffers: appState.allBuffers)
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
                            if item.kind == .joinChannel {
                                Image(systemName: "plus.circle.fill")
                                    .foregroundStyle(Theme.accent)
                                    .frame(width: 20)
                            } else if item.isChannel {
                                Image(systemName: "number")
                                    .foregroundStyle(.secondary)
                                    .frame(width: 20)
                            } else {
                                Circle()
                                    .fill(appState.isNickOnline(item.name) ? .green : Color.secondary.opacity(0.3))
                                    .frame(width: 10, height: 10)
                                    .frame(width: 20)
                            }
                            VStack(alignment: .leading, spacing: 1) {
                                Text(item.name)
                                    .lineLimit(1)
                                if item.kind == .joinChannel {
                                    Text("Join channel")
                                        .font(.caption2)
                                        .foregroundStyle(Theme.textTertiary)
                                }
                            }
                            Spacer()
                            if item.kind == .existing,
                               let unread = appState.unreadCounts[item.name.lowercased()], unread > 0 {
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
                            activate(item)
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
            guard !results.isEmpty else {
                selectedIndex = 0
                return .handled
            }
            selectedIndex = max(0, selectedIndex - 1)
            return .handled
        }
        .onKeyPress(.downArrow) {
            guard !results.isEmpty else {
                selectedIndex = 0
                return .handled
            }
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
        activate(results[selectedIndex])
        dismiss()
    }

    private func activate(_ item: QuickSwitchItem) {
        switch item.kind {
        case .existing:
            appState.activeChannel = item.name
        case .joinChannel:
            appState.getOrCreateChannel(item.name)
            appState.joinChannel(item.name)
            appState.activeChannel = item.name
        }
    }
}
