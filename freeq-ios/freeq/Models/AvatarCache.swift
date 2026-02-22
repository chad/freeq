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

    /// Request avatar fetch for a nick (if not already cached/pending).
    func prefetch(_ nick: String) {
        let key = nick.lowercased()
        guard cache[key] == nil, !pending.contains(key), !failed.contains(key) else { return }
        pending.insert(key)

        Task {
            await fetchAvatar(nick: nick, key: key)
        }
    }

    /// Prefetch avatars for a list of nicks.
    func prefetchAll(_ nicks: [String]) {
        for nick in nicks {
            prefetch(nick)
        }
    }

    private func fetchAvatar(nick: String, key: String) async {
        // Try the nick as an AT handle — could be "chadfowler.com" or "alice.bsky.social"
        // Also try with .bsky.social suffix if no dots
        let handles = nick.contains(".") ? [nick] : ["\(nick).bsky.social"]

        for handle in handles {
            if let url = await resolveAvatar(handle: handle) {
                cache[key] = url
                pending.remove(key)
                return
            }
        }

        failed.insert(key)
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
