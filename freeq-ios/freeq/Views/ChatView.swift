import SwiftUI

struct ChatView: View {
    @EnvironmentObject var appState: AppState
    @State private var showingSidebar = false
    @State private var showingJoinSheet = false
    @State private var showingMembers = false

    var body: some View {
        ZStack {
            Theme.bgPrimary.ignoresSafeArea()

            // Main chat area
            VStack(spacing: 0) {
                TopBarView(
                    showingSidebar: $showingSidebar,
                    showingJoinSheet: $showingJoinSheet,
                    showingMembers: $showingMembers
                )

                ZStack {
                    if let channel = appState.activeChannelState {
                        MessageListView(channel: channel)
                    } else {
                        emptyState
                    }

                    // Member list slide-in
                    if showingMembers, let channel = appState.activeChannelState {
                        HStack(spacing: 0) {
                            Spacer()
                            MemberListView(channel: channel)
                                .frame(width: 260)
                                .transition(.move(edge: .trailing))
                        }
                    }
                }

                if appState.activeChannel != nil {
                    ComposeView()
                }
            }

            // Sidebar overlay
            if showingSidebar {
                Color.black.opacity(0.5)
                    .ignoresSafeArea()
                    .onTapGesture { showingSidebar = false }

                HStack(spacing: 0) {
                    SidebarView(showingSidebar: $showingSidebar)
                        .frame(width: 290)
                        .transition(.move(edge: .leading))
                    Spacer()
                }
            }
        }
        .animation(.easeInOut(duration: 0.2), value: showingSidebar)
        .animation(.easeInOut(duration: 0.2), value: showingMembers)
        .sheet(isPresented: $showingJoinSheet) {
            JoinChannelSheet()
                .presentationDetents([.medium])
                .presentationDragIndicator(.visible)
        }
        .preferredColorScheme(.dark)
    }

    private var emptyState: some View {
        VStack(spacing: 16) {
            Image(systemName: "bubble.left.and.bubble.right")
                .font(.system(size: 48))
                .foregroundColor(Theme.textMuted)
            Text("No channel selected")
                .font(.system(size: 18, weight: .medium))
                .foregroundColor(Theme.textSecondary)
            Button("Join a channel") {
                showingJoinSheet = true
            }
            .font(.system(size: 15, weight: .medium))
            .foregroundColor(Theme.accent)
        }
    }
}
