import SwiftUI

struct ChatView: View {
    @EnvironmentObject var appState: AppState
    @State private var showingSidebar = false
    @State private var showingJoinSheet = false

    var body: some View {
        ZStack {
            // Main chat area
            VStack(spacing: 0) {
                // Top bar
                TopBarView(
                    showingSidebar: $showingSidebar,
                    showingJoinSheet: $showingJoinSheet
                )

                // Messages
                if let channel = appState.activeChannelState {
                    MessageListView(channel: channel)
                } else {
                    Spacer()
                    Text("Join a channel to start chatting")
                        .foregroundColor(.secondary)
                    Spacer()
                }

                // Compose
                if appState.activeChannel != nil {
                    ComposeView()
                }
            }

            // Sidebar overlay
            if showingSidebar {
                Color.black.opacity(0.3)
                    .ignoresSafeArea()
                    .onTapGesture { showingSidebar = false }

                HStack {
                    SidebarView(showingSidebar: $showingSidebar)
                        .frame(width: 280)
                        .transition(.move(edge: .leading))
                    Spacer()
                }
            }
        }
        .animation(.easeInOut(duration: 0.25), value: showingSidebar)
        .sheet(isPresented: $showingJoinSheet) {
            JoinChannelSheet()
        }
    }
}
