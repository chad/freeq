import AppKit
import Foundation

/// Test-mode automation bridge. Only started when `FREEQ_TEST_NICK` is set.
///
/// Polls a command file (default `/tmp/freeq-cmd`, override with
/// `FREEQ_CMD_FILE`) and, for each newly-appended line, either:
///   - routes a `#`-prefixed *directive* to navigation/UI state, or
///   - feeds anything else (plain text or `/slash` command) through the exact
///     same `AppState.submitInput` path the compose box uses.
///
/// This lets the app be driven deterministically for screenshot-based UI
/// testing without fragile GUI-event injection. It is inert in normal use.
final class DebugBridge {
    private weak var appState: AppState?
    private let path: String
    private var processedLines = 0
    private var timer: Timer?

    init(appState: AppState) {
        self.appState = appState
        self.path = ProcessInfo.processInfo.environment["FREEQ_CMD_FILE"] ?? "/tmp/freeq-cmd"
    }

    func start() {
        // Start from the current end of the file so we don't replay stale lines.
        processedLines = (try? String(contentsOfFile: path, encoding: .utf8))?
            .split(separator: "\n", omittingEmptySubsequences: false).count ?? 0
        NSLog("[debug-bridge] watching \(path) (starting at line \(processedLines))")
        timer = Timer.scheduledTimer(withTimeInterval: 0.3, repeats: true) { [weak self] _ in
            self?.poll()
        }
    }

    private func poll() {
        guard let content = try? String(contentsOfFile: path, encoding: .utf8) else { return }
        let lines = content.split(separator: "\n", omittingEmptySubsequences: false).map(String.init)
        guard lines.count > processedLines else { return }
        let newLines = lines[processedLines...]
        processedLines = lines.count
        for line in newLines {
            let cmd = line.trimmingCharacters(in: .whitespaces)
            if cmd.isEmpty { continue }
            run(cmd)
        }
    }

    private func run(_ line: String) {
        guard let app = appState else { return }
        NSLog("[debug-bridge] » \(line)")
        if line.hasPrefix("#") {
            runDirective(line, app: app)
        } else {
            let target = app.activeChannel ?? ""
            guard !target.isEmpty else {
                NSLog("[debug-bridge] no active channel for: \(line)")
                return
            }
            app.submitInput(line, target: target)
        }
    }

    /// `#`-prefixed UI/navigation directives.
    private func runDirective(_ line: String, app: AppState) {
        let parts = line.dropFirst().split(separator: " ", maxSplits: 1).map(String.init)
        let cmd = parts.first?.lowercased() ?? ""
        let arg = parts.count > 1 ? parts[1] : ""
        switch cmd {
        case "active":
            app.activeChannel = arg
        case "join":
            let ch = arg.hasPrefix("#") ? arg : "#\(arg)"
            app.joinChannel(ch)
            app.activeChannel = ch
        case "detail":
            app.showDetailPanel = (arg != "off")
        case "search":
            app.showSearch = (arg != "off")
        case "quickswitch":
            app.showQuickSwitcher = (arg != "off")
        case "bookmarks":
            app.showBookmarks = (arg != "off")
        case "channellist":
            app.showChannelList = (arg != "off")
        case "joinsheet":
            app.showJoinSheet = (arg != "off")
        case "thread":
            if let ch = app.activeChannelState,
               let idx = ch.findMessage(byId: arg) {
                app.threadRootMessage = ch.messages[idx]
            }
        case "unthread":
            app.threadRootMessage = nil
        case "settings":
            // open the standard Settings scene
            NSApp.sendAction(Selector(("showSettingsWindow:")), to: nil, from: nil)
        default:
            NSLog("[debug-bridge] unknown directive: \(line)")
        }
    }
}
