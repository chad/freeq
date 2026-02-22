import SwiftUI

/// Profile sheet shown when tapping a user in member list or message header.
struct UserProfileSheet: View {
    @EnvironmentObject var appState: AppState
    @Environment(\.dismiss) var dismiss
    let nick: String
    @State private var profile: BlueskyProfile? = nil
    @State private var loading = true

    var body: some View {
        NavigationView {
            ZStack {
                Theme.bgPrimary.ignoresSafeArea()

                ScrollView {
                    VStack(spacing: 20) {
                        // Avatar
                        UserAvatar(nick: nick, size: 80)
                            .padding(.top, 24)

                        // Nick + verified
                        VStack(spacing: 4) {
                            HStack(spacing: 6) {
                                Text(nick)
                                    .font(.system(size: 22, weight: .bold))
                                    .foregroundColor(Theme.textPrimary)

                                if profile != nil {
                                    VerifiedBadge(size: 16)
                                }
                            }

                            if let p = profile {
                                if let displayName = p.displayName, !displayName.isEmpty {
                                    Text(displayName)
                                        .font(.system(size: 15))
                                        .foregroundColor(Theme.textSecondary)
                                }
                                Text("@\(p.handle)")
                                    .font(.system(size: 13))
                                    .foregroundColor(Theme.textMuted)
                            }
                        }

                        // Bio
                        if let bio = profile?.description, !bio.isEmpty {
                            Text(bio)
                                .font(.system(size: 14))
                                .foregroundColor(Theme.textSecondary)
                                .multilineTextAlignment(.center)
                                .padding(.horizontal, 32)
                        }

                        // Stats
                        if let p = profile {
                            HStack(spacing: 24) {
                                statItem(count: p.followersCount ?? 0, label: "Followers")
                                statItem(count: p.followsCount ?? 0, label: "Following")
                                statItem(count: p.postsCount ?? 0, label: "Posts")
                            }
                            .padding(.vertical, 8)
                        }

                        // Actions
                        VStack(spacing: 12) {
                            // DM button
                            if nick.lowercased() != appState.nick.lowercased() {
                                Button(action: startDM) {
                                    HStack(spacing: 8) {
                                        Image(systemName: "bubble.left.fill")
                                            .font(.system(size: 13))
                                        Text("Message")
                                            .font(.system(size: 15, weight: .semibold))
                                    }
                                    .frame(maxWidth: .infinity)
                                    .padding(.vertical, 12)
                                    .background(Theme.accent)
                                    .foregroundColor(.white)
                                    .cornerRadius(10)
                                }
                            }

                            // View on Bluesky
                            if let p = profile {
                                Link(destination: URL(string: "https://bsky.app/profile/\(p.handle)")!) {
                                    HStack(spacing: 8) {
                                        Image(systemName: "arrow.up.right")
                                            .font(.system(size: 13))
                                        Text("View on Bluesky")
                                            .font(.system(size: 15, weight: .medium))
                                    }
                                    .frame(maxWidth: .infinity)
                                    .padding(.vertical, 12)
                                    .background(Theme.bgTertiary)
                                    .foregroundColor(Theme.textPrimary)
                                    .cornerRadius(10)
                                }
                            }
                        }
                        .padding(.horizontal, 24)

                        if loading {
                            ProgressView()
                                .tint(Theme.textMuted)
                                .padding(.top, 8)
                        }

                        Spacer()
                    }
                }
            }
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Done") { dismiss() }
                        .foregroundColor(Theme.accent)
                }
            }
        }
        .preferredColorScheme(.dark)
        .task { await fetchProfile() }
    }

    private func statItem(count: Int, label: String) -> some View {
        VStack(spacing: 2) {
            Text(formatCount(count))
                .font(.system(size: 16, weight: .bold))
                .foregroundColor(Theme.textPrimary)
            Text(label)
                .font(.system(size: 11))
                .foregroundColor(Theme.textMuted)
        }
    }

    private func formatCount(_ n: Int) -> String {
        if n >= 1_000_000 { return "\(n / 1_000_000)M" }
        if n >= 1_000 { return "\(n / 1_000)K" }
        return "\(n)"
    }

    private func startDM() {
        let _ = appState.getOrCreateDM(nick)
        appState.activeChannel = nick
        dismiss()
    }

    private func fetchProfile() async {
        let handles = nick.contains(".") ? [nick] : ["\(nick).bsky.social"]

        for handle in handles {
            let urlStr = "https://public.api.bsky.app/xrpc/app.bsky.actor.getProfile?actor=\(handle.addingPercentEncoding(withAllowedCharacters: .urlQueryAllowed) ?? handle)"
            guard let url = URL(string: urlStr) else { continue }

            do {
                let (data, response) = try await URLSession.shared.data(from: url)
                guard (response as? HTTPURLResponse)?.statusCode == 200 else { continue }
                let json = try JSONSerialization.jsonObject(with: data) as? [String: Any]
                guard let json = json else { continue }

                await MainActor.run {
                    profile = BlueskyProfile(
                        handle: json["handle"] as? String ?? handle,
                        displayName: json["displayName"] as? String,
                        description: json["description"] as? String,
                        avatar: json["avatar"] as? String,
                        followersCount: json["followersCount"] as? Int,
                        followsCount: json["followsCount"] as? Int,
                        postsCount: json["postsCount"] as? Int
                    )
                    loading = false
                }
                return
            } catch { }
        }

        await MainActor.run { loading = false }
    }
}

struct BlueskyProfile {
    let handle: String
    let displayName: String?
    let description: String?
    let avatar: String?
    let followersCount: Int?
    let followsCount: Int?
    let postsCount: Int?
}
