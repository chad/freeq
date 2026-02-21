import SwiftUI

struct TopBarView: View {
    @EnvironmentObject var appState: AppState
    @Binding var showingSidebar: Bool
    @Binding var showingJoinSheet: Bool

    var body: some View {
        HStack(spacing: 12) {
            // Hamburger
            Button(action: { showingSidebar.toggle() }) {
                Image(systemName: "line.3.horizontal")
                    .font(.title2)
                    .foregroundColor(.primary)
            }

            // Channel name
            VStack(alignment: .leading, spacing: 2) {
                if let channel = appState.activeChannel {
                    Text(channel)
                        .font(.headline)
                        .lineLimit(1)
                } else {
                    Text("freeq")
                        .font(.headline)
                }

                if let topic = appState.activeChannelState?.topic, !topic.isEmpty {
                    Text(topic)
                        .font(.caption)
                        .foregroundColor(.secondary)
                        .lineLimit(1)
                }
            }

            Spacer()

            // Join channel
            Button(action: { showingJoinSheet = true }) {
                Image(systemName: "number.square")
                    .font(.title2)
                    .foregroundColor(.primary)
            }
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 10)
        .background(Color(.systemBackground))
        .overlay(
            Divider(), alignment: .bottom
        )
    }
}
