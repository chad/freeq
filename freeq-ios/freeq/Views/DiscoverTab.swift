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
    @State private var loading = true
    @State private var searchText = ""
    @FocusState private var joinFocused: Bool

    private var filteredChannels: [ServerChannel] {
        let channels = serverChannels
        if searchText.isEmpty { return channels }
        let q = searchText.lowercased()
        return channels.filter {
            $0.name.lowercased().contains(q) ||
            $0.topic.lowercased().contains(q)
        }
    }

    var body: some View {
        NavigationStack {
            ZStack {
                Theme.bgPrimary.ignoresSafeArea()

                VStack(spacing: 0) {
                    // Search bar (always visible at top)
                    HStack(spacing: 10) {
                        Image(systemName: "magnifyingglass")
                            .font(.system(size: 15))
                            .foregroundColor(Theme.textMuted)

                        TextField("", text: $searchText, prompt: Text("Search channels...").foregroundColor(Theme.textMuted))
                            .foregroundColor(Theme.textPrimary)
                            .font(.system(size: 16))
                            .autocapitalization(.none)
                            .disableAutocorrection(true)
                            .submitLabel(.search)

                        if !searchText.isEmpty {
                            Button(action: { searchText = "" }) {
                                Image(systemName: "xmark.circle.fill")
                                    .font(.system(size: 16))
                                    .foregroundColor(Theme.textMuted)
                            }
                        }
                    }
                    .padding(.horizontal, 14)
                    .padding(.vertical, 10)
                    .background(Theme.bgSecondary)
                    .cornerRadius(12)
                    .padding(.horizontal, 16)
                    .padding(.top, 8)
                    .padding(.bottom, 4)

                    // Quick join bar
                    HStack(spacing: 8) {
                        Text("#")
                            .font(.system(size: 16, weight: .medium, design: .monospaced))
                            .foregroundColor(Theme.textMuted)

                        TextField("", text: $channelInput, prompt: Text("Join by name...").foregroundColor(Theme.textMuted))
                            .foregroundColor(Theme.textPrimary)
                            .font(.system(size: 15))
                            .autocapitalization(.none)
                            .disableAutocorrection(true)
                            .submitLabel(.join)
                            .focused($joinFocused)
                            .onSubmit { joinCustom() }

                        if !channelInput.isEmpty {
                            Button(action: joinCustom) {
                                Text("Join")
                                    .font(.system(size: 14, weight: .semibold))
                                    .foregroundColor(.white)
                                    .padding(.horizontal, 14)
                                    .padding(.vertical, 6)
                                    .background(Theme.accent)
                                    .cornerRadius(8)
                            }
                        }
                    }
                    .padding(.horizontal, 16)
                    .padding(.vertical, 8)
                    .background(Theme.bgSecondary.opacity(0.5))

                    Rectangle().fill(Theme.border).frame(height: 1)

                    // Channel list
                    if loading && serverChannels.isEmpty {
                        Spacer()
                        VStack(spacing: 12) {
                            ProgressView().tint(Theme.accent).scaleEffect(1.1)
                            Text("Loading channels...")
                                .font(.system(size: 14))
                                .foregroundColor(Theme.textMuted)
                        }
                        Spacer()
                    } else if filteredChannels.isEmpty {
                        Spacer()
                        VStack(spacing: 12) {
                            Image(systemName: searchText.isEmpty ? "bubble.left.and.bubble.right" : "magnifyingglass")
                                .font(.system(size: 36))
                                .foregroundColor(Theme.textMuted)
                            if searchText.isEmpty {
                                Text("No active channels")
                                    .font(.system(size: 16, weight: .medium))
                                    .foregroundColor(Theme.textSecondary)
                            } else {
                                Text("No channels matching \"\(searchText)\"")
                                    .font(.system(size: 15))
                                    .foregroundColor(Theme.textSecondary)
                                Button("Create #\(searchText)") {
                                    channelInput = searchText
                                    joinCustom()
                                }
                                .font(.system(size: 14, weight: .medium))
                                .foregroundColor(Theme.accent)
                            }
                        }
                        Spacer()
                    } else {
                        ScrollView {
                            LazyVStack(spacing: 0) {
                                // Result count
                                if !searchText.isEmpty {
                                    HStack {
                                        Text("\(filteredChannels.count) result\(filteredChannels.count == 1 ? "" : "s")")
                                            .font(.system(size: 12))
                                            .foregroundColor(Theme.textMuted)
                                        Spacer()
                                    }
                                    .padding(.horizontal, 16)
                                    .padding(.top, 8)
                                    .padding(.bottom, 4)
                                }

                                ForEach(filteredChannels) { ch in
                                    channelRow(ch)
                                }
                            }
                            .padding(.bottom, 16)
                        }
                        .refreshable { await fetchChannels() }
                    }
                }
            }
            .navigationTitle("Discover")
            .navigationBarTitleDisplayMode(.inline)
            .toolbarBackground(Theme.bgSecondary, for: .navigationBar)
            .toolbarBackground(.visible, for: .navigationBar)
        }
        .task { await fetchChannels() }
    }

    private func channelRow(_ ch: ServerChannel) -> some View {
        let joined = appState.channels.contains { $0.name.lowercased() == ch.name.lowercased() }

        return Button(action: {
            appState.joinChannel(ch.name)
            // Switch to Chats tab
            if joined {
                appState.activeChannel = ch.name
            }
            UIImpactFeedbackGenerator(style: .light).impactOccurred()
        }) {
            HStack(spacing: 12) {
                // Channel icon
                ZStack {
                    RoundedRectangle(cornerRadius: 12)
                        .fill(Theme.accent.opacity(joined ? 0.2 : 0.1))
                        .frame(width: 48, height: 48)
                    Text("#")
                        .font(.system(size: 20, weight: .bold, design: .rounded))
                        .foregroundColor(Theme.accent.opacity(joined ? 1 : 0.7))
                }

                VStack(alignment: .leading, spacing: 3) {
                    HStack(spacing: 6) {
                        Text(ch.name)
                            .font(.system(size: 16, weight: .medium))
                            .foregroundColor(Theme.textPrimary)

                        HStack(spacing: 3) {
                            Image(systemName: "person.2.fill")
                                .font(.system(size: 9))
                            Text("\(ch.memberCount)")
                                .font(.system(size: 12))
                        }
                        .foregroundColor(Theme.textMuted)
                    }

                    if !ch.topic.isEmpty {
                        Text(ch.topic)
                            .font(.system(size: 13))
                            .foregroundColor(Theme.textSecondary)
                            .lineLimit(2)
                    }
                }

                Spacer()

                if joined {
                    Image(systemName: "checkmark.circle.fill")
                        .font(.system(size: 20))
                        .foregroundColor(Theme.success)
                } else {
                    Text("Join")
                        .font(.system(size: 14, weight: .semibold))
                        .foregroundColor(Theme.accent)
                        .padding(.horizontal, 16)
                        .padding(.vertical, 7)
                        .background(Theme.accent.opacity(0.12))
                        .cornerRadius(8)
                }
            }
            .padding(.horizontal, 16)
            .padding(.vertical, 10)
        }
        .buttonStyle(.plain)
    }

    private func fetchChannels() async {
        loading = true
        defer { loading = false }

        guard let url = URL(string: "https://irc.freeq.at/api/v1/channels") else { return }
        do {
            var request = URLRequest(url: url)
            request.timeoutInterval = 8
            let (data, response) = try await URLSession.shared.data(for: request)
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
        joinFocused = false
        UIImpactFeedbackGenerator(style: .medium).impactOccurred()
        ToastManager.shared.show("Joining \(channel)", icon: "number")
    }
}
