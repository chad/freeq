import SwiftUI

struct ContentView: View {
    @EnvironmentObject var appState: AppState
    @State private var reconnectSeconds = 0
    @State private var reconnectTimer: Timer? = nil
    @State private var userCancelledReconnect = false

    var body: some View {
        Group {
            switch appState.connectionState {
            case .disconnected:
                if appState.hasSavedSession && !userCancelledReconnect {
                    // Auto-reconnect: keep trying until user explicitly cancels
                    reconnectingView
                        .onAppear {
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
                reconnectingView
                    .onAppear { if reconnectTimer == nil { startReconnectTimer() } }
                    .transition(.opacity)
            case .connected, .registered:
                MainTabView()
                    .onAppear {
                        userCancelledReconnect = false
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
        reconnectTimer?.invalidate()
        reconnectTimer = Timer.scheduledTimer(withTimeInterval: 1, repeats: true) { _ in
            reconnectSeconds += 1
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
                Text(reconnectSeconds < 5 ? "Connecting..." : "Still connecting...")
                    .font(.system(size: 15, weight: .medium))
                    .foregroundColor(Theme.textMuted)

                if reconnectSeconds >= 8 {
                    Button(action: {
                        userCancelledReconnect = true
                        stopReconnectTimer()
                        appState.disconnect()
                    }) {
                        Text("Sign in manually")
                            .font(.system(size: 14, weight: .medium))
                            .foregroundColor(Theme.accent)
                    }
                    .transition(.opacity)
                }
            }
            .animation(.easeInOut, value: reconnectSeconds >= 8)
        }
    }
}
