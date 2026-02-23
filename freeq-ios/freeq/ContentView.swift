import SwiftUI

struct ContentView: View {
    @EnvironmentObject var appState: AppState
    @State private var hasAttemptedReconnect = false

    var body: some View {
        Group {
            switch appState.connectionState {
            case .disconnected:
                if appState.hasSavedSession && !hasAttemptedReconnect {
                    // Show a brief loading state while reconnecting
                    reconnectingView
                        .onAppear {
                            hasAttemptedReconnect = true
                            appState.reconnectSavedSession()
                        }
                } else {
                    ConnectView()
                }
            case .connecting:
                if appState.hasSavedSession {
                    reconnectingView
                } else {
                    ConnectView()
                }
            case .connected, .registered:
                MainTabView()
                    .onAppear { hasAttemptedReconnect = false }
            }
        }
        .preferredColorScheme(appState.isDarkTheme ? .dark : .light)
    }

    private var reconnectingView: some View {
        ZStack {
            Theme.bgPrimary.ignoresSafeArea()
            VStack(spacing: 16) {
                ProgressView()
                    .tint(Theme.accent)
                    .scaleEffect(1.2)
                Text("Connecting...")
                    .font(.system(size: 15, weight: .medium))
                    .foregroundColor(Theme.textMuted)
            }
        }
    }
}
