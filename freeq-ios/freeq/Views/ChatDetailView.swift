import SwiftUI

/// Full-screen chat view — pushed from the chat list.
struct ChatDetailView: View {
    @EnvironmentObject var appState: AppState
    let channelName: String
    @State private var showingMembers = false
    @State private var showingSearch = false
    @Environment(\.dismiss) var dismiss

    private var channelState: ChannelState? {
        appState.channels.first { $0.name == channelName }
            ?? appState.dmBuffers.first { $0.name == channelName }
    }

    private var isChannel: Bool { channelName.hasPrefix("#") }

    var body: some View {
        ZStack {
            Theme.bgPrimary.ignoresSafeArea()

            VStack(spacing: 0) {
                // Connection status bar — always offer a Sign out escape
                // hatch when not registered, so a stuck saved session (bad
                // broker token, revoked refresh, etc.) doesn't trap the user.
                if appState.connectionState != .registered {
                    HStack(spacing: 8) {
                        if appState.connectionState == .connecting || appState.connectionState == .connected {
                            ProgressView()
                                .progressViewStyle(CircularProgressViewStyle(tint: .white))
                                .scaleEffect(0.7)
                        } else {
                            Image(systemName: "wifi.slash")
                                .font(.system(size: 12))
                        }
                        Text(appState.connectionState == .disconnected ? "Disconnected — pull down to reconnect" :
                             appState.connectionState == .connecting ? "Connecting..." : "Registering...")
                            .font(.system(size: 13, weight: .medium))
                        Spacer()
                        Button("Sign out") { appState.logout() }
                            .font(.system(size: 13, weight: .semibold))
                            .foregroundColor(.white)
                            .padding(.horizontal, 8)
                            .padding(.vertical, 2)
                            .background(Color.white.opacity(0.18))
                            .clipShape(Capsule())
                    }
                    .foregroundColor(.white)
                    .frame(maxWidth: .infinity)
                    .padding(.horizontal, 12)
                    .padding(.vertical, 6)
                    .background(appState.connectionState == .disconnected ? Theme.danger : Theme.warning)
                    .transition(.move(edge: .top).combined(with: .opacity))
                    .animation(.easeInOut(duration: 0.3), value: appState.connectionState)
                }

                // Voice/video call panel — pinned above the message list
                // when an AV session is active in this channel.
                if appState.isInCall && isChannel,
                   appState.currentCallChannel?.lowercased() == channelName.lowercased() {
                    CallView(channel: channelName)
                }

                if let channel = channelState {
                    ZStack {
                        MessageListView(channel: channel)
                            .onTapGesture {
                                UIApplication.shared.sendAction(#selector(UIResponder.resignFirstResponder), to: nil, from: nil, for: nil)
                            }

                        // Member list slide-in
                        if showingMembers {
                            HStack(spacing: 0) {
                                Spacer()
                                Color.black.opacity(0.3)
                                    .ignoresSafeArea()
                                    .onTapGesture { showingMembers = false }
                                MemberListView(channel: channel)
                                    .frame(width: 260)
                                    .transition(.move(edge: .trailing))
                            }
                            .animation(.easeInOut(duration: 0.2), value: showingMembers)
                        }
                    }

                    ComposeView()
                } else {
                    Spacer()
                    Text("Channel not found")
                        .foregroundColor(Theme.textMuted)
                    Spacer()
                }
            }
        }
        .navigationBarTitleDisplayMode(.inline)
        .toolbarBackground(Theme.bgSecondary, for: .navigationBar)
        .toolbarBackground(.visible, for: .navigationBar)
        .toolbar {
            ToolbarItem(placement: .principal) {
                VStack(spacing: 1) {
                    Text(channelName)
                        .font(.system(size: 16, weight: .semibold))
                        .foregroundColor(Theme.textPrimary)

                    if let channel = channelState {
                        if !channel.activeTypers.isEmpty {
                            Text(typingText(channel.activeTypers))
                                .font(.system(size: 11))
                                .foregroundColor(Theme.accent)
                        } else if !channel.topic.isEmpty {
                            Text(channel.topic)
                                .font(.system(size: 11))
                                .foregroundColor(Theme.textMuted)
                                .lineLimit(1)
                        } else if isChannel {
                            Text("\(channel.members.count) members")
                                .font(.system(size: 11))
                                .foregroundColor(Theme.textMuted)
                        }
                    }
                }
            }

            ToolbarItemGroup(placement: .topBarTrailing) {
                if isChannel {
                    // Voice call — green when in this call, accent when a
                    // session is active but we haven't joined, muted otherwise.
                    Button(action: { appState.startOrJoinVoice(channel: channelName) }) {
                        let inThisCall = appState.isInCall
                            && appState.currentCallChannel?.lowercased() == channelName.lowercased()
                        let sessionActive = appState.activeAvSessions[channelName.lowercased()] != nil
                        Image(systemName: inThisCall ? "speaker.wave.2.fill" : "speaker.wave.2")
                            .font(.system(size: 16, weight: .semibold))
                            .foregroundColor(
                                inThisCall ? Theme.success
                                : (sessionActive ? Theme.accent : Theme.textSecondary)
                            )
                    }

                    Button(action: { showingSearch = true }) {
                        Image(systemName: "magnifyingglass")
                            .font(.system(size: 14))
                            .foregroundColor(Theme.textSecondary)
                    }

                    Button(action: { showingMembers.toggle() }) {
                        Image(systemName: "person.2")
                            .font(.system(size: 14))
                            .foregroundColor(Theme.textSecondary)
                    }
                }
            }
        }
        .onAppear {
            appState.activeChannel = channelName
            appState.markRead(channelName)
        }
        .onDisappear {
            // Clear activeChannel so unread counting works for this channel
            if appState.activeChannel == channelName {
                appState.activeChannel = nil
            }
        }
        .sheet(isPresented: $showingSearch) {
            SearchSheet()
                .presentationDetents([.large])
        }
    }

    private func typingText(_ typers: [String]) -> String {
        switch typers.count {
        case 1: return "\(typers[0]) is typing..."
        case 2: return "\(typers[0]) and \(typers[1]) are typing..."
        default: return "Several people are typing..."
        }
    }
}
