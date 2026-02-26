import SwiftUI

/// Live channel from server API
struct ServerChannel: Identifiable {
    let name: String
    let topic: String
    let memberCount: Int
    var id: String { name }
}

/// Channel discovery — browse and join channels.
struct DiscoverTab: View {
    @EnvironmentObject var appState: AppState
    @State private var channelInput = ""
    @State private var serverChannels: [ServerChannel] = []
    @State private var loading = false
    @State private var searchText = ""

    private var filteredChannels: [ServerChannel] {
        if searchText.isEmpty { return serverChannels }
        return serverChannels.filter { $0.name.localizedCaseInsensitiveContains(searchText) || $0.topic.localizedCaseInsensitiveContains(searchText) }
    }

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

                        // Live channels from server
                        VStack(alignment: .leading, spacing: 12) {
                            HStack {
                                Text("CHANNELS")
                                    .font(.system(size: 11, weight: .bold))
                                    .foregroundColor(Theme.textMuted)
                                    .kerning(1)
                                Spacer()
                                if loading {
                                    ProgressView().tint(Theme.textMuted).scaleEffect(0.7)
                                } else {
                                    Text("\(serverChannels.count)")
                                        .font(.system(size: 11, weight: .medium))
                                        .foregroundColor(Theme.textMuted)
                                }
                            }
                            .padding(.horizontal, 16)

                            if !serverChannels.isEmpty {
                                // Search filter
                                HStack(spacing: 8) {
                                    Image(systemName: "magnifyingglass")
                                        .font(.system(size: 13))
                                        .foregroundColor(Theme.textMuted)
                                    TextField("", text: $searchText, prompt: Text("Filter channels").foregroundColor(Theme.textMuted))
                                        .foregroundColor(Theme.textPrimary)
                                        .font(.system(size: 14))
                                        .autocapitalization(.none)
                                }
                                .padding(.horizontal, 12)
                                .padding(.vertical, 8)
                                .background(Theme.bgTertiary)
                                .cornerRadius(8)
                                .padding(.horizontal, 16)
                            }

                            ForEach(filteredChannels) { ch in
                                let joined = appState.channels.contains { $0.name.lowercased() == ch.name.lowercased() }

                                Button(action: { appState.joinChannel(ch.name) }) {
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
                                            Text(ch.name)
                                                .font(.system(size: 16, weight: .medium))
                                                .foregroundColor(Theme.textPrimary)

                                            if !ch.topic.isEmpty {
                                                Text(ch.topic)
                                                    .font(.system(size: 13))
                                                    .foregroundColor(Theme.textMuted)
                                                    .lineLimit(1)
                                            }
                                        }

                                        Spacer()

                                        VStack(alignment: .trailing, spacing: 2) {
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

                                            HStack(spacing: 3) {
                                                Image(systemName: "person.2.fill")
                                                    .font(.system(size: 9))
                                                Text("\(ch.memberCount)")
                                                    .font(.system(size: 11))
                                            }
                                            .foregroundColor(Theme.textMuted)
                                        }
                                    }
                                    .padding(.horizontal, 16)
                                    .padding(.vertical, 6)
                                }
                                .buttonStyle(.plain)
                            }

                            if serverChannels.isEmpty && !loading {
                                Text("Connect to see available channels")
                                    .font(.system(size: 14))
                                    .foregroundColor(Theme.textMuted)
                                    .frame(maxWidth: .infinity)
                                    .padding(.vertical, 24)
                            }
                        }
                    }
                    .padding(.top, 16)
                }
                .refreshable {
                    await fetchChannels()
                }
            }
            .navigationTitle("Discover")
            .navigationBarTitleDisplayMode(.inline)
            .toolbarBackground(Theme.bgSecondary, for: .navigationBar)
            .toolbarBackground(.visible, for: .navigationBar)
        }
        .task { await fetchChannels() }
    }

    private func fetchChannels() async {
        loading = true
        defer { loading = false }

        guard let url = URL(string: "https://irc.freeq.at/api/v1/channels") else { return }
        do {
            let (data, response) = try await URLSession.shared.data(from: url)
            guard (response as? HTTPURLResponse)?.statusCode == 200 else { return }
            if let json = try? JSONSerialization.jsonObject(with: data) as? [[String: Any]] {
                let channels = json.compactMap { ch -> ServerChannel? in
                    guard let name = ch["name"] as? String else { return nil }
                    let topic = ch["topic"] as? String ?? ""
                    let members = ch["member_count"] as? Int ?? ch["members"] as? Int ?? 0
                    return ServerChannel(name: name, topic: topic, memberCount: members)
                }
                .filter { $0.memberCount > 0 }
                .sorted { $0.memberCount > $1.memberCount }

                await MainActor.run { serverChannels = channels }
            }
        } catch { }
    }

    private func joinCustom() {
        let name = channelInput.trimmingCharacters(in: .whitespaces)
        guard !name.isEmpty else { return }
        let channel = name.hasPrefix("#") ? name : "#\(name)"
        appState.joinChannel(channel)
        channelInput = ""
    }
}
