import SwiftUI

struct SidebarView: View {
    @EnvironmentObject var appState: AppState
    @Binding var showingSidebar: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            // Header
            HStack(spacing: 10) {
                Image("FreeqLogo")
                    .resizable()
                    .scaledToFit()
                    .frame(width: 32, height: 32)
                    .clipShape(RoundedRectangle(cornerRadius: 8))

                Text("freeq")
                    .font(.system(size: 20, weight: .bold, design: .rounded))
                    .foregroundColor(Theme.accent)

                Spacer()

                // Connection status dot
                Circle()
                    .fill(statusColor)
                    .frame(width: 8, height: 8)
            }
            .padding(.horizontal, 16)
            .frame(height: 56)
            .background(Theme.bgSecondary)

            Rectangle()
                .fill(Theme.border)
                .frame(height: 1)

            // Content
            ScrollView {
                VStack(alignment: .leading, spacing: 2) {
                    // Channels
                    sectionHeader("CHANNELS", count: appState.channels.count)

                    ForEach(appState.channels) { channel in
                        channelRow(channel)
                    }

                    // DMs
                    if !appState.dmBuffers.isEmpty {
                        sectionHeader("DIRECT MESSAGES", count: appState.dmBuffers.count)
                            .padding(.top, 12)

                        ForEach(appState.dmBuffers) { dm in
                            dmRow(dm)
                        }
                    }
                }
                .padding(.vertical, 8)
            }

            Rectangle()
                .fill(Theme.border)
                .frame(height: 1)

            // User footer
            HStack(spacing: 12) {
                // Avatar
                ZStack {
                    Circle()
                        .fill(Theme.nickColor(for: appState.nick).opacity(0.2))
                        .frame(width: 36, height: 36)
                    Text(String(appState.nick.prefix(1)).uppercased())
                        .font(.system(size: 14, weight: .bold))
                        .foregroundColor(Theme.nickColor(for: appState.nick))
                }

                VStack(alignment: .leading, spacing: 2) {
                    HStack(spacing: 4) {
                        Text(appState.nick)
                            .font(.system(size: 14, weight: .semibold))
                            .foregroundColor(Theme.textPrimary)
                            .lineLimit(1)

                        if appState.authenticatedDID != nil {
                            Image(systemName: "checkmark.seal.fill")
                                .font(.system(size: 10))
                                .foregroundColor(Theme.accent)
                        }
                    }

                    Text(appState.authenticatedDID ?? "Guest")
                        .font(.system(size: 11))
                        .foregroundColor(Theme.textMuted)
                        .lineLimit(1)
                }

                Spacer()

                // Settings / disconnect
                Menu {
                    Button(role: .destructive, action: {
                        appState.disconnect()
                        showingSidebar = false
                    }) {
                        Label("Disconnect", systemImage: "rectangle.portrait.and.arrow.right")
                    }
                } label: {
                    Image(systemName: "ellipsis")
                        .font(.system(size: 16))
                        .foregroundColor(Theme.textMuted)
                        .frame(width: 32, height: 32)
                }
            }
            .padding(.horizontal, 16)
            .padding(.vertical, 12)
            .background(Theme.bgSecondary)
        }
        .background(Theme.bgPrimary)
    }

    private var statusColor: Color {
        switch appState.connectionState {
        case .registered: return Theme.success
        case .connected, .connecting: return Theme.warning
        case .disconnected: return Theme.danger
        }
    }

    private func sectionHeader(_ title: String, count: Int) -> some View {
        HStack {
            Text(title)
                .font(.system(size: 11, weight: .bold))
                .foregroundColor(Theme.textMuted)
                .kerning(0.8)
            Spacer()
            Text("\(count)")
                .font(.system(size: 11, weight: .medium))
                .foregroundColor(Theme.textMuted)
        }
        .padding(.horizontal, 16)
        .padding(.top, 12)
        .padding(.bottom, 4)
    }

    private func channelRow(_ channel: ChannelState) -> some View {
        let isActive = appState.activeChannel == channel.name

        return Button(action: {
            appState.activeChannel = channel.name
            showingSidebar = false
        }) {
            HStack(spacing: 8) {
                Text("#")
                    .font(.system(size: 16, weight: .medium, design: .monospaced))
                    .foregroundColor(isActive ? Theme.accent : Theme.textMuted)
                    .frame(width: 20)

                Text(String(channel.name.dropFirst()))
                    .font(.system(size: 15, weight: isActive ? .semibold : .regular))
                    .foregroundColor(isActive ? Theme.textPrimary : Theme.textSecondary)
                    .lineLimit(1)

                Spacer()

                let unread = appState.unreadCounts[channel.name] ?? 0
                if unread > 0 {
                    Text("\(unread)")
                        .font(.system(size: 11, weight: .bold))
                        .foregroundColor(.white)
                        .padding(.horizontal, 6)
                        .padding(.vertical, 2)
                        .background(Theme.accent)
                        .cornerRadius(10)
                } else if !channel.members.isEmpty {
                    Text("\(channel.members.count)")
                        .font(.system(size: 11))
                        .foregroundColor(Theme.textMuted)
                        .padding(.horizontal, 6)
                        .padding(.vertical, 2)
                        .background(Theme.bgTertiary)
                        .cornerRadius(4)
                }
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 8)
            .background(isActive ? Theme.accent.opacity(0.12) : Color.clear)
            .cornerRadius(8)
        }
        .buttonStyle(.plain)
        .padding(.horizontal, 4)
    }

    private func dmRow(_ dm: ChannelState) -> some View {
        let isActive = appState.activeChannel == dm.name

        return Button(action: {
            appState.activeChannel = dm.name
            showingSidebar = false
        }) {
            HStack(spacing: 10) {
                ZStack {
                    Circle()
                        .fill(Theme.nickColor(for: dm.name).opacity(0.2))
                        .frame(width: 28, height: 28)
                    Text(String(dm.name.prefix(1)).uppercased())
                        .font(.system(size: 11, weight: .bold))
                        .foregroundColor(Theme.nickColor(for: dm.name))
                }

                Text(dm.name)
                    .font(.system(size: 15, weight: isActive ? .semibold : .regular))
                    .foregroundColor(isActive ? Theme.textPrimary : Theme.textSecondary)
                    .lineLimit(1)

                Spacer()
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 6)
            .background(isActive ? Theme.accent.opacity(0.12) : Color.clear)
            .cornerRadius(8)
        }
        .buttonStyle(.plain)
        .padding(.horizontal, 4)
    }
}
