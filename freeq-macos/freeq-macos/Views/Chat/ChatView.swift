import SwiftUI

/// Main chat area: TopBar + Search + MessageList + Typing + ComposeBar
struct ChatView: View {
    @Environment(AppState.self) private var appState

    var body: some View {
        VStack(spacing: 0) {
            TopBarView()
            Divider()

            // Search bar
            if appState.showSearch {
                SearchBar(isPresented: Binding(
                    get: { appState.showSearch },
                    set: { appState.showSearch = $0 }
                ))
                Divider()
            }

            MessageListView()
            Divider()

            // Typing indicator bar
            if let typers = appState.activeChannelState?.activeTypers, !typers.isEmpty {
                HStack(spacing: 4) {
                    TypingDotsView()
                    Text(typingText(typers))
                        .font(.caption)
                        .foregroundStyle(.secondary)
                    Spacer()
                }
                .padding(.horizontal, 16)
                .padding(.vertical, 4)
                .background(.bar)
            }
            ComposeBar()
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    private func typingText(_ typers: [String]) -> String {
        switch typers.count {
        case 1: return "\(typers[0]) is typing…"
        case 2: return "\(typers[0]) and \(typers[1]) are typing…"
        default: return "Several people are typing…"
        }
    }
}

struct TopBarView: View {
    @Environment(AppState.self) private var appState

    private var channel: ChannelState? { appState.activeChannelState }
    private var isChannel: Bool { channel?.isChannel ?? false }

    var body: some View {
        HStack(spacing: 10) {
            // Channel/DM name
            if isChannel {
                Image(systemName: "number")
                    .foregroundStyle(.secondary)
                Text(channel?.name.replacingOccurrences(of: "#", with: "") ?? "")
                    .font(.headline)
            } else {
                Circle()
                    .fill(isOnline ? .green : Color.secondary.opacity(0.3))
                    .frame(width: 10, height: 10)
                Text(channel?.name ?? "")
                    .font(.headline)

                // P2P badge
                if let name = channel?.name,
                   appState.p2pDMActive.contains(name.lowercased()) {
                    Label("Direct", systemImage: "point.3.connected.trianglepath.dotted")
                        .font(.caption2)
                        .foregroundStyle(.green)
                        .padding(.horizontal, 6)
                        .padding(.vertical, 2)
                        .background(Capsule().fill(.green.opacity(0.1)))
                }

                if !isChannel {
                    Text(isOnline ? (awayMsg != nil ? "away" : "online") : "offline")
                        .font(.caption)
                        .foregroundStyle(isOnline ? (awayMsg != nil ? .orange : .green) : .secondary)
                }
            }

            if isChannel, let topic = channel?.topic, !topic.isEmpty {
                Divider().frame(height: 16)
                Text(topic)
                    .font(.subheadline)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .help(topic)
            }

            Spacer()

            // Search
            Button {
                appState.showSearch.toggle()
            } label: {
                Image(systemName: "magnifyingglass")
                    .foregroundStyle(appState.showSearch ? .primary : .secondary)
            }
            .buttonStyle(.plain)
            .help("Search (⌘F)")

            // Member count (channels only)
            if isChannel {
                Label("\(channel?.members.count ?? 0)", systemImage: "person.2")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }

            // Detail panel toggle
            Button {
                appState.showDetailPanel.toggle()
            } label: {
                Image(systemName: "sidebar.trailing")
                    .foregroundStyle(appState.showDetailPanel ? .primary : .secondary)
            }
            .buttonStyle(.plain)
            .help("Toggle detail panel")
        }
        .padding(.horizontal, 16)
        .frame(height: 44)
        .background(.bar)
    }

    private var isOnline: Bool {
        guard let name = channel?.name else { return false }
        return appState.isNickOnline(name)
    }

    private var awayMsg: String? {
        guard let name = channel?.name else { return nil }
        return appState.awayStatus(for: name)
    }
}

// MARK: - Typing dots animation

struct TypingDotsView: View {
    @State private var phase = 0

    var body: some View {
        HStack(spacing: 2) {
            ForEach(0..<3) { i in
                Circle()
                    .fill(.secondary)
                    .frame(width: 4, height: 4)
                    .opacity(phase == i ? 1 : 0.3)
            }
        }
        .onAppear {
            Timer.scheduledTimer(withTimeInterval: 0.4, repeats: true) { _ in
                phase = (phase + 1) % 3
            }
        }
    }
}
