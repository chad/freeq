import SwiftUI

/// OG metadata fetched from server proxy
struct OGData {
    var title: String?
    var description: String?
    var image: String?
    var siteName: String?
}

/// Cache to avoid re-fetching
private actor OGCache {
    static let shared = OGCache()
    private var cache: [String: OGData?] = [:]

    func get(_ url: String) -> OGData?? {
        return cache[url]
    }

    func set(_ url: String, data: OGData?) {
        cache[url] = data
    }
}

/// Rich link preview — fetches OG metadata from server proxy.
struct LinkPreviewCard: View {
    let url: URL
    @State private var ogData: OGData? = nil
    @State private var loaded = false

    private var domain: String {
        url.host?.replacingOccurrences(of: "www.", with: "") ?? url.absoluteString
    }

    private var icon: String {
        let d = domain.lowercased()
        if d.contains("github.com") { return "chevron.left.forwardslash.chevron.right" }
        if d.contains("twitter.com") || d.contains("x.com") { return "at" }
        if d.contains("bsky.app") { return "bird" }
        if d.contains("reddit.com") { return "bubble.left.and.bubble.right" }
        if d.contains("wikipedia.org") { return "book" }
        if d.contains("apple.com") { return "apple.logo" }
        return "link"
    }

    var body: some View {
        Link(destination: url) {
            VStack(alignment: .leading, spacing: 0) {
                // OG image
                if let imageUrl = ogData?.image, let imgURL = URL(string: imageUrl) {
                    AsyncImage(url: imgURL) { phase in
                        switch phase {
                        case .success(let image):
                            image
                                .resizable()
                                .aspectRatio(contentMode: .fill)
                                .frame(maxWidth: 300, maxHeight: 160)
                                .clipped()
                        default:
                            EmptyView()
                        }
                    }
                }

                HStack(spacing: 8) {
                    Image(systemName: icon)
                        .font(.system(size: 13))
                        .foregroundColor(Theme.accent)
                        .frame(width: 28, height: 28)
                        .background(Theme.accent.opacity(0.1))
                        .cornerRadius(6)

                    VStack(alignment: .leading, spacing: 2) {
                        if let siteName = ogData?.siteName {
                            Text(siteName)
                                .font(.system(size: 10, weight: .medium))
                                .foregroundColor(Theme.textMuted)
                                .textCase(.uppercase)
                        }

                        Text(ogData?.title ?? domain)
                            .font(.system(size: 13, weight: .semibold))
                            .foregroundColor(Theme.accent)
                            .lineLimit(2)

                        if let desc = ogData?.description {
                            Text(desc)
                                .font(.system(size: 11))
                                .foregroundColor(Theme.textMuted)
                                .lineLimit(2)
                        }

                        if ogData == nil && loaded {
                            Text(url.path.count > 1 ? String(url.path.prefix(50)) : domain)
                                .font(.system(size: 11))
                                .foregroundColor(Theme.textMuted)
                                .lineLimit(1)
                        }
                    }

                    Spacer()

                    Image(systemName: "arrow.up.right.square")
                        .font(.system(size: 12))
                        .foregroundColor(Theme.textMuted)
                }
                .padding(10)
            }
            .background(Theme.bgTertiary)
            .cornerRadius(10)
            .overlay(
                RoundedRectangle(cornerRadius: 10)
                    .stroke(Theme.border, lineWidth: 1)
            )
        }
        .buttonStyle(.plain)
        .frame(maxWidth: 300)
        .task {
            await fetchOG()
        }
    }

    private func fetchOG() async {
        // Check cache
        if let cached = await OGCache.shared.get(url.absoluteString) {
            ogData = cached
            loaded = true
            return
        }

        // Fetch from server proxy
        let encoded = url.absoluteString.addingPercentEncoding(withAllowedCharacters: .urlQueryAllowed) ?? ""
        guard let apiURL = URL(string: "https://irc.freeq.at/api/v1/og?url=\(encoded)") else {
            loaded = true
            return
        }

        do {
            var request = URLRequest(url: apiURL)
            request.timeoutInterval = 6
            let (data, response) = try await URLSession.shared.data(for: request)
            guard let http = response as? HTTPURLResponse, http.statusCode == 200 else {
                await OGCache.shared.set(url.absoluteString, data: nil)
                loaded = true
                return
            }

            if let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any] {
                let og = OGData(
                    title: json["title"] as? String,
                    description: json["description"] as? String,
                    image: json["image"] as? String,
                    siteName: json["site_name"] as? String
                )
                let hasData = og.title != nil || og.description != nil || og.image != nil
                await OGCache.shared.set(url.absoluteString, data: hasData ? og : nil)
                if hasData {
                    await MainActor.run { ogData = og }
                }
            }
        } catch {
            await OGCache.shared.set(url.absoluteString, data: nil)
        }
        await MainActor.run { loaded = true }
    }
}
