import SwiftUI

/// Right panel — Member list for channels, Profile for DMs, P2P info
struct DetailPanel: View {
    @Environment(AppState.self) private var appState

    private var channel: ChannelState? { appState.activeChannelState }

    var body: some View {
        VStack(spacing: 0) {
            if let ch = channel {
                if ch.isChannel {
                    MemberListView(channel: ch)
                } else {
                    DMProfilePanel(nick: ch.name)
                }
            }
        }
        .background(Color(nsColor: .controlBackgroundColor))
    }
}

struct MemberListView: View {
    @Environment(AppState.self) private var appState
    let channel: ChannelState
    @State private var searchText: String = ""

    private var ops: [MemberInfo] { filtered.filter(\.isOp).sorted { $0.nick < $1.nick } }
    private var voiced: [MemberInfo] { filtered.filter { !$0.isOp && $0.isVoiced }.sorted { $0.nick < $1.nick } }
    private var regular: [MemberInfo] { filtered.filter { !$0.isOp && !$0.isVoiced }.sorted { $0.nick < $1.nick } }

    private var filtered: [MemberInfo] {
        if searchText.isEmpty { return channel.members }
        let q = searchText.lowercased()
        return channel.members.filter { $0.nick.lowercased().contains(q) }
    }

    var body: some View {
        VStack(spacing: 0) {
            // Search
            HStack(spacing: 6) {
                Image(systemName: "magnifyingglass")
                    .font(.caption)
                    .foregroundStyle(.tertiary)
                TextField("Search members", text: $searchText)
                    .textFieldStyle(.plain)
                    .font(.caption)
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 6)
            .background(Color(nsColor: .textBackgroundColor).opacity(0.5))

            Divider()

            // Total member count header
            HStack {
                Text("\(channel.members.count) Members")
                    .font(.caption.weight(.semibold))
                    .foregroundStyle(.secondary)
                Spacer()
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 6)

            ScrollView {
                LazyVStack(alignment: .leading, spacing: 0) {
                    if !ops.isEmpty {
                        memberSection("Operators — \(ops.count)", members: ops)
                    }
                    if !voiced.isEmpty {
                        memberSection("Voiced — \(voiced.count)", members: voiced)
                    }
                    if !regular.isEmpty {
                        memberSection("Online — \(regular.count)", members: regular)
                    }
                }
                .padding(.vertical, 4)
            }
        }
    }

    @ViewBuilder
    func memberSection(_ title: String, members: [MemberInfo]) -> some View {
        Text(title)
            .font(.caption.weight(.bold))
            .foregroundStyle(.tertiary)
            .textCase(.uppercase)
            .padding(.horizontal, 12)
            .padding(.top, 12)
            .padding(.bottom, 4)

        ForEach(members) { member in
            MemberRow(member: member, channelName: channel.name)
        }
    }
}

struct MemberRow: View {
    @Environment(AppState.self) private var appState
    let member: MemberInfo
    let channelName: String
    @State private var showProfile = false

    private var profile: ProfileCache.Profile? {
        ProfileCache.shared.profile(for: member.nick)
    }

    private var hasDid: Bool {
        member.did != nil || ProfileCache.shared.did(for: member.nick) != nil
    }

    var body: some View {
        HStack(spacing: 8) {
            // Avatar with presence indicator
            AvatarView(nick: member.nick, size: 28)
                .overlay(alignment: .bottomTrailing) {
                    Circle()
                        .fill(member.isAway ? .orange : .green)
                        .frame(width: 8, height: 8)
                        .overlay(
                            Circle().strokeBorder(Color(nsColor: .windowBackgroundColor), lineWidth: 1.5)
                        )
                }

            VStack(alignment: .leading, spacing: 1) {
                HStack(spacing: 3) {
                    if member.isOp {
                        Image(systemName: "shield.fill")
                            .font(.system(size: 9))
                            .foregroundStyle(.orange)
                    } else if member.isHalfop {
                        Image(systemName: "shield.lefthalf.filled")
                            .font(.system(size: 9))
                            .foregroundStyle(.blue)
                    } else if !member.prefix.isEmpty {
                        Text(member.prefix)
                            .font(.caption.weight(.bold))
                            .foregroundStyle(.orange)
                    }

                    // Display name or nick
                    if let displayName = profile?.displayName, !displayName.isEmpty {
                        Text(displayName)
                            .font(.system(.body, weight: member.isAway ? .regular : .medium))
                            .foregroundStyle(member.isAway ? .secondary : .primary)
                            .lineLimit(1)
                    } else {
                        Text(member.nick)
                            .font(.system(.body, weight: member.isAway ? .regular : .medium))
                            .foregroundStyle(member.isAway ? .secondary : .primary)
                            .lineLimit(1)
                    }

                    // Verified badge
                    if hasDid {
                        Image(systemName: "checkmark.seal.fill")
                            .font(.caption2)
                            .foregroundStyle(.blue)
                            .help("AT Protocol verified identity")
                    }

                    if member.isAway {
                        Text("Away")
                            .font(.system(size: 9, weight: .semibold))
                            .foregroundStyle(.orange)
                            .padding(.horizontal, 4)
                            .padding(.vertical, 1)
                            .background(Color.orange.opacity(0.15))
                            .clipShape(RoundedRectangle(cornerRadius: 3))
                    }
                }

                // Handle or away message
                if let handle = profile?.handle {
                    Text("@\(handle)")
                        .font(.caption2)
                        .foregroundStyle(.tertiary)
                        .lineLimit(1)
                } else if member.isAway, let away = member.awayMsg {
                    Text(away)
                        .font(.caption2)
                        .foregroundStyle(.tertiary)
                        .lineLimit(1)
                }
            }
            Spacer()
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 4)
        .contentShape(Rectangle())
        .onTapGesture {
            if member.nick.lowercased() != appState.nick.lowercased() {
                showProfile = true
            }
        }
        .sheet(isPresented: $showProfile) {
            UserProfileSheet(nick: member.nick)
                .environment(appState)
        }
        .contextMenu {
            Button("Send Message") {
                let dm = appState.getOrCreateDM(member.nick)
                appState.activeChannel = dm.name
            }
            if let handle = profile?.handle {
                Button("View on Bluesky") {
                    if let url = URL(string: "https://bsky.app/profile/\(handle)") {
                        NSWorkspace.shared.open(url)
                    }
                }
            }
            Button("WHOIS") {
                appState.sendWhois(member.nick)
            }
            Divider()
            Button("Op") { appState.setMode(channelName, "+o", member.nick) }
            Button("Deop") { appState.setMode(channelName, "-o", member.nick) }
            Button("Voice") { appState.setMode(channelName, "+v", member.nick) }
            Divider()
            Button("Kick", role: .destructive) {
                appState.kickUser(channelName, member.nick)
            }
        }
    }
}

struct DMProfilePanel: View {
    @Environment(AppState.self) private var appState
    let nick: String

    private var isOnline: Bool { appState.isNickOnline(nick) }
    private var awayMsg: String? { appState.awayStatus(for: nick) }
    private var isP2p: Bool { appState.p2pDMActive.contains(nick.lowercased()) }
    private var profile: ProfileCache.Profile? { ProfileCache.shared.profile(for: nick) }
    private var did: String? { ProfileCache.shared.did(for: nick) }

    var body: some View {
        ScrollView {
            VStack(spacing: 0) {
                // Banner
                LinearGradient(
                    colors: [Theme.nickColor(for: nick).opacity(0.3), .clear],
                    startPoint: .topLeading,
                    endPoint: .bottomTrailing
                )
                .frame(height: 80)
                .overlay(alignment: .bottom) {
                    AvatarView(nick: nick, size: 56)
                        .overlay(alignment: .bottomTrailing) {
                            Circle()
                                .fill(isOnline ? (awayMsg != nil ? .orange : .green) : Color.secondary.opacity(0.3))
                                .frame(width: 14, height: 14)
                                .overlay(Circle().strokeBorder(.background, lineWidth: 2))
                        }
                        .offset(y: 28)
                }

                VStack(spacing: 4) {
                    // Display name
                    if let displayName = profile?.displayName, !displayName.isEmpty {
                        Text(displayName)
                            .font(.headline)
                            .padding(.top, 32)
                        Text(nick)
                            .font(.subheadline)
                            .foregroundStyle(.secondary)
                    } else {
                        Text(nick)
                            .font(.headline)
                            .padding(.top, 32)
                    }

                    // Handle
                    if let handle = profile?.handle {
                        HStack(spacing: 3) {
                            Text("@\(handle)")
                                .font(.caption)
                                .foregroundStyle(.blue)
                            Image(systemName: "checkmark.seal.fill")
                                .font(.caption2)
                                .foregroundStyle(.blue)
                        }
                    }

                    // Status
                    if isOnline {
                        if let away = awayMsg {
                            Label("Away: \(away)", systemImage: "moon.fill")
                                .font(.caption)
                                .foregroundStyle(.orange)
                        } else {
                            Label("Online", systemImage: "circle.fill")
                                .font(.caption)
                                .foregroundStyle(.green)
                        }
                    } else {
                        Label("Offline — messages saved", systemImage: "circle")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }

                    // P2P
                    if isP2p {
                        Label("Direct P2P via iroh", systemImage: "point.3.connected.trianglepath.dotted")
                            .font(.caption)
                            .foregroundStyle(.green)
                            .padding(.horizontal, 8)
                            .padding(.vertical, 4)
                            .background(Capsule().fill(.green.opacity(0.1)))
                    }
                }
                .padding(.horizontal, 16)
                .padding(.bottom, 12)

                // Bio
                if let desc = profile?.description, !desc.isEmpty {
                    Text(desc)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .padding(.horizontal, 16)
                        .padding(.bottom, 12)
                        .frame(maxWidth: .infinity, alignment: .leading)
                }

                // Stats
                if let profile, (profile.followersCount ?? 0) > 0 {
                    HStack(spacing: 16) {
                        statItem(count: profile.postsCount ?? 0, label: "Posts")
                        statItem(count: profile.followersCount ?? 0, label: "Followers")
                        statItem(count: profile.followsCount ?? 0, label: "Following")
                    }
                    .padding(.horizontal, 16)
                    .padding(.bottom, 12)
                }

                // DID
                if let did {
                    Text(did)
                        .font(.system(size: 9, design: .monospaced))
                        .foregroundStyle(.tertiary)
                        .lineLimit(2)
                        .padding(.horizontal, 16)
                        .padding(.bottom, 8)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .textSelection(.enabled)
                }

                Divider()

                // Actions
                VStack(spacing: 8) {
                    if let handle = profile?.handle {
                        Link(destination: URL(string: "https://bsky.app/profile/\(handle)")!) {
                            Label("View on Bluesky", systemImage: "arrow.up.right.square")
                                .frame(maxWidth: .infinity)
                        }
                        .buttonStyle(.bordered)
                    }

                    Button {
                        appState.sendWhois(nick)
                    } label: {
                        Label("WHOIS", systemImage: "person.text.rectangle")
                            .frame(maxWidth: .infinity)
                    }
                    .buttonStyle(.bordered)
                }
                .padding(16)
            }
        }
    }

    private func statItem(count: Int, label: String) -> some View {
        VStack(spacing: 1) {
            Text("\(count)")
                .font(.caption.weight(.bold))
            Text(label)
                .font(.caption2)
                .foregroundStyle(.tertiary)
        }
    }
}
