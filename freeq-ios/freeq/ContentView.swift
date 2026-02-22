import SwiftUI

struct ContentView: View {
    @EnvironmentObject var appState: AppState

    var body: some View {
        Group {
            switch appState.connectionState {
            case .disconnected, .connecting:
                ConnectView()
            case .connected, .registered:
                MainTabView()
            }
        }
        .preferredColorScheme(.dark)
    }
}
