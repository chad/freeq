import SwiftUI

/// Delegate to handle notification taps and navigate to the right channel.
class NotificationDelegate: NSObject, UNUserNotificationCenterDelegate {
    weak var appState: AppState?

    func userNotificationCenter(_ center: UNUserNotificationCenter,
                                didReceive response: UNNotificationResponse,
                                withCompletionHandler completionHandler: @escaping () -> Void) {
        if let channel = response.notification.request.content.userInfo["channel"] as? String {
            DispatchQueue.main.async { [weak self] in
                guard let state = self?.appState else { return }
                if channel.hasPrefix("#") {
                    state.activeChannel = channel
                } else {
                    state.pendingDMNick = channel
                }
            }
        }
        completionHandler()
    }

    // Show notifications even when app is in foreground (for non-active channels)
    func userNotificationCenter(_ center: UNUserNotificationCenter,
                                willPresent notification: UNNotification,
                                withCompletionHandler completionHandler: @escaping (UNNotificationPresentationOptions) -> Void) {
        let channel = notification.request.content.userInfo["channel"] as? String
        if channel != appState?.activeChannel {
            completionHandler([.banner, .sound])
        } else {
            completionHandler([])
        }
    }
}

@main
struct FreeqApp: App {
    @StateObject private var appState = AppState()
    @StateObject private var networkMonitor = NetworkMonitor()
    @Environment(\.scenePhase) private var scenePhase
    private let notificationDelegate = NotificationDelegate()

    var body: some Scene {
        WindowGroup {
            ContentView()
                .environmentObject(appState)
                .environmentObject(networkMonitor)
                .onAppear {
                    networkMonitor.bind(to: appState)
                    notificationDelegate.appState = appState
                    UNUserNotificationCenter.current().delegate = notificationDelegate
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
        KeychainHelper.save(key: "brokerToken", value: brokerToken)
        UserDefaults.standard.removeObject(forKey: "freeq.loginPending")

        // Connect
        appState.pendingWebToken = token
        appState.brokerToken = brokerToken
        appState.authenticatedDID = did
        appState.serverAddress = "irc.freeq.at:6667"
        appState.connect(nick: nick)
    }
}
