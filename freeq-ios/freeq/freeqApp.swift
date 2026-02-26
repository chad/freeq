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
                    // NOTE: notification permission is deferred until first mention
                    // (see NotificationManager.requestPermissionIfNeeded)
                }
        }
        .onChange(of: scenePhase) { _, newPhase in
            appState.handleScenePhase(newPhase)
        }
    }
}
