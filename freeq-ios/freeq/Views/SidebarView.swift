import SwiftUI

struct SidebarView: View {
    @EnvironmentObject var appState: AppState
    @Binding var showingSidebar: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            // Header
            HStack {
                Text("freeq")
                    .font(.system(size: 22, weight: .bold, design: .rounded))
                    .foregroundColor(.accentColor)

                Spacer()

                if appState.authenticatedDID != nil {
                    Image(systemName: "checkmark.seal.fill")
                        .foregroundColor(.green)
                        .font(.caption)
                }
            }
            .padding(.horizontal, 16)
            .padding(.vertical, 14)
            .background(Color(.secondarySystemBackground))

            Divider()

            ScrollView {
                VStack(alignment: .leading, spacing: 4) {
                    // Channels section
                    if !appState.channels.isEmpty {
                        Text("CHANNELS")
                            .font(.caption)
                            .fontWeight(.bold)
                            .foregroundColor(.secondary)
                            .padding(.horizontal, 16)
                            .padding(.top, 12)

                        ForEach(appState.channels) { channel in
                            Button(action: {
                                appState.activeChannel = channel.name
                                showingSidebar = false
                            }) {
                                HStack(spacing: 8) {
                                    Text("#")
                                        .font(.system(size: 16, weight: .medium, design: .monospaced))
                                        .foregroundColor(.secondary)

                                    Text(channel.name.dropFirst())
                                        .font(.system(size: 15))
                                        .lineLimit(1)

                                    Spacer()

                                    Text("\(channel.members.count)")
                                        .font(.caption)
                                        .foregroundColor(.secondary)
                                }
                                .padding(.horizontal, 16)
                                .padding(.vertical, 8)
                                .background(
                                    appState.activeChannel == channel.name
                                        ? Color.accentColor.opacity(0.15)
                                        : Color.clear
                                )
                                .cornerRadius(8)
                            }
                            .buttonStyle(.plain)
                            .padding(.horizontal, 4)
                        }
                    }

                    // DMs section
                    if !appState.dmBuffers.isEmpty {
                        Text("DIRECT MESSAGES")
                            .font(.caption)
                            .fontWeight(.bold)
                            .foregroundColor(.secondary)
                            .padding(.horizontal, 16)
                            .padding(.top, 12)

                        ForEach(appState.dmBuffers) { dm in
                            Button(action: {
                                appState.activeChannel = dm.name
                                showingSidebar = false
                            }) {
                                HStack(spacing: 8) {
                                    Circle()
                                        .fill(Color.green)
                                        .frame(width: 8, height: 8)

                                    Text(dm.name)
                                        .font(.system(size: 15))
                                        .lineLimit(1)

                                    Spacer()
                                }
                                .padding(.horizontal, 16)
                                .padding(.vertical, 8)
                                .background(
                                    appState.activeChannel == dm.name
                                        ? Color.accentColor.opacity(0.15)
                                        : Color.clear
                                )
                                .cornerRadius(8)
                            }
                            .buttonStyle(.plain)
                            .padding(.horizontal, 4)
                        }
                    }
                }
            }

            Divider()

            // User footer
            HStack(spacing: 10) {
                Circle()
                    .fill(Color.green)
                    .frame(width: 10, height: 10)

                Text(appState.nick)
                    .font(.system(size: 15, weight: .medium))
                    .lineLimit(1)

                Spacer()

                Button(action: { appState.disconnect() }) {
                    Image(systemName: "rectangle.portrait.and.arrow.right")
                        .foregroundColor(.secondary)
                }
            }
            .padding(.horizontal, 16)
            .padding(.vertical, 12)
            .background(Color(.secondarySystemBackground))
        }
        .background(Color(.systemBackground))
        .clipShape(RoundedRectangle(cornerRadius: 0))
        .shadow(radius: 8)
    }
}
