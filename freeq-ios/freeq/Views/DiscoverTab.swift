import SwiftUI

/// Channel discovery — browse and join channels.
struct DiscoverTab: View {
    @EnvironmentObject var appState: AppState
    @State private var searchText = ""
    @State private var channelInput = ""

    var body: some View {
        NavigationStack {
            ZStack {
                Theme.bgPrimary.ignoresSafeArea()

                ScrollView {
                    VStack(spacing: 24) {
                        // Join custom channel
                        VStack(alignment: .leading, spacing: 10) {
                            Text("JOIN A CHANNEL")
                                .font(.system(size: 11, weight: .bold))
                                .foregroundColor(Theme.textMuted)
                                .kerning(1)

                            HStack(spacing: 10) {
                                Text("#")
                                    .font(.system(size: 18, weight: .medium))
                                    .foregroundColor(Theme.textMuted)

                                TextField("channel-name", text: $channelInput)
                                    .foregroundColor(Theme.textPrimary)
                                    .font(.system(size: 16))
                                    .autocapitalization(.none)
                                    .disableAutocorrection(true)
                                    .submitLabel(.join)
                                    .onSubmit { joinCustom() }

                                Button(action: joinCustom) {
                                    Image(systemName: "arrow.right.circle.fill")
                                        .font(.system(size: 24))
                                        .foregroundColor(channelInput.isEmpty ? Theme.textMuted : Theme.accent)
                                }
                                .disabled(channelInput.isEmpty)
                            }
                            .padding(.horizontal, 14)
                            .padding(.vertical, 12)
                            .background(Theme.bgSecondary)
                            .cornerRadius(10)
                            .overlay(
                                RoundedRectangle(cornerRadius: 10)
                                    .stroke(Theme.border, lineWidth: 1)
                            )
                        }
                        .padding(.horizontal, 16)

                        // Popular channels
                        VStack(alignment: .leading, spacing: 12) {
                            Text("POPULAR CHANNELS")
                                .font(.system(size: 11, weight: .bold))
                                .foregroundColor(Theme.textMuted)
                                .kerning(1)
                                .padding(.horizontal, 16)

                            ForEach(popularChannels, id: \.self) { channel in
                                Button(action: { appState.joinChannel(channel) }) {
                                    HStack(spacing: 12) {
                                        ZStack {
                                            Circle()
                                                .fill(Theme.accent.opacity(0.15))
                                                .frame(width: 44, height: 44)
                                            Text("#")
                                                .font(.system(size: 18, weight: .bold, design: .rounded))
                                                .foregroundColor(Theme.accent)
                                        }

                                        VStack(alignment: .leading, spacing: 2) {
                                            Text(channel)
                                                .font(.system(size: 16, weight: .medium))
                                                .foregroundColor(Theme.textPrimary)

                                            Text(channelDescription(channel))
                                                .font(.system(size: 13))
                                                .foregroundColor(Theme.textMuted)
                                        }

                                        Spacer()

                                        let joined = appState.channels.contains { $0.name == channel }
                                        if joined {
                                            Text("Joined")
                                                .font(.system(size: 12, weight: .medium))
                                                .foregroundColor(Theme.textMuted)
                                        } else {
                                            Text("Join")
                                                .font(.system(size: 13, weight: .semibold))
                                                .foregroundColor(Theme.accent)
                                                .padding(.horizontal, 14)
                                                .padding(.vertical, 6)
                                                .background(Theme.accent.opacity(0.15))
                                                .clipShape(Capsule())
                                        }
                                    }
                                    .padding(.horizontal, 16)
                                    .padding(.vertical, 6)
                                }
                                .buttonStyle(.plain)
                            }
                        }
                    }
                    .padding(.top, 16)
                }
            }
            .navigationTitle("Discover")
            .navigationBarTitleDisplayMode(.inline)
            .toolbarBackground(Theme.bgSecondary, for: .navigationBar)
            .toolbarBackground(.visible, for: .navigationBar)
        }
    }

    private var popularChannels: [String] {
        ["#general", "#freeq", "#dev", "#music", "#random", "#crypto", "#gaming"]
    }

    private func channelDescription(_ channel: String) -> String {
        switch channel {
        case "#general": return "General discussion"
        case "#freeq": return "freeq development & support"
        case "#dev": return "Programming & technology"
        case "#music": return "Music recommendations"
        case "#random": return "Off-topic chat"
        case "#crypto": return "Cryptocurrency discussion"
        case "#gaming": return "Games & gaming"
        default: return ""
        }
    }

    private func joinCustom() {
        let name = channelInput.trimmingCharacters(in: .whitespaces)
        guard !name.isEmpty else { return }
        let channel = name.hasPrefix("#") ? name : "#\(name)"
        appState.joinChannel(channel)
        channelInput = ""
    }
}
