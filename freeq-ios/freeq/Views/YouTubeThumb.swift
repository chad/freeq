import SwiftUI

/// Renders a YouTube video thumbnail with play button overlay.
struct YouTubeThumb: View {
    let videoId: String

    var thumbnailURL: URL {
        URL(string: "https://img.youtube.com/vi/\(videoId)/mqdefault.jpg")!
    }

    var videoURL: URL {
        URL(string: "https://youtube.com/watch?v=\(videoId)")!
    }

    var body: some View {
        Link(destination: videoURL) {
            ZStack {
                AsyncImage(url: thumbnailURL) { image in
                    image
                        .resizable()
                        .aspectRatio(16/9, contentMode: .fill)
                } placeholder: {
                    Rectangle()
                        .fill(Theme.bgTertiary)
                        .aspectRatio(16/9, contentMode: .fill)
                }
                .frame(maxWidth: 280)
                .clipped()

                // Play button overlay
                Circle()
                    .fill(Color.red)
                    .frame(width: 48, height: 48)
                    .overlay(
                        Image(systemName: "play.fill")
                            .font(.system(size: 20))
                            .foregroundColor(.white)
                            .offset(x: 2) // Visual centering
                    )
                    .shadow(radius: 4)
            }
            .cornerRadius(10)
            .overlay(
                RoundedRectangle(cornerRadius: 10)
                    .stroke(Theme.border, lineWidth: 1)
            )
        }
        .buttonStyle(.plain)
        .frame(maxWidth: 280)
    }
}
