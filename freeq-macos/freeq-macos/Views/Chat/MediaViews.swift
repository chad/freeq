import SwiftUI

// MARK: - URL pattern detection

private let imageExtensions = Set(["jpg", "jpeg", "png", "gif", "webp"])
private let cdnImagePattern = try! NSRegularExpression(pattern: "https?://cdn\\.bsky\\.app/img/[^\\s<]+", options: .caseInsensitive)
private let youtubePattern = try! NSRegularExpression(pattern: "(?:youtube\\.com/watch\\?v=|youtu\\.be/)([a-zA-Z0-9_-]{11})", options: .caseInsensitive)
private let bskyPostPattern = try! NSRegularExpression(pattern: "https?://bsky\\.app/profile/([^/]+)/post/([a-zA-Z0-9]+)", options: .caseInsensitive)

/// Extract image URLs from message text.
func extractImageURLs(from text: String) -> [String] {
    var urls: [String] = []

    // Standard image URLs (.jpg, .png, etc.)
    let detector = try? NSDataDetector(types: NSTextCheckingResult.CheckingType.link.rawValue)
    if let matches = detector?.matches(in: text, range: NSRange(text.startIndex..., in: text)) {
        for match in matches {
            guard let range = Range(match.range, in: text), let url = match.url else { continue }
            let path = url.pathExtension.lowercased()
            if imageExtensions.contains(path) {
                urls.append(String(text[range]))
            }
        }
    }

    // CDN image URLs (no extension)
    let cdnMatches = cdnImagePattern.matches(in: text, range: NSRange(text.startIndex..., in: text))
    for match in cdnMatches {
        if let range = Range(match.range, in: text) {
            let url = String(text[range])
            if !urls.contains(url) { urls.append(url) }
        }
    }

    return urls
}

/// Extract YouTube video ID from text.
func extractYouTubeID(from text: String) -> String? {
    let match = youtubePattern.firstMatch(in: text, range: NSRange(text.startIndex..., in: text))
    guard let match, let range = Range(match.range(at: 1), in: text) else { return nil }
    return String(text[range])
}

/// Extract Bluesky post (handle, rkey) from text.
func extractBskyPost(from text: String) -> (handle: String, rkey: String)? {
    let match = bskyPostPattern.firstMatch(in: text, range: NSRange(text.startIndex..., in: text))
    guard let match,
          let handleRange = Range(match.range(at: 1), in: text),
          let rkeyRange = Range(match.range(at: 2), in: text) else { return nil }
    return (String(text[handleRange]), String(text[rkeyRange]))
}

/// Remove image URLs from text for cleaner display.
func textWithoutImages(_ text: String, imageURLs: [String]) -> String {
    var result = text
    for url in imageURLs {
        result = result.replacingOccurrences(of: url, with: "").trimmingCharacters(in: .whitespaces)
    }
    return result
}

/// Check if text has any media (images, YouTube, Bluesky) that we should show separately.
func hasMedia(in text: String) -> Bool {
    !extractImageURLs(from: text).isEmpty || extractYouTubeID(from: text) != nil
}

// MARK: - Inline Image View

struct InlineImageView: View {
    let url: String
    @State private var showLightbox = false

    var body: some View {
        AsyncImage(url: URL(string: url)) { phase in
            switch phase {
            case .success(let image):
                image
                    .resizable()
                    .aspectRatio(contentMode: .fit)
                    .frame(maxWidth: 400, maxHeight: 300)
                    .clipShape(RoundedRectangle(cornerRadius: 8))
                    .overlay(
                        RoundedRectangle(cornerRadius: 8)
                            .strokeBorder(Color(nsColor: .separatorColor), lineWidth: 0.5)
                    )
                    .onTapGesture { showLightbox = true }
                    .popover(isPresented: $showLightbox) {
                        ImageLightbox(url: url)
                    }
            case .failure:
                HStack(spacing: 4) {
                    Image(systemName: "photo.badge.exclamationmark")
                        .font(.caption)
                    Text("Failed to load image")
                        .font(.caption)
                }
                .foregroundStyle(.secondary)
                .padding(8)
                .background(RoundedRectangle(cornerRadius: 6).fill(Color(nsColor: .controlBackgroundColor)))
            case .empty:
                RoundedRectangle(cornerRadius: 8)
                    .fill(Color(nsColor: .controlBackgroundColor))
                    .frame(width: 200, height: 100)
                    .overlay(ProgressView().scaleEffect(0.7))
            @unknown default:
                EmptyView()
            }
        }
        .padding(.top, 4)
    }
}

// MARK: - Image Lightbox

struct ImageLightbox: View {
    let url: String

    var body: some View {
        VStack(spacing: 0) {
            AsyncImage(url: URL(string: url)) { phase in
                switch phase {
                case .success(let image):
                    image
                        .resizable()
                        .aspectRatio(contentMode: .fit)
                case .failure:
                    Text("Failed to load")
                        .foregroundStyle(.secondary)
                default:
                    ProgressView()
                }
            }
            .frame(minWidth: 400, maxWidth: 800, minHeight: 300, maxHeight: 600)

            HStack {
                Button("Copy URL") {
                    NSPasteboard.general.clearContents()
                    NSPasteboard.general.setString(url, forType: .string)
                }
                Button("Open in Browser") {
                    if let u = URL(string: url) { NSWorkspace.shared.open(u) }
                }
                Spacer()
                Button("Save…") {
                    saveImage()
                }
            }
            .padding(12)
            .background(.bar)
        }
    }

    private func saveImage() {
        Task {
            guard let imgURL = URL(string: url) else { return }
            let (data, _) = try await URLSession.shared.data(from: imgURL)
            let panel = NSSavePanel()
            panel.nameFieldStringValue = imgURL.lastPathComponent
            panel.allowedContentTypes = [.png, .jpeg, .gif]
            if panel.runModal() == .OK, let saveURL = panel.url {
                try data.write(to: saveURL)
            }
        }
    }
}

// MARK: - Bluesky Post Embed

struct BlueskyEmbed: View {
    let handle: String
    let rkey: String
    @State private var post: BskyPost?
    @State private var loaded = false

    struct BskyPost {
        let authorName: String
        let authorHandle: String
        let authorAvatar: String?
        let text: String
        let createdAt: String
    }

    var body: some View {
        Group {
            if let post {
                Link(destination: URL(string: "https://bsky.app/profile/\(handle)/post/\(rkey)")!) {
                    VStack(alignment: .leading, spacing: 6) {
                        HStack(spacing: 6) {
                            if let avatar = post.authorAvatar, let url = URL(string: avatar) {
                                AsyncImage(url: url) { phase in
                                    if case .success(let img) = phase {
                                        img.resizable().aspectRatio(contentMode: .fill)
                                            .frame(width: 20, height: 20).clipShape(Circle())
                                    }
                                }
                            }
                            Text(post.authorName)
                                .font(.caption.weight(.semibold))
                                .foregroundStyle(.primary)
                            Text("@\(post.authorHandle)")
                                .font(.caption2)
                                .foregroundStyle(.secondary)
                            Spacer()
                            Image(systemName: "cloud.fill")
                                .font(.caption2)
                                .foregroundStyle(.blue)
                        }
                        Text(post.text)
                            .font(.caption)
                            .foregroundStyle(.primary)
                            .lineLimit(4)
                    }
                    .padding(10)
                    .frame(maxWidth: 380, alignment: .leading)
                    .background(Color(nsColor: .controlBackgroundColor))
                    .clipShape(RoundedRectangle(cornerRadius: 8))
                    .overlay(
                        RoundedRectangle(cornerRadius: 8)
                            .strokeBorder(Color.blue.opacity(0.2), lineWidth: 1)
                    )
                }
                .buttonStyle(.plain)
                .padding(.top, 4)
            }
        }
        .onAppear { fetchPost() }
    }

    private func fetchPost() {
        guard !loaded else { return }
        loaded = true
        Task {
            let url = "https://public.api.bsky.app/xrpc/app.bsky.feed.getPostThread?uri=at://\(handle)/app.bsky.feed.post/\(rkey)&depth=0"
            guard let requestURL = URL(string: url) else { return }
            do {
                let (data, _) = try await URLSession.shared.data(from: requestURL)
                let json = try JSONSerialization.jsonObject(with: data) as? [String: Any]
                let thread = json?["thread"] as? [String: Any]
                let postObj = thread?["post"] as? [String: Any]
                let author = postObj?["author"] as? [String: Any]
                let record = postObj?["record"] as? [String: Any]
                guard let text = record?["text"] as? String else { return }
                let p = BskyPost(
                    authorName: author?["displayName"] as? String ?? handle,
                    authorHandle: author?["handle"] as? String ?? handle,
                    authorAvatar: author?["avatar"] as? String,
                    text: text,
                    createdAt: record?["createdAt"] as? String ?? ""
                )
                await MainActor.run { self.post = p }
            } catch {}
        }
    }
}

// MARK: - YouTube Thumbnail

struct YouTubeThumbnail: View {
    let videoId: String

    var body: some View {
        Link(destination: URL(string: "https://youtube.com/watch?v=\(videoId)")!) {
            VStack(spacing: 0) {
                AsyncImage(url: URL(string: "https://img.youtube.com/vi/\(videoId)/mqdefault.jpg")) { phase in
                    if case .success(let image) = phase {
                        image
                            .resizable()
                            .aspectRatio(contentMode: .fill)
                            .frame(maxWidth: 320, maxHeight: 180)
                            .clipped()
                            .overlay {
                                // Play button overlay
                                Image(systemName: "play.circle.fill")
                                    .font(.system(size: 44))
                                    .foregroundStyle(.white)
                                    .shadow(radius: 4)
                            }
                    } else {
                        RoundedRectangle(cornerRadius: 0)
                            .fill(Color(nsColor: .controlBackgroundColor))
                            .frame(width: 320, height: 180)
                            .overlay(ProgressView().scaleEffect(0.7))
                    }
                }
                HStack(spacing: 4) {
                    Text("▶")
                        .foregroundStyle(.red)
                        .font(.caption)
                    Text("YouTube")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                    Spacer()
                }
                .padding(.horizontal, 10)
                .padding(.vertical, 6)
                .background(Color(nsColor: .controlBackgroundColor))
            }
            .frame(maxWidth: 320)
            .clipShape(RoundedRectangle(cornerRadius: 8))
            .overlay(
                RoundedRectangle(cornerRadius: 8)
                    .strokeBorder(Color(nsColor: .separatorColor), lineWidth: 0.5)
            )
        }
        .buttonStyle(.plain)
        .padding(.top, 4)
    }
}
