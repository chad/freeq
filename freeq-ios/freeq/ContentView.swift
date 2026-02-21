import SwiftUI

struct ContentView: View {
    @EnvironmentObject var appState: AppState

    var body: some View {
        switch appState.connectionState {
        case .disconnected, .connecting:
            ConnectView()
        case .connected, .registered:
            ChatView()
        }
    }
}
