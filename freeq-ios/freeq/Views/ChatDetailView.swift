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
