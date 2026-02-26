import SwiftUI

/// Root view after login — WhatsApp/Telegram-style bottom tab navigation.
struct MainTabView: View {
    @EnvironmentObject var appState: AppState
    @EnvironmentObject var networkMonitor: NetworkMonitor
    @State private var selectedTab = 0

    var body: some View {
        ZStack {
            TabView(selection: $selectedTab) {
                ChatsTab()
                    .tabItem {
                        Image(systemName: "bubble.left.and.bubble.right.fill")
                        Text("Chats")
                    }
                    .tag(0)
                    .badge(totalUnread)

                DiscoverTab()
                    .tabItem {
                        Image(systemName: "magnifyingglass")
                        Text("Discover")
                    }
                    .tag(1)

                SettingsTab()
                    .tabItem {
                        Image(systemName: "gear")
                        Text("Settings")
                    }
                    .tag(2)
            }
            .tint(Theme.accent)

            // Image lightbox overlay
            if let url = appState.lightboxURL {
                ImageLightbox(url: url)
                    .transition(.opacity)
                    .zIndex(100)
            }
        }
        .animation(.easeInOut(duration: 0.2), value: appState.lightboxURL != nil)
        .onChange(of: appState.pendingDMNick) {
            if appState.pendingDMNick != nil {
                selectedTab = 0 // Switch to Chats tab
            }
        }
        .withToast()
        .preferredColorScheme(.dark)
    }

    private var totalUnread: Int {
        appState.unreadCounts.values.reduce(0, +)
    }
}
