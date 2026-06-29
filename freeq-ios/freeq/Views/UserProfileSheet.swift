import SwiftUI

/// Profile sheet shown when tapping a user in member list or message header.
struct UserProfileSheet: View {
    @EnvironmentObject var appState: AppState
    @Environment(\.dismiss) var dismiss
    let nick: String
    /// Set when opened from a federated message (+freeq.at/origin = peer name).
    /// The sender is peer-vouched by that server, not verified here — so we warn
    /// and suppress the local verified badge.
    var origin: String? = nil
    @State private var profile: BlueskyProfile? = nil
    @State private var loading = true
    @State private var recentPosts: [BskyFeedItem] = []
    @State private var loadingFeed = false

    var body: some View {
        NavigationView {
            ZStack {
                Theme.bgPrimary.ignoresSafeArea()

                ScrollView {
                    VStack(spacing: 20) {
                        // Federated provenance warning — opened from a relayed message.
                        if let origin {
                            HStack(spacing: 8) {
                                Image(systemName: "exclamationmark.triangle.fill")
                                    .font(.system(size: 13))
                                Text("Relayed via \(origin) — this server did not verify this identity.")
                                    .font(.system(size: 12))
                                Spacer()
                            }
                            .foregroundColor(Theme.warning)
                            .padding(.horizontal, 16)
                            .padding(.vertical, 10)
                            .frame(maxWidth: .infinity)
                            .background(Theme.warning.opacity(0.15))
                        }

                        // Avatar
                        UserAvatar(nick: nick, size: 80)
                            .padding(.top, 24)

                        // Nick + verified
                        VStack(spacing: 4) {
                            HStack(spacing: 6) {
                                Text(nick)
                                    .font(.system(size: 22, weight: .bold))
                                    .foregroundColor(Theme.textPrimary)

                                if profile != nil && origin == nil {
                                    VerifiedBadge(size: 16)
                                }
                            }

                            if let away = appState.awayMessage(for: nick) {
                                HStack(spacing: 6) {
                                    Circle()
                                        .fill(Theme.warning)
                                        .frame(width: 8, height: 8)
                                    Text("Away")
                                        .font(.system(size: 12, weight: .semibold))
                                        .foregroundColor(Theme.warning)
                                }
                                if !away.isEmpty {
                                    Text(away)
                                        .font(.system(size: 13))
                                        .foregroundColor(Theme.textMuted)
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

                        // Recent Bluesky posts
                        if !recentPosts.isEmpty {
                            VStack(alignment: .leading, spacing: 12) {
                                Text("Recent Posts")
                                    .font(.system(size: 13, weight: .semibold))
                                    .foregroundColor(Theme.textMuted)
                                    .padding(.horizontal, 16)

                                ForEach(recentPosts, id: \.uri) { item in
                                    if let rkey = item.uri.split(separator: "/").last.map(String.init),
                                       let handle = profile?.handle {
                                        BlueskyEmbed(handle: handle, rkey: rkey)
                                            .padding(.horizontal, 16)
                                    }
                                }
                            }
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .padding(.top, 16)
                        } else if loadingFeed {
                            ProgressView()
                                .tint(Theme.textMuted)
                                .padding(.top, 16)
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
        appState.pendingDMNick = nick
        dismiss()
    }

    private func fetchProfile() async {
        // Resolve ONLY by the server-verified DID. We must NOT fall back to
        // the nick (nor a guessed "<nick>.bsky.social"): nicks are freely
        // chosen, so resolving from one shows a STRANGER's profile, avatar
        // and feed for whoever happens to match — e.g. tapping the AI being
        // "olive" surfacing the unrelated real account olive.bsky.social.
        // Identity on freeq is the DID bound at SASL; without one there is no
        // Bluesky profile to show. did:key users (guests, AI beings) have
        // none either.
        let did = await MainActor.run { () -> String? in
            // 1. Member-roster DID.
            for ch in appState.channels {
                if let m = ch.members.first(where: { $0.nick.lowercased() == nick.lowercased() }),
                   let d = m.did, !d.isEmpty {
                    return d
                }
            }
            // 2. DID seen on a message's account tag (retained by AvatarCache).
            //    Covers senders whose DID rode the message, not the roster —
            //    custom-domain handles and federated senders. This is the same
            //    source that resolves their avatar + verified badge on the row.
            if let cached = AvatarCache.shared.did(for: nick), !cached.isEmpty {
                return cached
            }
            // 3. Own profile: the authenticated session DID (mirrors Android).
            if nick.lowercased() == appState.nick.lowercased(),
               let own = appState.authenticatedDID, !own.isEmpty {
                return own
            }
            return nil
        }
        guard let actor = did, !actor.hasPrefix("did:key:") else {
            await MainActor.run { loading = false }
            return
        }

        let urlStr = "https://public.api.bsky.app/xrpc/app.bsky.actor.getProfile?actor=\(actor.addingPercentEncoding(withAllowedCharacters: .urlQueryAllowed) ?? actor)"
        guard let url = URL(string: urlStr) else {
            await MainActor.run { loading = false }
            return
        }

        do {
            let (data, response) = try await URLSession.shared.data(from: url)
            guard (response as? HTTPURLResponse)?.statusCode == 200 else {
                await MainActor.run { loading = false }
                return
            }
            let json = try JSONSerialization.jsonObject(with: data) as? [String: Any]
            guard let json = json else {
                await MainActor.run { loading = false }
                return
            }

            // Trust the DID-resolved handle, not the nick.
            let resolvedHandle = json["handle"] as? String ?? actor
            await MainActor.run {
                profile = BlueskyProfile(
                    handle: resolvedHandle,
                    displayName: json["displayName"] as? String,
                    description: json["description"] as? String,
                    avatar: json["avatar"] as? String,
                    followersCount: json["followersCount"] as? Int,
                    followsCount: json["followsCount"] as? Int,
                    postsCount: json["postsCount"] as? Int
                )
                loading = false
                loadingFeed = true
            }
            await fetchAuthorFeed(actor: resolvedHandle)
        } catch {
            await MainActor.run { loading = false }
        }
    }

    private func fetchAuthorFeed(actor: String) async {
        let urlStr = "https://public.api.bsky.app/xrpc/app.bsky.feed.getAuthorFeed?actor=\(actor.addingPercentEncoding(withAllowedCharacters: .urlQueryAllowed) ?? actor)&limit=5&filter=posts_no_replies"
        guard let url = URL(string: urlStr) else {
            await MainActor.run { loadingFeed = false }
            return
        }
        do {
            let (data, response) = try await URLSession.shared.data(from: url)
            guard (response as? HTTPURLResponse)?.statusCode == 200 else {
                await MainActor.run { loadingFeed = false }
                return
            }
            let json = try JSONSerialization.jsonObject(with: data) as? [String: Any]
            let feed = (json?["feed"] as? [[String: Any]]) ?? []
            let items: [BskyFeedItem] = feed.compactMap { entry in
                guard let post = entry["post"] as? [String: Any],
                      let uri = post["uri"] as? String else { return nil }
                let record = post["record"] as? [String: Any]
                let text = record?["text"] as? String ?? ""
                return BskyFeedItem(uri: uri, text: text)
            }
            await MainActor.run {
                recentPosts = items
                loadingFeed = false
            }
        } catch {
            await MainActor.run { loadingFeed = false }
        }
    }
}

struct BskyFeedItem {
    let uri: String
    let text: String
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
