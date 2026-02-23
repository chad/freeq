import SwiftUI

struct SettingsTab: View {
    @EnvironmentObject var appState: AppState

    var body: some View {
        NavigationStack {
            ZStack {
                Theme.bgPrimary.ignoresSafeArea()

                List {
                    // Account
                    Section {
                        HStack(spacing: 12) {
                            UserAvatar(nick: appState.nick, size: 48)

                            VStack(alignment: .leading, spacing: 3) {
                                HStack(spacing: 5) {
                                    Text(appState.nick)
                                        .font(.system(size: 17, weight: .semibold))
                                        .foregroundColor(Theme.textPrimary)

                                    if appState.authenticatedDID != nil {
                                        VerifiedBadge(size: 14)
                                    }
                                }

                                if let did = appState.authenticatedDID {
                                    Text(did)
                                        .font(.system(size: 11, design: .monospaced))
                                        .foregroundColor(Theme.textMuted)
                                        .lineLimit(1)
                                } else {
                                    Text("Guest")
                                        .font(.system(size: 13))
                                        .foregroundColor(Theme.textSecondary)
                                }
                            }
                        }
                        .listRowBackground(Theme.bgSecondary)
                    } header: {
                        Text("Account")
                            .foregroundColor(Theme.textMuted)
                    }

                    // Appearance
                    Section {
                        Toggle(isOn: Binding(
                            get: { !appState.isDarkTheme },
                            set: { _ in appState.toggleTheme() }
                        )) {
                            Label("Light Theme", systemImage: "sun.max.fill")
                                .foregroundColor(Theme.textPrimary)
                        }
                        .tint(Theme.accent)
                        .listRowBackground(Theme.bgSecondary)
                    } header: {
                        Text("Appearance")
                            .foregroundColor(Theme.textMuted)
                    }

                    // Connection
                    Section {
                        HStack {
                            Label("Server", systemImage: "server.rack")
                                .foregroundColor(Theme.textPrimary)
                            Spacer()
                            Text(appState.serverAddress)
                                .font(.system(size: 14))
                                .foregroundColor(Theme.textSecondary)
                        }
                        .listRowBackground(Theme.bgSecondary)

                        HStack {
                            Label("Status", systemImage: "circle.fill")
                                .foregroundColor(Theme.textPrimary)
                            Spacer()
                            HStack(spacing: 6) {
                                Circle()
                                    .fill(statusColor)
                                    .frame(width: 8, height: 8)
                                Text(statusText)
                                    .font(.system(size: 14))
                                    .foregroundColor(Theme.textSecondary)
                            }
                        }
                        .listRowBackground(Theme.bgSecondary)

                        HStack {
                            Label("Channels", systemImage: "number")
                                .foregroundColor(Theme.textPrimary)
                            Spacer()
                            Text("\(appState.channels.count)")
                                .font(.system(size: 14))
                                .foregroundColor(Theme.textSecondary)
                        }
                        .listRowBackground(Theme.bgSecondary)
                    } header: {
                        Text("Connection")
                            .foregroundColor(Theme.textMuted)
                    }

                    // About
                    Section {
                        HStack {
                            Label("Version", systemImage: "info.circle")
                                .foregroundColor(Theme.textPrimary)
                            Spacer()
                            Text("1.0.0")
                                .font(.system(size: 14))
                                .foregroundColor(Theme.textSecondary)
                        }
                        .listRowBackground(Theme.bgSecondary)

                        Link(destination: URL(string: "https://freeq.at")!) {
                            HStack {
                                Label("Website", systemImage: "globe")
                                    .foregroundColor(Theme.textPrimary)
                                Spacer()
                                Image(systemName: "arrow.up.right")
                                    .font(.system(size: 12))
                                    .foregroundColor(Theme.textMuted)
                            }
                        }
                        .listRowBackground(Theme.bgSecondary)

                        Link(destination: URL(string: "https://github.com/chad/freeq")!) {
                            HStack {
                                Label("Source Code", systemImage: "chevron.left.forwardslash.chevron.right")
                                    .foregroundColor(Theme.textPrimary)
                                Spacer()
                                Image(systemName: "arrow.up.right")
                                    .font(.system(size: 12))
                                    .foregroundColor(Theme.textMuted)
                            }
                        }
                        .listRowBackground(Theme.bgSecondary)
                    } header: {
                        Text("About")
                            .foregroundColor(Theme.textMuted)
                    }

                    // Disconnect / Logout
                    Section {
                        Button(action: { appState.disconnect() }) {
                            HStack {
                                Spacer()
                                Text("Disconnect")
                                    .font(.system(size: 16, weight: .medium))
                                    .foregroundColor(Theme.textSecondary)
                                Spacer()
                            }
                        }
                        .listRowBackground(Theme.bgSecondary)

                        Button(action: { appState.logout() }) {
                            HStack {
                                Spacer()
                                Text("Log Out")
                                    .font(.system(size: 16, weight: .medium))
                                    .foregroundColor(Theme.danger)
                                Spacer()
                            }
                        }
                        .listRowBackground(Theme.bgSecondary)
                    }
                }
                .listStyle(.insetGrouped)
                .scrollContentBackground(.hidden)
            }
            .navigationTitle("Settings")
            .navigationBarTitleDisplayMode(.inline)
            .toolbarBackground(Theme.bgSecondary, for: .navigationBar)
            .toolbarBackground(.visible, for: .navigationBar)
        }
    }

    private var statusColor: Color {
        switch appState.connectionState {
        case .registered: return Theme.success
        case .connected, .connecting: return Theme.warning
        case .disconnected: return Theme.danger
        }
    }

    private var statusText: String {
        switch appState.connectionState {
        case .registered: return "Connected"
        case .connected: return "Registering..."
        case .connecting: return "Connecting..."
        case .disconnected: return "Disconnected"
        }
    }
}
