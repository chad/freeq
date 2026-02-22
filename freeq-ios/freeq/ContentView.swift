import SwiftUI

struct ContentView: View {
    @EnvironmentObject var appState: AppState

    var body: some View {
        ZStack {
            Group {
                switch appState.connectionState {
                case .disconnected, .connecting:
                    ConnectView()
                case .connected, .registered:
                    ChatView()
                }
            }

            // Image lightbox overlay
            if let url = appState.lightboxURL {
                ImageLightbox(url: url)
                    .transition(.opacity)
                    .zIndex(100)
            }
        }
        .animation(.easeInOut(duration: 0.2), value: appState.lightboxURL != nil)
        .preferredColorScheme(.dark)
    }
}
