import SwiftUI

struct ImageLightbox: View {
    @EnvironmentObject var appState: AppState
    let url: URL

    @State private var scale: CGFloat = 1.0
    @State private var offset: CGSize = .zero

    var body: some View {
        ZStack {
            Color.black.opacity(0.9)
                .ignoresSafeArea()
                .onTapGesture {
                    withAnimation(.easeOut(duration: 0.2)) {
                        appState.lightboxURL = nil
                    }
                }

            AsyncImage(url: url) { phase in
                switch phase {
                case .success(let image):
                    image
                        .resizable()
                        .aspectRatio(contentMode: .fit)
                        .scaleEffect(scale)
                        .offset(offset)
                        .gesture(
                            MagnifyGesture()
                                .onChanged { value in
                                    scale = value.magnification
                                }
                                .onEnded { _ in
                                    withAnimation { scale = 1.0 }
                                }
                        )
                        .gesture(
                            DragGesture()
                                .onChanged { value in
                                    offset = value.translation
                                }
                                .onEnded { value in
                                    if abs(value.translation.height) > 100 {
                                        appState.lightboxURL = nil
                                    } else {
                                        withAnimation { offset = .zero }
                                    }
                                }
                        )
                default:
                    ProgressView()
                        .tint(.white)
                }
            }
            .padding(20)

            // Close button
            VStack {
                HStack {
                    Spacer()
                    Button(action: { appState.lightboxURL = nil }) {
                        Image(systemName: "xmark.circle.fill")
                            .font(.system(size: 30))
                            .foregroundColor(.white.opacity(0.7))
                    }
                    .padding(16)
                }
                Spacer()
            }

            // Share button
            VStack {
                Spacer()
                HStack {
                    Spacer()
                    ShareLink(item: url) {
                        Image(systemName: "square.and.arrow.up")
                            .font(.system(size: 20))
                            .foregroundColor(.white.opacity(0.7))
                            .frame(width: 44, height: 44)
                            .background(Color.white.opacity(0.1))
                            .cornerRadius(22)
                    }
                    .padding(16)
                }
            }
        }
    }
}
