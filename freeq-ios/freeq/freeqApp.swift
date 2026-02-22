import SwiftUI

@main
struct FreeqApp: App {
    @StateObject private var appState = AppState()
    @StateObject private var networkMonitor = NetworkMonitor()

    var body: some Scene {
        WindowGroup {
            ContentView()
                .environmentObject(appState)
                .environmentObject(networkMonitor)
                .onAppear {
                    networkMonitor.bind(to: appState)
                }
        }
    }
}
