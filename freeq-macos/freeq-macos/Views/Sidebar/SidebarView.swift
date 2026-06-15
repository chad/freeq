import SwiftUI

struct SidebarView: View {
    @Environment(AppState.self) private var appState

    var body: some View {
        @Bindable var state = appState
        List(selection: $state.activeChannel) {
            // Favorites
            let favChannels = appState.channels.filter { appState.favorites.contains($0.name.lowercased()) }
            if !favChannels.isEmpty {
                Section("Favorites") {
                    ForEach(favChannels) { channel in
                        ChannelRow(channel: channel)
                            .tag(channel.name)
                            .listRowBackground(Color.clear)
                            .listRowSeparator(.hidden)
                    }
                }
            }

            // Channels (non-favorites)
            Section("Channels") {
                ForEach(appState.channels.filter { !appState.favorites.contains($0.name.lowercased()) }) { channel in
                    ChannelRow(channel: channel)
                        .tag(channel.name)
                        .listRowBackground(Color.clear)
                        .listRowSeparator(.hidden)
                }
            }

            // DMs
            if !appState.dmBuffers.isEmpty {
                Section("Direct Messages") {
                    ForEach(appState.dmBuffers.sorted(by: { $0.lastActivity > $1.lastActivity })) { dm in
                        DMRow(dm: dm)
                            .tag(dm.name)
                            .listRowBackground(Color.clear)
                            .listRowSeparator(.hidden)
                    }
                }
            }

            // P2P connections
            if !appState.p2pConnectedPeers.isEmpty {
                Section("P2P Direct") {
                    ForEach(Array(appState.p2pConnectedPeers), id: \.self) { peerId in
                        Label {
                            Text(String(peerId.prefix(12)) + "…")
                                .font(.system(.body, design: .monospaced))
                        } icon: {
                            Image(systemName: "point.3.connected.trianglepath.dotted")
                                .foregroundStyle(.green)
                        }
                        .tag("p2p:\(String(peerId.prefix(8)))")
                    }
                }
            }
        }
        .listStyle(.sidebar)
        .scrollContentBackground(.hidden)
        .background(Theme.sidebarBackground)
        .safeAreaInset(edge: .bottom) {
            VStack(spacing: 0) {
                Divider().overlay(Theme.borderSoft)
                bottomBar
            }
        }
        .onChange(of: appState.activeChannel) { _, newValue in
            if let ch = newValue {
                appState.clearUnread(ch)
                // Request DM history if no messages loaded yet
                if !ch.hasPrefix("#") {
                    if let dm = appState.dmBuffers.first(where: { $0.name.lowercased() == ch.lowercased() }),
                       dm.messages.isEmpty {
                        appState.requestHistory(channel: ch)
                    }
                }
            }
        }
    }

    @ViewBuilder
    private var bottomBar: some View {
        HStack(spacing: 8) {
            // User info
            if let did = appState.authenticatedDID {
                Circle()
                    .fill(Theme.success)
                    .frame(width: 8, height: 8)
                VStack(alignment: .leading, spacing: 0) {
                    Text(appState.nick)
                        .font(.caption.weight(.medium))
                        .lineLimit(1)
                    Text(did.prefix(24) + "…")
                        .font(.caption2)
                        .foregroundStyle(Theme.textTertiary)
                        .lineLimit(1)
                }
            } else if appState.connectionState == .registered {
                Circle()
                    .fill(Theme.warning)
                    .frame(width: 8, height: 8)
                Text("\(appState.nick) (guest)")
                    .font(.caption)
                    .foregroundStyle(Theme.textSecondary)
            } else {
                Circle()
                    .fill(Theme.textTertiary)
                    .frame(width: 8, height: 8)
                Text("Not connected")
                    .font(.caption)
                    .foregroundStyle(Theme.textSecondary)
            }
            Spacer()

            // P2P status
            if appState.isP2pActive {
                Image(systemName: "point.3.connected.trianglepath.dotted")
                    .font(.caption)
                    .foregroundStyle(Theme.success)
                    .help("iroh P2P: \(appState.p2pConnectedPeers.count) peers")
            }

            // Join channel
            Button {
                appState.showJoinSheet = true
            } label: {
                Image(systemName: "plus.bubble")
            }
            .buttonStyle(.plain)
            .help("Join Channel (⌘J)")

            // User menu
            Menu {
                if appState.authenticatedDID != nil {
                    Button("Set Away…") {
                        appState.setAway("AFK")
                    }
                    Button("Remove Away") {
                        appState.setAway(nil)
                    }
                    Divider()
                }
                Button("Disconnect") {
                    appState.disconnect()
                }
                if appState.authenticatedDID != nil {
                    Button("Logout", role: .destructive) {
                        appState.logout()
                    }
                }
            } label: {
                Image(systemName: "ellipsis.circle")
            }
            .buttonStyle(.plain)
            .menuStyle(.borderlessButton)
            .frame(width: 20)
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 8)
        .background(Theme.sidebarBackground)
    }
}

struct ChannelRow: View {
    @Environment(AppState.self) private var appState
    let channel: ChannelState

    private var unread: Int {
        appState.unreadCounts[channel.name.lowercased()] ?? 0
    }

    private var mentions: Int {
        appState.mentionCounts[channel.name.lowercased()] ?? 0
    }

    private var isActive: Bool {
        appState.activeChannel?.lowercased() == channel.name.lowercased()
    }

    private var lastMessage: ChatMessage? {
        channel.messages.last(where: { !$0.from.isEmpty && !$0.isDeleted })
    }

    var body: some View {
        Label {
            VStack(alignment: .leading, spacing: 2) {
                HStack {
                    Text(channel.name.replacingOccurrences(of: "#", with: ""))
                        .lineLimit(1)
                        .font(.system(.body, weight: unread > 0 || isActive ? .semibold : .medium))
                        .foregroundStyle(isActive ? Theme.textPrimary : Theme.textSecondary)
                    Spacer()
                    if mentions > 0 {
                        Text("\(mentions)")
                            .font(.caption2.weight(.bold))
                            .foregroundStyle(.white)
                            .padding(.horizontal, 6)
                            .padding(.vertical, 2)
                            .background(Capsule().fill(Theme.danger))
                    } else if unread > 0 {
                        Circle()
                            .fill(Theme.accent)
                            .frame(width: 8, height: 8)
                    }
                }
                if let last = lastMessage {
                    Text("\(last.from): \(last.text)")
                        .font(.caption2)
                        .foregroundStyle(isActive ? Theme.textSecondary : Theme.textTertiary)
                        .lineLimit(1)
                }
            }
        } icon: {
            Image(systemName: "number")
                .font(.system(size: 15, weight: .medium))
                .foregroundStyle(isActive ? Theme.accent : Theme.textTertiary)
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 7)
        .background(
            RoundedRectangle(cornerRadius: 10)
                .fill(isActive ? Theme.surfaceElevated : Color.clear)
        )
        .overlay(
            RoundedRectangle(cornerRadius: 10)
                .strokeBorder(isActive ? Theme.borderSoft : Color.clear, lineWidth: 1)
        )
        .contextMenu {
            Button(appState.favorites.contains(channel.name.lowercased()) ? "Unfavorite" : "Favorite") {
                appState.toggleFavorite(channel.name)
            }
            Button(appState.mutedChannels.contains(channel.name.lowercased()) ? "Unmute" : "Mute") {
                appState.toggleMuted(channel.name)
            }
            Divider()
            Button("Leave Channel") {
                appState.partChannel(channel.name)
            }
        }
        .opacity(appState.mutedChannels.contains(channel.name.lowercased()) ? 0.5 : 1)
    }
}

struct DMRow: View {
    @Environment(AppState.self) private var appState
    let dm: ChannelState

    private var isOnline: Bool {
        appState.isNickOnline(dm.name)
    }

    private var unread: Int {
        appState.unreadCounts[dm.name.lowercased()] ?? 0
    }

    private var profile: ProfileCache.Profile? {
        ProfileCache.shared.profile(for: dm.name)
    }

    private var isActive: Bool {
        appState.activeChannel?.lowercased() == dm.name.lowercased()
    }

    private var lastMessage: ChatMessage? {
        dm.messages.last(where: { !$0.isDeleted })
    }

    var body: some View {
        Label {
            VStack(alignment: .leading, spacing: 2) {
                HStack {
                    Text(profile?.displayName ?? dm.name)
                        .lineLimit(1)
                        .font(.system(.body, weight: unread > 0 || isActive ? .semibold : .medium))
                        .foregroundStyle(isActive ? Theme.textPrimary : Theme.textSecondary)

                    if appState.p2pDMActive.contains(dm.name.lowercased()) {
                        Image(systemName: "point.3.connected.trianglepath.dotted")
                            .font(.caption2)
                            .foregroundStyle(Theme.success)
                    }

                    Spacer()
                    if unread > 0 {
                        Text("\(unread)")
                            .font(.caption2.weight(.bold))
                            .foregroundStyle(.white)
                            .padding(.horizontal, 6)
                            .padding(.vertical, 2)
                            .background(Capsule().fill(Theme.danger))
                    }
                }
                if let last = lastMessage {
                    Text(last.text)
                        .font(.caption2)
                        .foregroundStyle(isActive ? Theme.textSecondary : Theme.textTertiary)
                        .lineLimit(1)
                }
            }
        } icon: {
            AvatarView(nick: dm.name, size: 22)
                .overlay(alignment: .bottomTrailing) {
                    Circle()
                        .fill(isOnline ? Theme.success : Theme.textTertiary.opacity(0.35))
                        .frame(width: 7, height: 7)
                        .overlay(Circle().strokeBorder(Theme.surface, lineWidth: 1))
                }
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 7)
        .background(
            RoundedRectangle(cornerRadius: 10)
                .fill(isActive ? Theme.surfaceElevated : Color.clear)
        )
        .overlay(
            RoundedRectangle(cornerRadius: 10)
                .strokeBorder(isActive ? Theme.borderSoft : Color.clear, lineWidth: 1)
        )
        .contextMenu {
            Button("Close DM") {
                appState.closeDM(dm.name)
            }
        }
    }
}
