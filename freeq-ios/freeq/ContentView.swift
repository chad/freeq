import SwiftUI

struct ContentView: View {
    @EnvironmentObject var appState: AppState
    @State private var hasAttemptedReconnect = false
    @State private var reconnectTimedOut = false
    @State private var reconnectSeconds = 0
    @State private var reconnectTimer: Timer? = nil

    var body: some View {
        Group {
            switch appState.connectionState {
            case .disconnected:
                if appState.hasSavedSession && !hasAttemptedReconnect && !reconnectTimedOut {
                    reconnectingView
                        .onAppear {
                            hasAttemptedReconnect = true
                            startReconnectTimer()
                            appState.reconnectSavedSession()
                        }
                        .transition(.opacity)
                } else {
                    ConnectView()
                        .onAppear { stopReconnectTimer() }
                        .transition(.opacity.combined(with: .scale(scale: 1.02)))
                }
            case .connecting:
                if appState.hasSavedSession || hasAttemptedReconnect {
                    reconnectingView
                        .onAppear { if reconnectTimer == nil { startReconnectTimer() } }
                        .transition(.opacity)
                } else {
                    ConnectView()
                        .transition(.opacity)
                }
            case .connected, .registered:
                MainTabView()
                    .onAppear {
                        hasAttemptedReconnect = false
                        reconnectTimedOut = false
                        stopReconnectTimer()
                        UIImpactFeedbackGenerator(style: .medium).impactOccurred()
                    }
                    .transition(.asymmetric(
                        insertion: .move(edge: .trailing).combined(with: .opacity),
                        removal: .opacity
                    ))
            }
        }
        .animation(.easeInOut(duration: 0.35), value: appState.connectionState)
        .preferredColorScheme(appState.isDarkTheme ? .dark : .light)
    }

    private func startReconnectTimer() {
        reconnectSeconds = 0
        reconnectTimer = Timer.scheduledTimer(withTimeInterval: 1, repeats: true) { _ in
            reconnectSeconds += 1
            if reconnectSeconds >= 10 {
                reconnectTimedOut = true
                stopReconnectTimer()
            }
        }
    }

    private func stopReconnectTimer() {
        reconnectTimer?.invalidate()
        reconnectTimer = nil
        reconnectSeconds = 0
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

                if reconnectSeconds >= 5 {
                    Button(action: {
                        reconnectTimedOut = true
                        stopReconnectTimer()
                        appState.disconnect()
                    }) {
                        Text("Cancel")
                            .font(.system(size: 14, weight: .medium))
                            .foregroundColor(Theme.accent)
                    }
                    .transition(.opacity)
                }
            }
            .animation(.easeInOut, value: reconnectSeconds >= 5)
        }
    }
}
