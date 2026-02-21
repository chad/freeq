import SwiftUI

struct TopBarView: View {
    @EnvironmentObject var appState: AppState
    @Binding var showingSidebar: Bool
    @Binding var showingJoinSheet: Bool
    @Binding var showingMembers: Bool

    var body: some View {
        HStack(spacing: 14) {
            // Hamburger
            Button(action: { showingSidebar.toggle() }) {
                Image(systemName: "line.3.horizontal")
                    .font(.system(size: 20, weight: .medium))
                    .foregroundColor(Theme.textSecondary)
                    .frame(width: 36, height: 36)
            }

            // Channel info
            VStack(alignment: .leading, spacing: 2) {
                if let channel = appState.activeChannel {
                    HStack(spacing: 6) {
                        if channel.hasPrefix("#") {
                            Text("#")
                                .font(.system(size: 18, weight: .bold, design: .monospaced))
                                .foregroundColor(Theme.textMuted)
                            Text(String(channel.dropFirst()))
                                .font(.system(size: 17, weight: .bold))
                                .foregroundColor(Theme.textPrimary)
                        } else {
                            Circle()
                                .fill(Theme.success)
                                .frame(width: 8, height: 8)
                            Text(channel)
                                .font(.system(size: 17, weight: .bold))
                                .foregroundColor(Theme.textPrimary)
                        }
                    }
                } else {
                    Text("freeq")
                        .font(.system(size: 17, weight: .bold))
                        .foregroundColor(Theme.textPrimary)
                }

                if let topic = appState.activeChannelState?.topic, !topic.isEmpty {
                    Text(topic)
                        .font(.system(size: 12))
                        .foregroundColor(Theme.textMuted)
                        .lineLimit(1)
                }
            }

            Spacer()

            // Member count badge
            if let channel = appState.activeChannelState, channel.name.hasPrefix("#") {
                Button(action: { showingMembers.toggle() }) {
                    HStack(spacing: 4) {
                        Image(systemName: "person.2.fill")
                            .font(.system(size: 13))
                        Text("\(channel.members.count)")
                            .font(.system(size: 13, weight: .medium))
                    }
                    .foregroundColor(showingMembers ? Theme.accent : Theme.textMuted)
                    .padding(.horizontal, 10)
                    .padding(.vertical, 6)
                    .background(showingMembers ? Theme.accent.opacity(0.15) : Theme.bgTertiary)
                    .cornerRadius(8)
                }
            }

            // Join channel
            Button(action: { showingJoinSheet = true }) {
                Image(systemName: "plus.bubble")
                    .font(.system(size: 18))
                    .foregroundColor(Theme.textSecondary)
                    .frame(width: 36, height: 36)
            }
        }
        .padding(.horizontal, 12)
        .frame(height: 56)
        .background(Theme.bgSecondary)
        .overlay(
            Rectangle()
                .fill(Theme.border)
                .frame(height: 1),
            alignment: .bottom
        )
    }
}
