import SwiftUI

/// Unfurls URLs with OpenGraph metadata.
struct LinkPreviewView: View {
    let url: String
    @State private var ogData: OGData?
    @State private var loaded = false

    private static var cache: [String: OGData?] = [:]

    struct OGData {
        let title: String?
        let description: String?
        let image: String?
        let siteName: String?
    }

    var body: some View {
        Group {
            if let data = ogData, (data.title != nil || data.image != nil) {
                Link(destination: URL(string: url)!) {
                    VStack(alignment: .leading, spacing: 0) {
                        // Image
                        if let imageURL = data.image, let imgURL = URL(string: imageURL) {
                            AsyncImage(url: imgURL) { phase in
                                switch phase {
                                case .success(let image):
                                    image
                                        .resizable()
                                        .aspectRatio(contentMode: .fill)
                                        .frame(maxWidth: 380, maxHeight: 160)
                                        .clipped()
                                default:
                                    EmptyView()
                                }
                            }
                        }

                        VStack(alignment: .leading, spacing: 2) {
                            if let siteName = data.siteName {
                                Text(siteName)
                                    .font(.system(size: 9))
                                    .foregroundStyle(.tertiary)
                                    .textCase(.uppercase)
                            }
                            if let title = data.title {
                                Text(title)
                                    .font(.caption.weight(.semibold))
                                    .foregroundStyle(.blue)
                                    .lineLimit(2)
                            }
                            if let desc = data.description {
                                Text(desc)
                                    .font(.caption2)
                                    .foregroundStyle(.secondary)
                                    .lineLimit(2)
                            }
                            Text(domain)
                                .font(.system(size: 9))
                                .foregroundStyle(.tertiary)
                        }
                        .padding(8)
                    }
                    .frame(maxWidth: 380, alignment: .leading)
                    .background(Color(nsColor: .controlBackgroundColor))
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
        .onAppear { fetchOG() }
    }

    private var domain: String {
        URL(string: url)?.host?.replacingOccurrences(of: "www.", with: "") ?? ""
    }

    private func fetchOG() {
        if let cached = Self.cache[url] {
            ogData = cached
            loaded = true
            return
        }
        guard !loaded else { return }
        loaded = true

        Task {
            let proxyURL = "https://irc.freeq.at/api/v1/og?url=\(url.addingPercentEncoding(withAllowedCharacters: .urlQueryAllowed) ?? url)"
            guard let requestURL = URL(string: proxyURL) else { return }

            do {
                let (data, response) = try await URLSession.shared.data(from: requestURL)
                guard let httpResponse = response as? HTTPURLResponse,
                      httpResponse.statusCode == 200 else {
                    Self.cache[url] = nil
                    return
                }

                let json = try JSONSerialization.jsonObject(with: data) as? [String: Any] ?? [:]
                let og = OGData(
                    title: json["title"] as? String,
                    description: json["description"] as? String,
                    image: json["image"] as? String,
                    siteName: json["site_name"] as? String
                )

                if og.title != nil || og.image != nil {
                    Self.cache[url] = og
                    await MainActor.run { ogData = og }
                } else {
                    Self.cache[url] = nil
                }
            } catch {
                Self.cache[url] = nil
            }
        }
    }
}
