import SwiftUI

/// Settings — account info, connection, about.
struct SettingsTab: View {
    @EnvironmentObject var appState: AppState

    var body: some View {
        NavigationStack {
            ZStack {
                Theme.bgPrimary.ignoresSafeArea()

                List {
                    // Account section
                    Section {
                        HStack(spacing: 14) {
                            // Avatar
                            ZStack {
                                Circle()
                                    .fill(Theme.accent.opacity(0.15))
                                    .frame(width: 56, height: 56)

                                Text(String(appState.nick.prefix(1)).uppercased())
                                    .font(.system(size: 24, weight: .semibold))
                                    .foregroundColor(Theme.accent)
                            }

                            VStack(alignment: .leading, spacing: 4) {
                                Text(appState.nick)
                                    .font(.system(size: 18, weight: .semibold))
                                    .foregroundColor(Theme.textPrimary)

                                if let did = appState.authenticatedDID {
                                    HStack(spacing: 4) {
                                        Image(systemName: "checkmark.seal.fill")
                                            .font(.system(size: 12))
                                            .foregroundColor(Theme.accent)
                                        Text("Verified")
                                            .font(.system(size: 13))
                                            .foregroundColor(Theme.accent)
                                    }
                                } else {
                                    Text("Guest")
                                        .font(.system(size: 13))
                                        .foregroundColor(Theme.textMuted)
                                }
                            }

                            Spacer()
                        }
                        .padding(.vertical, 4)
                        .listRowBackground(Theme.bgSecondary)
                    }

                    // Connection
                    Section("Connection") {
                        HStack {
                            Label("Server", systemImage: "server.rack")
                                .foregroundColor(Theme.textPrimary)
                            Spacer()
                            Text(appState.serverAddress)
                                .font(.system(size: 14))
                                .foregroundColor(Theme.textMuted)
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
                                    .foregroundColor(Theme.textMuted)
                            }
                        }
                        .listRowBackground(Theme.bgSecondary)

                        HStack {
                            Label("Channels", systemImage: "number")
                                .foregroundColor(Theme.textPrimary)
                            Spacer()
                            Text("\(appState.channels.count)")
                                .font(.system(size: 14))
                                .foregroundColor(Theme.textMuted)
                        }
                        .listRowBackground(Theme.bgSecondary)
                    }

                    // About
                    Section("About") {
                        HStack {
                            Label("Version", systemImage: "info.circle")
                                .foregroundColor(Theme.textPrimary)
                            Spacer()
                            Text("1.0.0")
                                .font(.system(size: 14))
                                .foregroundColor(Theme.textMuted)
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
                    }

                    // Disconnect
                    Section {
                        Button(action: {
                            appState.disconnect()
                        }) {
                            HStack {
                                Spacer()
                                Text("Disconnect")
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
        case .registered: return .green
        case .connected, .connecting: return .yellow
        case .disconnected: return .red
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
