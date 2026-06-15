import SwiftUI

/// Main chat area: TopBar + Search + MessageList + Typing + ComposeBar
struct ChatView: View {
    @Environment(AppState.self) private var appState

    var body: some View {
        VStack(spacing: 0) {
            TopBarView()
            Divider().overlay(Theme.borderSoft)

            if appState.showMotd && !appState.motd.isEmpty {
                MotdBanner()
                Divider().overlay(Theme.borderSoft)
            }

            // Pinned messages bar
            if let pins = appState.activeChannelState?.pinnedMessages, !pins.isEmpty {
                PinnedMessagesBar(pins: pins)
                Divider().overlay(Theme.borderSoft)
            }

            // Search bar
            if appState.showSearch {
                SearchBar(isPresented: Binding(
                    get: { appState.showSearch },
                    set: { appState.showSearch = $0 }
                ))
                Divider().overlay(Theme.borderSoft)
            }

            ZStack {
                MessageListView()
                if appState.activeChannelState?.messages.isEmpty ?? true {
                    ChannelWelcomeView()
                }
            }
            Divider().overlay(Theme.borderSoft)

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
                .background(Theme.surfaceSoft)
            }
            ComposeBar()
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(Theme.chatBackground)
    }

    private func typingText(_ typers: [String]) -> String {
        switch typers.count {
        case 1: return "\(typers[0]) is typing…"
        case 2: return "\(typers[0]) and \(typers[1]) are typing…"
        default: return "Several people are typing…"
        }
    }
}

// MARK: - MOTD Banner

struct MotdBanner: View {
    @Environment(AppState.self) private var appState
    @State private var expanded = false

    private var motdText: String {
        appState.motd.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    private var previewText: String {
        motdText
            .split(separator: "\n", omittingEmptySubsequences: true)
            .first
            .map(String.init) ?? "Server announcement"
    }

    var body: some View {
        VStack(alignment: .leading, spacing: expanded ? 8 : 0) {
            HStack(spacing: 8) {
                Image(systemName: "info.circle")
                    .font(.caption)
                    .foregroundStyle(.secondary)

                Text("MOTD")
                    .font(.caption.weight(.semibold))

                if !expanded {
                    Text(previewText)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                        .truncationMode(.tail)
                }

                Spacer(minLength: 8)

                Button {
                    withAnimation(.easeInOut(duration: 0.15)) {
                        expanded.toggle()
                    }
                } label: {
                    Label(expanded ? "Hide" : "Show", systemImage: expanded ? "chevron.up" : "chevron.down")
                        .labelStyle(.titleAndIcon)
                }
                .font(.caption)
                .buttonStyle(.plain)
                .help(expanded ? "Hide message of the day" : "Show message of the day")

                Button {
                    appState.showMotd = false
                } label: {
                    Image(systemName: "xmark")
                        .font(.caption2)
                }
                .buttonStyle(.plain)
                .help("Dismiss message of the day")
            }

            if expanded {
                ScrollView {
                    Text(motdText)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .textSelection(.enabled)
                        .frame(maxWidth: .infinity, alignment: .leading)
                }
                .frame(maxHeight: 180)
            }
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 8)
        .background(Theme.surface)
    }
}

struct ChannelWelcomeView: View {
    @Environment(AppState.self) private var appState

    private var channel: ChannelState? { appState.activeChannelState }
    private var isChannel: Bool { channel?.isChannel ?? false }
    private var displayName: String {
        guard let name = channel?.name else { return "freeq" }
        return isChannel ? name.replacingOccurrences(of: "#", with: "") : name
    }

    var body: some View {
        VStack(spacing: 18) {
            ZStack {
                Circle()
                    .fill(isChannel ? Theme.accentSoft : Theme.blue.opacity(0.10))
                    .frame(width: 84, height: 84)
                Image(systemName: isChannel ? "number" : "person.crop.circle.fill")
                    .font(.system(size: 34, weight: .semibold))
                    .foregroundStyle(isChannel ? Theme.accent : Theme.blue)
            }

            VStack(spacing: 6) {
                Text(isChannel ? "#\(displayName)" : displayName)
                    .font(.title.weight(.semibold))
                    .foregroundStyle(Theme.textPrimary)
                Text(subtitle)
                    .font(.subheadline)
                    .foregroundStyle(Theme.textSecondary)
                    .multilineTextAlignment(.center)
                    .frame(maxWidth: 460)
            }

            HStack(spacing: 10) {
                if isChannel {
                    contextPill(icon: "person.2.fill", text: "\(channel?.members.count ?? 0) members")
                    if let topic = channel?.topic, !topic.isEmpty {
                        contextPill(icon: "quote.bubble.fill", text: topic)
                    }
                } else if let did = ProfileCache.shared.did(for: displayName) {
                    contextPill(icon: "checkmark.seal.fill", text: did.hasPrefix("did:key:") ? "Verified identity" : "Bluesky identity")
                }
            }
        }
        .padding(32)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(Theme.chatBackground)
        .allowsHitTesting(false)
    }

    private var subtitle: String {
        if isChannel {
            if let topic = channel?.topic, !topic.isEmpty {
                return topic
            }
            return "No messages here yet. Start the conversation, pin the important bits, and let identity do the quiet trust work."
        }
        return "This direct message is private to the two of you. Say hello when you are ready."
    }

    private func contextPill(icon: String, text: String) -> some View {
        Label(text, systemImage: icon)
            .font(.caption.weight(.medium))
            .foregroundStyle(Theme.textSecondary)
            .lineLimit(1)
            .padding(.horizontal, 10)
            .padding(.vertical, 6)
            .background(Capsule().fill(Theme.surface))
            .overlay(Capsule().strokeBorder(Theme.borderSoft, lineWidth: 1))
    }
}

struct TopBarView: View {
    @Environment(AppState.self) private var appState
    @State private var showSettings = false

    private var channel: ChannelState? { appState.activeChannelState }
    private var isChannel: Bool { channel?.isChannel ?? false }

    var body: some View {
        HStack(spacing: 10) {
            // Channel/DM name
            if isChannel {
                ZStack {
                    RoundedRectangle(cornerRadius: 8)
                        .fill(Theme.accentSoft)
                        .frame(width: 30, height: 30)
                    Image(systemName: "number")
                        .font(.headline)
                        .foregroundStyle(Theme.accent)
                }
                VStack(alignment: .leading, spacing: 1) {
                    Text(channel?.name.replacingOccurrences(of: "#", with: "") ?? "")
                        .font(.headline.weight(.semibold))
                        .foregroundStyle(Theme.textPrimary)
                    Text("\(channel?.members.count ?? 0) members")
                        .font(.caption2)
                        .foregroundStyle(Theme.textTertiary)
                }
            } else {
                Circle()
                    .fill(isOnline ? Theme.success : Theme.textTertiary.opacity(0.35))
                    .frame(width: 10, height: 10)
                Text(channel?.name ?? "")
                    .font(.headline.weight(.semibold))
                    .foregroundStyle(Theme.textPrimary)

                // P2P badge
                if let name = channel?.name,
                   appState.p2pDMActive.contains(name.lowercased()) {
                    Label("Direct", systemImage: "point.3.connected.trianglepath.dotted")
                        .font(.caption2)
                        .foregroundStyle(Theme.success)
                        .padding(.horizontal, 6)
                        .padding(.vertical, 2)
                        .background(Capsule().fill(Theme.success.opacity(0.10)))
                }

                if !isChannel {
                    Text(isOnline ? (awayMsg != nil ? "away" : "online") : "offline")
                        .font(.caption)
                        .foregroundStyle(isOnline ? (awayMsg != nil ? Theme.warning : Theme.success) : Theme.textSecondary)

                    // E2EE badge for DMs
                    if let did = ProfileCache.shared.did(for: channel?.name ?? ""),
                       E2eeManager.shared.hasSession(remoteDid: did) {
                        HStack(spacing: 3) {
                            Image(systemName: "lock.shield.fill")
                                .font(.caption2)
                            Text("Encrypted")
                                .font(.caption2)
                        }
                        .foregroundStyle(Theme.success)
                        .padding(.horizontal, 6)
                        .padding(.vertical, 2)
                        .background(Capsule().fill(Theme.success.opacity(0.10)))
                    }
                }
            }

            if isChannel, let topic = channel?.topic, !topic.isEmpty {
                Divider().frame(height: 16)
                Text(topic)
                    .font(.subheadline)
                    .foregroundStyle(Theme.textSecondary)
                    .lineLimit(1)
                    .help(topic)
            }

            Spacer()

            // Search
            Button {
                appState.showSearch.toggle()
            } label: {
                Image(systemName: "magnifyingglass")
                    .foregroundStyle(appState.showSearch ? Theme.textPrimary : Theme.textTertiary)
            }
            .buttonStyle(.plain)
            .help("Search (⌘F)")

            // Member count + settings (channels only)
            if isChannel {
                Button {
                    showSettings = true
                } label: {
                    Label("\(channel?.members.count ?? 0)", systemImage: "person.2")
                        .font(.caption)
                        .foregroundStyle(Theme.textSecondary)
                }
                .buttonStyle(.plain)
                .help("Channel settings")
                .sheet(isPresented: $showSettings) {
                    if let ch = channel {
                        ChannelSettingsSheet(channel: ch)
                            .environment(appState)
                    }
                }
            }

            // Detail panel toggle
            Button {
                appState.showDetailPanel.toggle()
            } label: {
                Image(systemName: "sidebar.trailing")
                    .foregroundStyle(appState.showDetailPanel ? Theme.textPrimary : Theme.textTertiary)
            }
            .buttonStyle(.plain)
            .help("Toggle detail panel")
        }
        .padding(.horizontal, 16)
        .frame(height: 58)
        .background(Theme.surface)
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
    @State private var timer: Timer?

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
            // Stash the Timer in @State so .onDisappear can invalidate
            // it. Without this, recreating the view (chat-switch, list
            // diffing) accumulates phantom timers that keep mutating
            // the destroyed @State and leak memory across long sessions.
            timer = Timer.scheduledTimer(withTimeInterval: 0.4, repeats: true) { _ in
                phase = (phase + 1) % 3
            }
        }
        .onDisappear {
            timer?.invalidate()
            timer = nil
        }
    }
}
