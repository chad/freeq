import Foundation
import SwiftUI

/// Fetches and caches Bluesky profile data for IRC members.
@Observable
class ProfileCache {
    static let shared = ProfileCache()

    struct Profile {
        let did: String?
        let handle: String?
        let displayName: String?
        let avatarURL: URL?
        let description: String?
        let followersCount: Int?
        let followsCount: Int?
        let postsCount: Int?
    }

    private var cache: [String: Profile] = [:]  // lowercase nick → profile
    private var fetching: Set<String> = []
    private var didMap: [String: String] = [:]  // lowercase nick → DID
    private var nickForDid: [String: String] = [:]  // DID → nick

    /// Get cached profile for a nick (or nil if not fetched yet).
    func profile(for nick: String) -> Profile? {
        cache[nick.lowercased()]
    }

    /// Get DID for a nick.
    func did(for nick: String) -> String? {
        didMap[nick.lowercased()]
    }

    /// Set DID for a nick (from WHOIS 330 or account-notify).
    func setDid(_ did: String, for nick: String) {
        let lower = nick.lowercased()
        didMap[lower] = did
        nickForDid[did] = nick
        // Trigger profile fetch if not cached
        if cache[lower] == nil && !fetching.contains(lower) {
            fetchProfile(nick: nick, did: did)
        }
    }

    /// Rename tracking.
    func renameUser(from oldNick: String, to newNick: String) {
        let oldLower = oldNick.lowercased()
        let newLower = newNick.lowercased()
        if let did = didMap.removeValue(forKey: oldLower) {
            didMap[newLower] = did
            nickForDid[did] = newNick
        }
        if let profile = cache.removeValue(forKey: oldLower) {
            cache[newLower] = profile
        }
    }

    /// Prefetch profiles for all members in a channel.
    func prefetchAll(_ nicks: [String]) {
        for nick in nicks {
            let lower = nick.lowercased()
            guard cache[lower] == nil, !fetching.contains(lower) else { continue }
            // Only fetch if we know their DID
            if let did = didMap[lower] {
                fetchProfile(nick: nick, did: did)
            }
        }
    }

    /// Fetch profile from Bluesky public API.
    func fetchProfile(nick: String, did: String) {
        let lower = nick.lowercased()
        fetching.insert(lower)

        Task { [weak self] in
            defer {
                DispatchQueue.main.async { self?.fetching.remove(lower) }
            }

            let urlString = "https://public.api.bsky.app/xrpc/app.bsky.actor.getProfile?actor=\(did)"
            guard let url = URL(string: urlString) else { return }

            do {
                let (data, response) = try await URLSession.shared.data(from: url)
                guard let httpResponse = response as? HTTPURLResponse,
                      httpResponse.statusCode == 200 else { return }

                let json = try JSONSerialization.jsonObject(with: data) as? [String: Any] ?? [:]
                let profile = Profile(
                    did: json["did"] as? String,
                    handle: json["handle"] as? String,
                    displayName: json["displayName"] as? String,
                    avatarURL: (json["avatar"] as? String).flatMap(URL.init(string:)),
                    description: json["description"] as? String,
                    followersCount: json["followersCount"] as? Int,
                    followsCount: json["followsCount"] as? Int,
                    postsCount: json["postsCount"] as? Int
                )

                await MainActor.run {
                    self?.cache[lower] = profile
                }
            } catch {
                // Silent failure — profile fetch is best-effort
            }
        }
    }
}

/// Async image loader with memory + disk caching.
///
/// Reads `ProfileCache.shared.profile(for: nick)` inside the body so
/// the @Observable cache tracking sees the dependency — that way when
/// WHOIS lands the DID, ProfileCache fetches the bsky profile, the
/// cache mutates, this view re-renders, and the `.task(id: avatarURL)`
/// fires with the freshly-arrived URL. The prior implementation called
/// `loadAvatar()` once from `.onAppear` and never retried, so any view
/// that mounted before the profile was cached just stayed on its
/// fallback initial.
struct AvatarView: View {
    let nick: String
    let size: CGFloat
    @State private var image: NSImage?

    private static var memoryCache: [URL: NSImage] = [:]
    private static let diskCacheDir: URL = {
        let caches = FileManager.default.urls(for: .cachesDirectory, in: .userDomainMask)[0]
        let dir = caches.appendingPathComponent("at.freeq.macos/avatars", isDirectory: true)
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        return dir
    }()

    var body: some View {
        // Read inside body so @Observable tracks the dependency on
        // ProfileCache.shared.cache; mutations rebuild this body.
        let avatarURL = ProfileCache.shared.profile(for: nick)?.avatarURL

        return Group {
            if let image {
                Image(nsImage: image)
                    .resizable()
                    .aspectRatio(contentMode: .fill)
                    .frame(width: size, height: size)
                    .clipShape(Circle())
            } else {
                ZStack {
                    Circle()
                        .fill(Theme.nickColor(for: nick).opacity(0.2))
                        .frame(width: size, height: size)
                    Text(String(nick.prefix(1)).uppercased())
                        .font(.system(size: size * 0.4, weight: .bold))
                        .foregroundStyle(Theme.nickColor(for: nick))
                }
            }
        }
        .task(id: avatarURL) {
            // Re-runs whenever avatarURL changes: nil→URL (profile just
            // arrived), URL→URL' (rare — user changed avatar), or
            // URL→nil (profile evicted). Each transition gets a fresh
            // load attempt.
            guard let url = avatarURL else {
                image = nil
                return
            }
            await loadImage(from: url)
        }
    }

    private func loadImage(from url: URL) async {
        if let cached = Self.memoryCache[url] {
            await MainActor.run { image = cached }
            return
        }
        let diskFile = Self.diskCacheDir.appendingPathComponent(url.lastPathComponent)
        if let data = try? Data(contentsOf: diskFile),
           let nsImage = NSImage(data: data) {
            Self.memoryCache[url] = nsImage
            await MainActor.run { image = nsImage }
            return
        }
        do {
            let (data, _) = try await URLSession.shared.data(from: url)
            if let nsImage = NSImage(data: data) {
                Self.memoryCache[url] = nsImage
                try? data.write(to: diskFile)
                await MainActor.run { image = nsImage }
            }
        } catch {
            Log.media.error("Avatar fetch failed for \(nick): \(error.localizedDescription)")
        }
    }
}
