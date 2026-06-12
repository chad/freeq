import SwiftUI

/// Fetches and caches Bluesky avatar URLs from the public API.
@MainActor
class AvatarCache: ObservableObject {
    static let shared = AvatarCache()

    @Published private var cache: [String: URL] = [:]  // nick -> avatar URL
    private var pending: Set<String> = []
    private var failed: Set<String> = []  // Don't retry failed lookups

    /// Get cached avatar URL for a nick. Returns nil if not yet fetched.
    func avatarURL(for nick: String) -> URL? {
        cache[nick]
    }

    /// Request avatar fetch for a nick. Resolution requires a verified
    /// `did` — see `fetchAvatar` for why we never resolve from the nick.
    /// A no-DID call is a no-op: we simply wait for the DID to arrive
    /// (account-tag on a message, account-notify, or WHOIS) and resolve then.
    func prefetch(_ nick: String, did: String? = nil) {
        let key = nick.lowercased()
        // Skip guest nicks - they're not Bluesky accounts.
        guard !key.hasPrefix("guest"), !key.hasPrefix("web") else { return }
        if cache[key] != nil || pending.contains(key) { return }
        // Identity on freeq is the DID the server bound at SASL — never the
        // freely-settable nick. Without a verified DID there is nothing we
        // can safely resolve, and `did:key` users (guests, AI beings) have
        // no Bluesky profile at all.
        guard let did = did, !did.isEmpty, !did.hasPrefix("did:key:") else { return }
        if failed.contains(key) { return }
        pending.insert(key)

        Task {
            await fetchAvatar(nick: nick, key: key, did: did)
        }
    }

    /// Prefetch avatars for a list of nicks.
    func prefetchAll(_ nicks: [String]) {
        for nick in nicks {
            prefetch(nick)
        }
    }

    private func fetchAvatar(nick: String, key: String, did: String? = nil) async {
        // Resolve ONLY by the server-verified DID. We must never derive a
        // Bluesky identity from the nick — neither the bare nick as a handle
        // nor a guessed "<nick>.bsky.social". Nicks are freely chosen, so any
        // such guess shows a STRANGER's photo and handle for whoever happens
        // to match (e.g. the AI being "olive" pulling up the unrelated real
        // account olive.bsky.social). That is impersonation. `prefetch`
        // already guarantees a non-empty, non-did:key DID here.
        guard let did = did, !did.isEmpty else {
            failed.insert(key)
            pending.remove(key)
            return
        }
        if let url = await resolveAvatar(handle: did) {
            cache[key] = url
        } else {
            failed.insert(key)
        }
        pending.remove(key)
    }

    private func resolveAvatar(handle: String) async -> URL? {
        let urlString = "https://public.api.bsky.app/xrpc/app.bsky.actor.getProfile?actor=\(handle.addingPercentEncoding(withAllowedCharacters: .urlQueryAllowed) ?? handle)"
        guard let url = URL(string: urlString) else { return nil }

        do {
            let (data, response) = try await URLSession.shared.data(from: url)
            guard (response as? HTTPURLResponse)?.statusCode == 200 else { return nil }
            let json = try JSONSerialization.jsonObject(with: data) as? [String: Any]
            if let avatarStr = json?["avatar"] as? String, let avatarURL = URL(string: avatarStr) {
                return avatarURL
            }
        } catch { }
        return nil
    }
}

/// SwiftUI view that displays a user's avatar (cached Bluesky profile pic or initial).
struct UserAvatar: View {
    let nick: String
    let size: CGFloat
    @StateObject private var cache = AvatarCache.shared

    var body: some View {
        Group {
            if let url = cache.avatarURL(for: nick.lowercased()) {
                AsyncImage(url: url) { image in
                    image.resizable().scaledToFill()
                } placeholder: {
                    initialCircle
                }
                .frame(width: size, height: size)
                .clipShape(Circle())
            } else {
                initialCircle
                    .onAppear { cache.prefetch(nick) }
            }
        }
    }

    private var initialCircle: some View {
        ZStack {
            Circle()
                .fill(nickColor)
                .frame(width: size, height: size)
            Text(String(nick.prefix(1)).uppercased())
                .font(.system(size: size * 0.4, weight: .semibold))
                .foregroundColor(.white)
        }
    }

    private var nickColor: Color {
        let colors: [Color] = [
            Color(hex: "e74c3c"), Color(hex: "3498db"), Color(hex: "2ecc71"),
            Color(hex: "f39c12"), Color(hex: "9b59b6"), Color(hex: "1abc9c"),
            Color(hex: "e67e22"), Color(hex: "e91e63"),
        ]
        let hash = nick.lowercased().unicodeScalars.reduce(0) { $0 + Int($1.value) }
        return colors[hash % colors.count]
    }
}
