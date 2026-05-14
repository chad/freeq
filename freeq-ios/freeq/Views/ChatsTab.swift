import SwiftUI

/// Top-level chat list. Renders either channels (`.channels`) or direct
/// messages (`.dms`); MainTabView shows one of each as a peer tab.
struct ChatsTab: View {
    enum Mode {
        case channels
        case dms
    }

    let mode: Mode

    @EnvironmentObject var appState: AppState
    @EnvironmentObject var networkMonitor: NetworkMonitor
    @State private var showingJoinSheet = false
    @State private var searchText = ""
    @State private var navigationPath = NavigationPath()

    var body: some View {
        NavigationStack(path: $navigationPath) {
            ZStack {
                Theme.bgPrimary.ignoresSafeArea()

                if conversations.isEmpty {
                    emptyState
                } else {
                    List {
                        // Network warning
                        if !networkMonitor.isConnected {
                            HStack(spacing: 8) {
                                Image(systemName: "wifi.slash")
                                    .font(.system(size: 12))
                                Text("No network connection")
                                    .font(.system(size: 13, weight: .medium))
                            }
                            .foregroundColor(.white)
                            .listRowBackground(Theme.danger)
                        }

                        ForEach(filteredConversations, id: \.name) { conv in
                            NavigationLink(value: conv.name) {
                                ChatRow(conversation: conv, unreadCount: appState.unreadCounts[conv.name] ?? 0)
                            }
                            .listRowBackground(Theme.bgSecondary)
                            .listRowSeparatorTint(Theme.border)
                            .swipeActions(edge: .trailing) {
                                Button(role: .destructive) {
                                    if conv.name.hasPrefix("#") {
                                        appState.partChannel(conv.name)
                                    } else {
                                        appState.closeDM(conv.name)
                                    }
                                } label: {
                                    Label(conv.name.hasPrefix("#") ? "Leave" : "Close", systemImage: "arrow.right.square")
                                }
                            }
                            .swipeActions(edge: .leading) {
                                Button {
                                    appState.toggleMute(conv.name)
                                    UIImpactFeedbackGenerator(style: .light).impactOccurred()
                                } label: {
                                    Label(
                                        appState.isMuted(conv.name) ? "Unmute" : "Mute",
                                        systemImage: appState.isMuted(conv.name) ? "bell.fill" : "bell.slash.fill"
                                    )
                                }
                                .tint(Theme.warning)

                                Button {
                                    appState.markRead(conv.name)
                                    UIImpactFeedbackGenerator(style: .light).impactOccurred()
                                } label: {
                                    Label("Read", systemImage: "checkmark.circle")
                                }
                                .tint(Theme.accent)
                            }
                        }
                    }
                    .listStyle(.plain)
                    .scrollContentBackground(.hidden)
                    .searchable(text: $searchText, prompt: searchPrompt)
                }
            }
            .navigationTitle(navTitle)
            .navigationBarTitleDisplayMode(.inline)
            .toolbarBackground(Theme.bgSecondary, for: .navigationBar)
            .toolbarBackground(.visible, for: .navigationBar)
            .toolbar {
                if mode == .channels {
                    ToolbarItem(placement: .topBarTrailing) {
                        Button(action: { showingJoinSheet = true }) {
                            Image(systemName: "square.and.pencil")
                                .font(.system(size: 16))
                                .foregroundColor(Theme.accent)
                        }
                    }
                }
            }
            .navigationDestination(for: String.self) { channelName in
                ChatDetailView(channelName: channelName)
            }
            .sheet(isPresented: $showingJoinSheet) {
                JoinChannelSheet()
                    .presentationDetents([.medium])
                    .presentationDragIndicator(.visible)
            }
            .onChange(of: appState.pendingDMNick) {
                // Only the DMs pane consumes pending-DM navigations; the
                // channels pane ignores them so we don't push a DM into
                // the channels nav stack.
                guard mode == .dms, let nick = appState.pendingDMNick else { return }
                appState.pendingDMNick = nil
                navigationPath = NavigationPath()
                DispatchQueue.main.asyncAfter(deadline: .now() + 0.1) {
                    appState.activeChannel = nick
                    navigationPath.append(nick)
                }
            }
        }
    }

    private var conversations: [ChannelState] {
        let source: [ChannelState]
        switch mode {
        case .channels: source = appState.channels
        case .dms:      source = appState.dmBuffers
        }
        return source
            .filter { !$0.name.trimmingCharacters(in: .whitespaces).isEmpty }
            .sorted { a, b in a.lastActivity > b.lastActivity }
    }

    private var filteredConversations: [ChannelState] {
        let convos = conversations
        if searchText.isEmpty { return convos }
        return convos.filter { $0.name.localizedCaseInsensitiveContains(searchText) }
    }

    private var navTitle: String {
        switch mode {
        case .channels: return "Channels"
        case .dms:      return "Direct Messages"
        }
    }

    private var searchPrompt: String {
        switch mode {
        case .channels: return "Search channels"
        case .dms:      return "Search messages"
        }
    }

    @ViewBuilder
    private var emptyState: some View {
        switch mode {
        case .channels:
            VStack(spacing: 16) {
                Image(systemName: "number.circle")
                    .font(.system(size: 48))
                    .foregroundColor(Theme.textMuted)
                Text("No channels yet")
                    .font(.system(size: 18, weight: .medium))
                    .foregroundColor(Theme.textSecondary)
                Text("Join a channel to get started")
                    .font(.system(size: 14))
                    .foregroundColor(Theme.textMuted)
                Button(action: { showingJoinSheet = true }) {
                    HStack(spacing: 6) {
                        Image(systemName: "plus.circle.fill")
                        Text("Join Channel")
                    }
                    .font(.system(size: 15, weight: .medium))
                    .foregroundColor(Theme.accent)
                }
            }
        case .dms:
            VStack(spacing: 16) {
                Image(systemName: "bubble.left.and.bubble.right")
                    .font(.system(size: 48))
                    .foregroundColor(Theme.textMuted)
                Text("No direct messages yet")
                    .font(.system(size: 18, weight: .medium))
                    .foregroundColor(Theme.textSecondary)
                Text("Tap a member's avatar in any channel to start a private chat.")
                    .font(.system(size: 14))
                    .foregroundColor(Theme.textMuted)
                    .multilineTextAlignment(.center)
                    .padding(.horizontal, 32)
            }
        }
    }
}

// MARK: - Chat Row

struct ChatRow: View {
    @EnvironmentObject var appState: AppState
    @ObservedObject var conversation: ChannelState
    let unreadCount: Int

    private var isChannel: Bool { conversation.name.hasPrefix("#") }

    /// Check if this DM contact is online in any shared channel
    private var presence: (online: Bool, away: Bool) {
        guard !isChannel else { return (false, false) }
        let nick = conversation.name.lowercased()
        var found = false
        var away = false
        for ch in appState.channels {
            if let m = ch.members.first(where: { $0.nick.lowercased() == nick }) {
                found = true
                away = away || m.isAway
            }
        }
        return (found, away)
    }

    private var lastMessage: ChatMessage? {
        conversation.messages.last(where: { !$0.from.isEmpty && !$0.isDeleted })
    }

    private static let listTimeFormatter: DateFormatter = {
        let f = DateFormatter()
        f.locale = .current
        f.timeStyle = .short  // "3:42 PM" in en_US, "15:42" in en_GB / de_DE / etc.
        f.dateStyle = .none
        return f
    }()

    private static let listDateFormatter: DateFormatter = {
        let f = DateFormatter()
        f.locale = .current
        // setLocalizedDateFormatFromTemplate respects the user's region:
        // "1/15/26" in en_US, "15/01/2026" in en_GB / de_DE.
        f.setLocalizedDateFormatFromTemplate("yMd")
        return f
    }()

    private var timeString: String {
        guard let msg = lastMessage else { return "" }
        let cal = Calendar.current
        if cal.isDateInToday(msg.timestamp) {
            return Self.listTimeFormatter.string(from: msg.timestamp)
        } else if cal.isDateInYesterday(msg.timestamp) {
            return "Yesterday"
        } else {
            return Self.listDateFormatter.string(from: msg.timestamp)
        }
    }

    var body: some View {
        HStack(spacing: 12) {
            // Avatar / Icon with presence dot for DMs
            ZStack(alignment: .bottomTrailing) {
                if isChannel {
                    ZStack {
                        Circle()
                            .fill(Theme.accent.opacity(0.15))
                            .frame(width: 50, height: 50)
                        Text("#")
                            .font(.system(size: 22, weight: .bold, design: .rounded))
                            .foregroundColor(Theme.accent)
                    }
                } else {
                    UserAvatar(nick: conversation.name, size: 50)
                }

                // Online/away dot for DMs
                if !isChannel {
                    Circle()
                        .fill(presence.online ? (presence.away ? Theme.warning : Theme.success) : Theme.textMuted.opacity(0.3))
                        .frame(width: 14, height: 14)
                        .overlay(
                            Circle().stroke(Theme.bgSecondary, lineWidth: 2)
                        )
                }
            }

            // Content
            VStack(alignment: .leading, spacing: 4) {
                HStack {
                    Text(isChannel ? conversation.name : "@" + conversation.name)
                        .font(.system(size: 16, weight: unreadCount > 0 ? .bold : .regular))
                        .foregroundColor(Theme.textPrimary)
                        .lineLimit(1)

                    if appState.isMuted(conversation.name) {
                        Image(systemName: "bell.slash.fill")
                            .font(.system(size: 10))
                            .foregroundColor(Theme.textMuted)
                    }

                    // Member count for channels
                    if isChannel && conversation.members.count > 0 {
                        Text("\(conversation.members.count)")
                            .font(.system(size: 11))
                            .foregroundColor(Theme.textMuted)
                    }

                    Spacer()

                    Text(timeString)
                        .font(.system(size: 12))
                        .foregroundColor(unreadCount > 0 ? Theme.accent : Theme.textMuted)
                }

                HStack {
                    if let msg = lastMessage {
                        Group {
                            if msg.isAction {
                                Text("\(msg.from) \(msg.text)")
                            } else {
                                Text("\(msg.from): \(msg.text)")
                            }
                        }
                        .font(.system(size: 14))
                        .foregroundColor(Theme.textSecondary)
                        .lineLimit(2)
                    } else if !conversation.topic.isEmpty {
                        Text(conversation.topic)
                            .font(.system(size: 14))
                            .foregroundColor(Theme.textMuted)
                            .lineLimit(1)
                    } else {
                        Text("No messages yet")
                            .font(.system(size: 14))
                            .foregroundColor(Theme.textMuted)
                            .lineLimit(1)
                    }

                    Spacer()

                    if unreadCount > 0 {
                        Text("\(unreadCount)")
                            .font(.system(size: 12, weight: .bold))
                            .foregroundColor(.white)
                            .padding(.horizontal, 7)
                            .padding(.vertical, 2)
                            .background(Theme.accent)
                            .clipShape(Capsule())
                    }

                    // Typing indicator
                    if !conversation.activeTypers.isEmpty {
                        Image(systemName: "ellipsis.bubble.fill")
                            .font(.system(size: 14))
                            .foregroundColor(Theme.accent)
                    }
                }
            }
        }
        .padding(.vertical, 4)
    }
}
