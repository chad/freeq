import SwiftUI

@main
struct FreeqApp: App {
    @StateObject private var appState = AppState()
    @StateObject private var networkMonitor = NetworkMonitor()
    @Environment(\.scenePhase) private var scenePhase

    var body: some Scene {
        WindowGroup {
            ContentView()
                .environmentObject(appState)
                .environmentObject(networkMonitor)
                .onAppear {
                    networkMonitor.bind(to: appState)
                }
                .onOpenURL { url in
                    handleAuthCallback(url)
                }
        }
        .onChange(of: scenePhase) { _, newPhase in
            appState.handleScenePhase(newPhase)
        }
    }

    /// Handle freeq://auth?token=...&broker_token=...&nick=...&did=...&handle=...
    private func handleAuthCallback(_ url: URL) {
        guard url.scheme == "freeq", url.host == "auth" else { return }
        guard let components = URLComponents(url: url, resolvingAgainstBaseURL: false) else { return }

        // Check for error
        if let error = components.queryItems?.first(where: { $0.name == "error" })?.value {
            appState.errorMessage = error
            appState.connectionState = .disconnected
            return
        }

        guard let token = components.queryItems?.first(where: { $0.name == "token" })?.value,
              let brokerToken = components.queryItems?.first(where: { $0.name == "broker_token" })?.value,
              let nick = components.queryItems?.first(where: { $0.name == "nick" })?.value,
              let did = components.queryItems?.first(where: { $0.name == "did" })?.value
        else {
            appState.errorMessage = "Invalid auth response"
            return
        }

        let handle = components.queryItems?.first(where: { $0.name == "handle" })?.value ?? nick

        // Save session
        UserDefaults.standard.set(handle, forKey: "freeq.handle")
        UserDefaults.standard.set(Date().timeIntervalSince1970, forKey: "freeq.lastLogin")
        UserDefaults.standard.set(brokerToken, forKey: "freeq.brokerToken")
        UserDefaults.standard.removeObject(forKey: "freeq.loginPending")

        // Connect
        appState.pendingWebToken = token
        appState.brokerToken = brokerToken
        appState.authenticatedDID = did
        appState.serverAddress = "irc.freeq.at:6667"
        appState.connect(nick: nick)
    }
}
