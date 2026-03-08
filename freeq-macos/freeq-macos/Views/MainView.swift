import SwiftUI

/// Three-column layout: Sidebar | Messages | Detail
struct MainView: View {
    @Environment(AppState.self) private var appState

    var body: some View {
        Group {
            if appState.connectionState == .disconnected && appState.brokerToken == nil {
                ConnectView()
            } else if appState.connectionState == .connecting {
                connectingView
            } else {
                ZStack(alignment: .top) {
                    NavigationSplitView {
                        SidebarView()
                            .navigationSplitViewColumnWidth(min: 180, ideal: 220, max: 300)
                    } detail: {
                        if appState.activeChannel != nil {
                            HStack(spacing: 0) {
                                ChatView()
                                if let threadRoot = appState.threadRootMessage,
                                   let channel = appState.activeChannel {
                                    Divider()
                                    ThreadView(rootMessage: threadRoot, channel: channel)
                                }
                                if appState.showDetailPanel {
                                    DetailPanel()
                                        .frame(width: 260)
                                }
                            }
                        } else {
                            emptyState
                        }
                    }
                    .toolbar {
                        ToolbarItem(placement: .navigation) {
                            connectionIndicator
                        }
                    }

                    // Reconnect banner
                    if appState.connectionState == .disconnected && appState.hasSavedSession {
                        ReconnectBanner()
                    }

                    // MOTD banner
                    if appState.showMotd && !appState.motd.isEmpty {
                        MotdBanner()
                    }

                    // Guest upgrade banner
                    if appState.connectionState == .registered && appState.authenticatedDID == nil {
                        GuestUpgradeBanner()
                    }
                }
            }
        }
        .sheet(isPresented: Binding(
            get: { appState.showJoinSheet },
            set: { appState.showJoinSheet = $0 }
        )) {
            JoinChannelSheet()
        }
        .onReceive(NotificationCenter.default.publisher(for: .cancelEdit)) { _ in
            appState.editingMessageId = nil
            appState.editingText = nil
            appState.replyingToMessage = nil
        }
        .alert("Error", isPresented: Binding(
            get: { appState.errorMessage != nil },
            set: { if !$0 { appState.errorMessage = nil } }
        )) {
            Button("OK") { appState.errorMessage = nil }
        } message: {
            Text(appState.errorMessage ?? "")
        }
    }

    private var connectingView: some View {
        VStack(spacing: 16) {
            ProgressView()
                .scaleEffect(1.5)
            Text("Connecting to \(appState.serverAddress)…")
                .foregroundStyle(.secondary)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    private var emptyState: some View {
        VStack(spacing: 12) {
            Image(systemName: "bubble.left.and.bubble.right")
                .font(.system(size: 48))
                .foregroundStyle(.tertiary)
            Text("Select a channel to start chatting")
                .foregroundStyle(.secondary)

            if appState.channels.isEmpty {
                Button("Join #freeq") {
                    appState.joinChannel("#freeq")
                }
                .buttonStyle(.borderedProminent)
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    @ViewBuilder
    private var connectionIndicator: some View {
        HStack(spacing: 6) {
            Circle()
                .fill(statusColor)
                .frame(width: 8, height: 8)

            if appState.isP2pActive {
                Image(systemName: "point.3.connected.trianglepath.dotted")
                    .font(.caption2)
                    .foregroundStyle(.secondary)
                    .help("P2P active via iroh")
            }

            if appState.transportType == .iroh {
                Text("iroh")
                    .font(.caption2)
                    .foregroundStyle(.secondary)
            }
        }
    }

    private var statusColor: Color {
        switch appState.connectionState {
        case .registered: .green
        case .connected: .yellow
        case .connecting: .orange
        case .disconnected: .red
        }
    }
}

// MARK: - MOTD Banner

struct MotdBanner: View {
    @Environment(AppState.self) private var appState
    @State private var expanded = false

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack {
                Image(systemName: "info.circle")
                    .font(.caption)
                Text("Message of the Day")
                    .font(.caption.weight(.semibold))
                Spacer()
                Button(expanded ? "Collapse" : "Show") {
                    withAnimation { expanded.toggle() }
                }
                .font(.caption)
                .buttonStyle(.plain)
                Button {
                    appState.showMotd = false
                } label: {
                    Image(systemName: "xmark")
                        .font(.caption2)
                }
                .buttonStyle(.plain)
            }
            if expanded {
                Text(appState.motd.trimmingCharacters(in: .whitespacesAndNewlines))
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .textSelection(.enabled)
            }
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 6)
        .background(.ultraThinMaterial)
    }
}

// MARK: - Guest Upgrade Banner

struct GuestUpgradeBanner: View {
    @Environment(AppState.self) private var appState

    var body: some View {
        HStack(spacing: 8) {
            Image(systemName: "person.badge.key")
                .font(.caption)
            Text("You're connected as a guest.")
                .font(.caption.weight(.medium))
            Text("Sign in with AT Protocol for DMs, history, and identity.")
                .font(.caption)
                .foregroundStyle(.secondary)
            Spacer()
            Button("Sign In") {
                appState.disconnect()
                appState.brokerToken = nil
            }
            .font(.caption)
            .buttonStyle(.bordered)
            .controlSize(.small)
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 8)
        .background(.blue.opacity(0.1))
        .foregroundStyle(.blue)
    }
}

// MARK: - Reconnect Banner

struct ReconnectBanner: View {
    @Environment(AppState.self) private var appState

    var body: some View {
        HStack(spacing: 8) {
            Image(systemName: "wifi.exclamationmark")
                .font(.caption)
            Text("Disconnected — reconnecting…")
                .font(.caption.weight(.medium))
            Spacer()
            Button("Reconnect Now") {
                appState.reconnectIfSaved()
            }
            .font(.caption)
            .buttonStyle(.bordered)
            .controlSize(.small)
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 8)
        .background(.red.opacity(0.15))
        .foregroundStyle(.red)
    }
}
