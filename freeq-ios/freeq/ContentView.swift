import SwiftUI

/// Root view. The decision is purely:
///   - `hasSavedSession` → MainTabView (always, with a banner if not connected)
///   - else                → ConnectView
///
/// We deliberately don't gate MainTabView on `connectionState` anymore.
/// Channels and DMs are hydrated from the on-disk cache (`flushBuffersToCache`),
/// so on cold launch the user sees their previous context instantly while the
/// network completes in the background. A small banner at the top shows the
/// connection status; if the connection comes back as `.registered`, the
/// banner dismisses itself.
///
/// The "Sign in with a different account" escape hatch lives in Settings →
/// Logout now, not as a 45-second cliff on the cold-launch spinner.
struct ContentView: View {
    @EnvironmentObject var appState: AppState

    var body: some View {
        Group {
            // Show MainTabView for either a persisted session (cold-launch
            // instant render from cache) or a live registered connection.
            if appState.hasSavedSession || appState.connectionState == .registered {
                MainTabView()
                    .safeAreaInset(edge: .top, spacing: 0) {
                        if appState.connectionState != .registered {
                            ConnectionStatusBanner()
                        }
                    }
                    .onAppear {
                        if appState.connectionState == .disconnected {
                            appState.reconnectSavedSession()
                        }
                    }
                    .transition(.opacity)
            } else {
                ConnectView()
                    .transition(.opacity.combined(with: .scale(scale: 1.02)))
            }
        }
        .animation(.easeInOut(duration: 0.35), value: appState.hasSavedSession)
        .preferredColorScheme(appState.isDarkTheme ? .dark : .light)
    }
}

/// Compact status pill that sits in the top safe-area inset when the
/// connection isn't fully `.registered`. Pulses subtly while connecting so
/// the user sees something is happening, but doesn't block the UI.
struct ConnectionStatusBanner: View {
    @EnvironmentObject var appState: AppState
    @EnvironmentObject var networkMonitor: NetworkMonitor
    @State private var elapsed: Int = 0
    @State private var timer: Timer? = nil

    var body: some View {
        HStack(spacing: 8) {
            statusIcon
            Text(statusText)
                .font(.system(size: 12, weight: .medium))
                .foregroundColor(Theme.textMuted)
                .lineLimit(1)
            Spacer()
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 6)
        .background(Theme.bgSecondary)
        .overlay(
            Rectangle()
                .frame(height: 0.5)
                .foregroundColor(Theme.textMuted.opacity(0.2)),
            alignment: .bottom
        )
        .onAppear { startTimer() }
        .onDisappear { stopTimer() }
        .onChange(of: appState.reconnectAttempt) { _, _ in
            elapsed = 0
        }
    }

    private var statusIcon: some View {
        Group {
            switch appState.connectionState {
            case .disconnected:
                if !networkMonitor.isConnected {
                    Image(systemName: "wifi.slash")
                        .foregroundColor(Theme.textMuted)
                } else {
                    Image(systemName: "arrow.triangle.2.circlepath")
                        .foregroundColor(Theme.accent)
                }
            case .connecting, .connected:
                ProgressView().controlSize(.mini).tint(Theme.accent)
            case .registered:
                EmptyView()
            }
        }
        .frame(width: 14, height: 14)
    }

    private var statusText: String {
        switch appState.connectionState {
        case .disconnected:
            if !networkMonitor.isConnected { return "Offline — messages will sync when you reconnect" }
            return elapsed < 12 ? "Reconnecting…" : "Still trying… network looks slow"
        case .connecting:
            return elapsed < 12 ? "Connecting…" : "Still connecting…"
        case .connected:
            return "Authenticating…"
        case .registered:
            return ""
        }
    }

    private func startTimer() {
        elapsed = 0
        timer?.invalidate()
        timer = Timer.scheduledTimer(withTimeInterval: 1, repeats: true) { _ in
            elapsed += 1
        }
    }

    private func stopTimer() {
        timer?.invalidate()
        timer = nil
    }
}
