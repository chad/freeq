import SwiftUI

@main
struct FreeqApp: App {
    @State private var appState = AppState()
    @State private var showOnboarding = !UserDefaults.standard.bool(forKey: "freeq.onboardingComplete")

    var body: some Scene {
        WindowGroup {
            MainView()
                .environment(appState)
                .frame(minWidth: 700, minHeight: 400)
                .sheet(isPresented: Binding(
                    get: { appState.showQuickSwitcher }, set: { appState.showQuickSwitcher = $0 })) {
                    QuickSwitcher()
                        .environment(appState)
                }
                .sheet(isPresented: Binding(
                    get: { appState.showBookmarks }, set: { appState.showBookmarks = $0 })) {
                    BookmarksPanel()
                        .environment(appState)
                }
                .sheet(isPresented: Binding(
                    get: { appState.showChannelList }, set: { appState.showChannelList = $0 })) {
                    ChannelListBrowser()
                        .environment(appState)
                }
                .sheet(isPresented: $showOnboarding) {
                    OnboardingView()
                }
                .onAppear {
                    appState.startupConnect()
                }
                .onChange(of: appState.activeChannel) { _, newValue in
                    updateWindowTitle(newValue)
                }
                .onChange(of: appState.totalUnread) { _, newValue in
                    NSApplication.shared.dockTile.badgeLabel = newValue > 0 ? "\(newValue)" : nil
                }
        }
        .commands {
            CommandGroup(after: .sidebar) {
                Button("Toggle Detail Panel") {
                    appState.showDetailPanel.toggle()
                }
                .keyboardShortcut("d", modifiers: [.command, .shift])
            }

            CommandGroup(replacing: .newItem) {
                Button("Quick Switcher") {
                    appState.showQuickSwitcher = true
                }
                .keyboardShortcut("k", modifiers: .command)

                Button("Search Messages") {
                    appState.showSearch.toggle()
                }
                .keyboardShortcut("f", modifiers: .command)

                Button("Bookmarks") {
                    appState.showBookmarks = true
                }
                .keyboardShortcut("b", modifiers: [.command, .shift])

                Button("Browse Channels") {
                    appState.showChannelList = true
                }
                .keyboardShortcut("l", modifiers: [.command, .shift])

                Button("Join Channel…") {
                    appState.showJoinSheet = true
                }
                .keyboardShortcut("j", modifiers: .command)

                Divider()

                ForEach(1...9, id: \.self) { i in
                    Button("Switch to Buffer \(i)") {
                        appState.switchToChannelByIndex(i - 1)
                    }
                    .keyboardShortcut(KeyEquivalent(Character("\(i)")), modifiers: .command)
                }
            }

            CommandGroup(replacing: .help) {
                Button("freeq Help") {
                    if let ch = appState.activeChannelState {
                        ch.appendIfNew(ChatMessage(
                            id: UUID().uuidString, from: "system",
                            text: "Type /help for a list of commands",
                            isAction: false, timestamp: Date(), replyTo: nil
                        ))
                    }
                }
            }
        }

        Settings {
            SettingsView()
                .environment(appState)
        }
    }

    private func updateWindowTitle(_ channel: String?) {
        DispatchQueue.main.async {
            if let channel {
                NSApplication.shared.mainWindow?.title = "\(channel) — freeq"
            } else {
                NSApplication.shared.mainWindow?.title = "freeq"
            }
        }
    }
}
