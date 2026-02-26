import SwiftUI

struct ImageLightbox: View {
    @EnvironmentObject var appState: AppState
    let url: URL

    @State private var scale: CGFloat = 1.0
    @State private var lastScale: CGFloat = 1.0
    @State private var offset: CGSize = .zero
    @State private var dragOffset: CGSize = .zero
    @State private var backgroundOpacity: Double = 1.0

    private func dismiss() {
        withAnimation(.easeOut(duration: 0.2)) {
            appState.lightboxURL = nil
        }
    }

    var body: some View {
        ZStack {
            Color.black.opacity(0.9 * backgroundOpacity)
                .ignoresSafeArea()
                .onTapGesture {
                    if scale > 1.0 {
                        withAnimation(.spring(response: 0.3)) {
                            scale = 1.0
                            lastScale = 1.0
                            offset = .zero
                        }
                    } else {
                        dismiss()
                    }
                }

            AsyncImage(url: url) { phase in
                switch phase {
                case .success(let image):
                    image
                        .resizable()
                        .aspectRatio(contentMode: .fit)
                        .scaleEffect(scale)
                        .offset(x: offset.width + dragOffset.width,
                                y: offset.height + dragOffset.height)
                        // Double-tap to zoom
                        .onTapGesture(count: 2) {
                            withAnimation(.spring(response: 0.3)) {
                                if scale > 1.5 {
                                    scale = 1.0
                                    lastScale = 1.0
                                    offset = .zero
                                } else {
                                    scale = 3.0
                                    lastScale = 3.0
                                }
                            }
                        }
                        // Pinch to zoom
                        .gesture(
                            MagnifyGesture()
                                .onChanged { value in
                                    let newScale = lastScale * value.magnification
                                    scale = min(max(newScale, 0.5), 6.0)
                                }
                                .onEnded { value in
                                    lastScale = scale
                                    if scale < 1.0 {
                                        withAnimation(.spring(response: 0.3)) {
                                            scale = 1.0
                                            lastScale = 1.0
                                            offset = .zero
                                        }
                                    }
                                }
                        )
                        // Pan when zoomed, dismiss when not
                        .gesture(
                            DragGesture()
                                .onChanged { value in
                                    if scale > 1.1 {
                                        // Pan within zoomed image
                                        dragOffset = value.translation
                                    } else {
                                        // Drag to dismiss
                                        dragOffset = value.translation
                                        let progress = abs(value.translation.height) / 300
                                        backgroundOpacity = max(0.3, 1.0 - progress)
                                    }
                                }
                                .onEnded { value in
                                    if scale > 1.1 {
                                        offset.width += dragOffset.width
                                        offset.height += dragOffset.height
                                        dragOffset = .zero
                                    } else {
                                        let velocity = abs(value.predictedEndTranslation.height)
                                        if abs(value.translation.height) > 80 || velocity > 500 {
                                            dismiss()
                                        } else {
                                            withAnimation(.spring(response: 0.3)) {
                                                dragOffset = .zero
                                                backgroundOpacity = 1.0
                                            }
                                        }
                                    }
                                }
                        )
                default:
                    ProgressView()
                        .tint(.white)
                }
            }
            .padding(20)

            // Top bar: close + share
            VStack {
                HStack {
                    Button(action: dismiss) {
                        Image(systemName: "xmark.circle.fill")
                            .font(.system(size: 30))
                            .symbolRenderingMode(.palette)
                            .foregroundStyle(.white, .white.opacity(0.3))
                    }
                    .padding(16)

                    Spacer()

                    ShareLink(item: url) {
                        Image(systemName: "square.and.arrow.up.circle.fill")
                            .font(.system(size: 30))
                            .symbolRenderingMode(.palette)
                            .foregroundStyle(.white, .white.opacity(0.3))
                    }
                    .padding(16)
                }
                Spacer()
            }
            .opacity(scale > 1.5 ? 0 : 1) // Hide controls when zoomed in
            .animation(.easeOut(duration: 0.15), value: scale > 1.5)
        }
    }
}
