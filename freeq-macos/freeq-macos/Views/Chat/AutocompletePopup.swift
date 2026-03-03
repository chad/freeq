import SwiftUI

/// Autocomplete popup for @mentions, /commands, and :emoji:
struct AutocompletePopup: View {
    @Environment(AppState.self) private var appState
    @Binding var text: String
    @Binding var selectedIndex: Int
    let anchor: CGPoint  // Not used for positioning (overlay at bottom of compose)

    enum Suggestion: Identifiable {
        case nick(String)
        case command(String, String)  // name, description
        case emoji(String, String)  // shortcode, emoji char

        var id: String {
            switch self {
            case .nick(let n): return "nick:\(n)"
            case .command(let c, _): return "cmd:\(c)"
            case .emoji(let s, _): return "emoji:\(s)"
            }
        }

        var display: String {
            switch self {
            case .nick(let n): return "@\(n)"
            case .command(let c, let d): return "/\(c) — \(d)"
            case .emoji(let s, let e): return "\(e)  :\(s):"
            }
        }

        var completion: String {
            switch self {
            case .nick(let n): return "@\(n) "
            case .command(let c, _): return "/\(c) "
            case .emoji(_, let e): return "\(e)"
            }
        }
    }

    static let commands: [(String, String)] = [
        ("join", "Join a channel"),
        ("part", "Leave current channel"),
        ("topic", "Set channel topic"),
        ("nick", "Change nickname"),
        ("me", "Send an action"),
        ("msg", "Send a direct message"),
        ("kick", "Kick a user"),
        ("op", "Give operator status"),
        ("deop", "Remove operator status"),
        ("voice", "Give voice"),
        ("invite", "Invite user to channel"),
        ("away", "Set away status"),
        ("whois", "Look up user info"),
        ("mode", "Set channel/user mode"),
        ("raw", "Send raw IRC command"),
        ("p2p", "P2P commands"),
        ("help", "Show help"),
    ]

    static let commonEmoji: [(String, String)] = [
        ("thumbsup", "👍"), ("thumbsdown", "👎"), ("heart", "❤️"), ("fire", "🔥"),
        ("laugh", "😂"), ("smile", "😊"), ("thinking", "🤔"), ("eyes", "👀"),
        ("rocket", "🚀"), ("100", "💯"), ("tada", "🎉"), ("wave", "👋"),
        ("clap", "👏"), ("pray", "🙏"), ("star", "⭐"), ("check", "✅"),
        ("x", "❌"), ("warning", "⚠️"), ("bug", "🐛"), ("sparkles", "✨"),
        ("zap", "⚡"), ("skull", "💀"), ("sob", "😭"), ("rolling_eyes", "🙄"),
        ("shrug", "🤷"), ("sunglasses", "😎"), ("nerd", "🤓"), ("salute", "🫡"),
        ("brain", "🧠"), ("gem", "💎"), ("trophy", "🏆"), ("party", "🥳"),
    ]

    var suggestions: [Suggestion] {
        let t = text
        // @mention
        if let atRange = t.range(of: "@", options: .backwards),
           t.distance(from: atRange.lowerBound, to: t.endIndex) <= 20,
           (atRange.lowerBound == t.startIndex || t[t.index(before: atRange.lowerBound)] == " ") {
            let prefix = String(t[atRange.upperBound...]).lowercased()
            let members = appState.activeChannelState?.members.map(\.nick) ?? []
            return members
                .filter { prefix.isEmpty || $0.lowercased().hasPrefix(prefix) }
                .prefix(8)
                .map { .nick($0) }
        }

        // /command
        if t.hasPrefix("/") && !t.contains(" ") {
            let prefix = String(t.dropFirst()).lowercased()
            return Self.commands
                .filter { prefix.isEmpty || $0.0.hasPrefix(prefix) }
                .prefix(8)
                .map { .command($0.0, $0.1) }
        }

        // :emoji:
        if let colonRange = t.range(of: ":", options: .backwards),
           t.distance(from: colonRange.lowerBound, to: t.endIndex) >= 2,
           t.distance(from: colonRange.lowerBound, to: t.endIndex) <= 15,
           (colonRange.lowerBound == t.startIndex || t[t.index(before: colonRange.lowerBound)] == " ") {
            let prefix = String(t[colonRange.upperBound...]).lowercased()
            guard !prefix.isEmpty else { return [] }
            return Self.commonEmoji
                .filter { $0.0.contains(prefix) }
                .prefix(8)
                .map { .emoji($0.0, $0.1) }
        }

        return []
    }

    var body: some View {
        let items = suggestions
        if !items.isEmpty {
            VStack(alignment: .leading, spacing: 0) {
                ForEach(Array(items.enumerated()), id: \.element.id) { index, item in
                    Button {
                        accept(item)
                    } label: {
                        HStack {
                            Text(item.display)
                                .font(.system(.body, design: .default))
                            Spacer()
                        }
                        .padding(.horizontal, 12)
                        .padding(.vertical, 6)
                        .background(index == selectedIndex ? Color.accentColor.opacity(0.15) : Color.clear)
                        .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                }
            }
            .frame(maxWidth: 350)
            .background(.regularMaterial)
            .clipShape(RoundedRectangle(cornerRadius: 8))
            .shadow(color: .black.opacity(0.15), radius: 8, y: -4)
            .padding(.horizontal, 16)
            .padding(.bottom, 4)
        }
    }

    private func accept(_ item: Suggestion) {
        switch item {
        case .nick(let nick):
            // Replace @prefix with @nick
            if let atRange = text.range(of: "@", options: .backwards) {
                text = String(text[..<atRange.lowerBound]) + "@\(nick) "
            }
        case .command(let cmd, _):
            text = "/\(cmd) "
        case .emoji(_, let char):
            // Replace :prefix with emoji
            if let colonRange = text.range(of: ":", options: .backwards) {
                text = String(text[..<colonRange.lowerBound]) + char
            }
        }
        selectedIndex = 0
    }
}
