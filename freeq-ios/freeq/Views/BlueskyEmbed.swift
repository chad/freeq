import SwiftUI

/// Renders a rich card for Bluesky post URLs.
struct BlueskyEmbed: View {
    let handle: String
    let rkey: String

    @State private var post: BskyPost? = nil
    @State private var loading = true

    struct BskyPost {
        let text: String
        let authorHandle: String
        let authorName: String?
        let authorAvatar: URL?
        let images: [URL]
        let likeCount: Int
        let repostCount: Int
    }

    var body: some View {
        if let post = post {
            Link(destination: URL(string: "https://bsky.app/profile/\(handle)/post/\(rkey)")!) {
                VStack(alignment: .leading, spacing: 8) {
                    // Author
                    HStack(spacing: 8) {
                        if let avatar = post.authorAvatar {
                            AsyncImage(url: avatar) { image in
                                image.resizable().aspectRatio(contentMode: .fill)
                            } placeholder: {
                                Circle().fill(Theme.bgTertiary)
                            }
                            .frame(width: 20, height: 20)
                            .clipShape(Circle())
                        }

                        Text(post.authorName ?? post.authorHandle)
                            .font(.system(size: 13, weight: .semibold))
                            .foregroundColor(Theme.textPrimary)

                        Text("@\(post.authorHandle)")
                            .font(.system(size: 12))
                            .foregroundColor(Theme.textMuted)

                        Spacer()

                        // Bluesky butterfly
                        Image(systemName: "bird")
                            .font(.system(size: 12))
                            .foregroundColor(Color(hex: "0085FF"))
                    }

                    // Post text
                    Text(post.text)
                        .font(.system(size: 14))
                        .foregroundColor(Theme.textPrimary)
                        .lineLimit(4)
                        .multilineTextAlignment(.leading)

                    // Images
                    if let first = post.images.first {
                        AsyncImage(url: first) { image in
                            image.resizable().aspectRatio(contentMode: .fill)
                        } placeholder: {
                            Rectangle().fill(Theme.bgTertiary)
                        }
                        .frame(maxHeight: 160)
                        .clipped()
                        .cornerRadius(8)
                    }

                    // Stats
                    HStack(spacing: 16) {
                        HStack(spacing: 4) {
                            Image(systemName: "heart")
                                .font(.system(size: 11))
                            Text("\(post.likeCount)")
                                .font(.system(size: 12))
                        }
                        HStack(spacing: 4) {
                            Image(systemName: "arrow.2.squarepath")
                                .font(.system(size: 11))
                            Text("\(post.repostCount)")
                                .font(.system(size: 12))
                        }
                    }
                    .foregroundColor(Theme.textMuted)
                }
                .padding(12)
                .background(Theme.bgTertiary)
                .cornerRadius(12)
                .overlay(
                    RoundedRectangle(cornerRadius: 12)
                        .stroke(Theme.border, lineWidth: 1)
                )
            }
            .buttonStyle(.plain)
            .frame(maxWidth: 300)
        } else if loading {
            HStack(spacing: 8) {
                ProgressView().tint(Theme.textMuted)
                Text("Loading Bluesky post...")
                    .font(.system(size: 12))
                    .foregroundColor(Theme.textMuted)
            }
            .padding(12)
            .background(Theme.bgTertiary)
            .cornerRadius(8)
            .task { await fetchPost() }
        }
    }

    private func fetchPost() async {
        let uri = "at://\(handle)/app.bsky.feed.post/\(rkey)"
        guard let url = URL(string: "https://public.api.bsky.app/xrpc/app.bsky.feed.getPostThread?uri=\(uri.addingPercentEncoding(withAllowedCharacters: .urlQueryAllowed) ?? "")&depth=0") else {
            loading = false
            return
        }

        do {
            let (data, _) = try await URLSession.shared.data(from: url)
            let json = try JSONSerialization.jsonObject(with: data) as? [String: Any]
            let thread = json?["thread"] as? [String: Any]
            let p = thread?["post"] as? [String: Any]
            let record = p?["record"] as? [String: Any]
            let author = p?["author"] as? [String: Any]
            let embed = p?["embed"] as? [String: Any]

            let images: [URL] = {
                let imgs = embed?["images"] as? [[String: Any]]
                    ?? (embed?["media"] as? [String: Any])?["images"] as? [[String: Any]]
                    ?? []
                return imgs.compactMap { URL(string: $0["thumb"] as? String ?? "") }
            }()

            post = BskyPost(
                text: record?["text"] as? String ?? "",
                authorHandle: author?["handle"] as? String ?? handle,
                authorName: author?["displayName"] as? String,
                authorAvatar: URL(string: author?["avatar"] as? String ?? ""),
                images: images,
                likeCount: p?["likeCount"] as? Int ?? 0,
                repostCount: p?["repostCount"] as? Int ?? 0
            )
        } catch {
            // Silently fail
        }
        loading = false
    }
}
