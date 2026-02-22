import Foundation
import SwiftUI

/// A single chat message.
struct ChatMessage: Identifiable, Equatable {
    let id: String  // msgid or UUID
    let from: String
    var text: String
    let isAction: Bool
    let timestamp: Date
    let replyTo: String?
    var isEdited: Bool = false
    var isDeleted: Bool = false
    var reactions: [String: Set<String>] = [:]  // emoji -> set of nicks

    static func == (lhs: ChatMessage, rhs: ChatMessage) -> Bool {
        lhs.id == rhs.id
    }
}

/// A channel with its messages and members.
class ChannelState: ObservableObject, Identifiable {
    let name: String
    @Published var messages: [ChatMessage] = []
    @Published var members: [MemberInfo] = []
    @Published var topic: String = ""
    @Published var typingUsers: [String: Date] = [:]  // nick -> last typing time

    var id: String { name }

    var activeTypers: [String] {
        let cutoff = Date().addingTimeInterval(-5)
        return typingUsers.filter { $0.value > cutoff }.map { $0.key }.sorted()
    }

    init(name: String) {
        self.name = name
    }

    private var messageIds: Set<String> = []

    func findMessage(byId id: String) -> Int? {
        messages.firstIndex(where: { $0.id == id })
    }

    /// Append a message only if its ID hasn't been seen before.
    func appendIfNew(_ msg: ChatMessage) {
        guard !messageIds.contains(msg.id) else { return }
        messageIds.insert(msg.id)
        messages.append(msg)
    }
}

/// Member info for the member list.
struct MemberInfo: Identifiable, Equatable {
    let nick: String
    let isOp: Bool
    let isVoiced: Bool

    var id: String { nick }

    var prefix: String {
        if isOp { return "@" }
        if isVoiced { return "+" }
        return ""
    }
}

/// Connection state.
enum ConnectionState {
    case disconnected
    case connecting
    case connected
    case registered
}

/// Main application state — bridges the Rust SDK to SwiftUI.
class AppState: ObservableObject {
    @Published var connectionState: ConnectionState = .disconnected
    @Published var nick: String = ""
    @Published var serverAddress: String = "irc.freeq.at:6667"
    @Published var channels: [ChannelState] = []
    @Published var activeChannel: String? = nil
    @Published var errorMessage: String? = nil
    @Published var authenticatedDID: String? = nil
    @Published var dmBuffers: [ChannelState] = []
    @Published var autoJoinChannels: [String] = ["#general"]
    @Published var unreadCounts: [String: Int] = [:]

    /// For reply UI
    @Published var replyingTo: ChatMessage? = nil
    /// For edit UI
    @Published var editingMessage: ChatMessage? = nil
    /// Image lightbox
    @Published var lightboxURL: URL? = nil
    /// Pending web-token for SASL auth (from AT Protocol OAuth)
    var pendingWebToken: String? = nil

    /// Read position tracking — channel name -> last read message ID
    @Published var lastReadMessageIds: [String: String] = [:]

    /// Theme
    @Published var isDarkTheme: Bool = true

    private var client: FreeqClient? = nil
    private var typingTimer: Timer? = nil
    private var lastTypingSent: Date = .distantPast

    var activeChannelState: ChannelState? {
        if let name = activeChannel {
            return channels.first { $0.name == name } ?? dmBuffers.first { $0.name == name }
        }
        return nil
    }

    init() {
        if let savedNick = UserDefaults.standard.string(forKey: "freeq.nick") {
            nick = savedNick
        }
        if let savedServer = UserDefaults.standard.string(forKey: "freeq.server") {
            serverAddress = savedServer
        }
        if let savedChannels = UserDefaults.standard.stringArray(forKey: "freeq.channels") {
            autoJoinChannels = savedChannels
        }
        if let savedReadPositions = UserDefaults.standard.dictionary(forKey: "freeq.readPositions") as? [String: String] {
            lastReadMessageIds = savedReadPositions
        }
        isDarkTheme = UserDefaults.standard.object(forKey: "freeq.darkTheme") as? Bool ?? true

        // Prune stale typing indicators every 3 seconds
        Timer.scheduledTimer(withTimeInterval: 3, repeats: true) { [weak self] _ in
            DispatchQueue.main.async {
                self?.pruneTypingIndicators()
            }
        }
    }

    func connect(nick: String) {
        self.nick = nick
        self.connectionState = .connecting
        self.errorMessage = nil

        UserDefaults.standard.set(nick, forKey: "freeq.nick")
        UserDefaults.standard.set(serverAddress, forKey: "freeq.server")

        do {
            let handler = SwiftEventHandler(appState: self)
            client = try FreeqClient(
                server: serverAddress,
                nick: nick,
                handler: handler
            )

            // Set web-token for SASL auth if available (from AT Protocol OAuth)
            if let token = pendingWebToken {
                try client?.setWebToken(token: token)
                pendingWebToken = nil
            }

            try client?.connect()
        } catch {
            DispatchQueue.main.async {
                self.connectionState = .disconnected
                self.errorMessage = "Connection failed: \(error)"
            }
        }
    }

    func disconnect() {
        client?.disconnect()
        DispatchQueue.main.async {
            self.connectionState = .disconnected
            self.channels = []
            self.dmBuffers = []
            self.activeChannel = nil
            self.replyingTo = nil
            self.editingMessage = nil
        }
    }

    func joinChannel(_ channel: String) {
        let ch = channel.hasPrefix("#") ? channel : "#\(channel)"
        do { try client?.join(channel: ch) }
        catch { DispatchQueue.main.async { self.errorMessage = "Failed to join \(ch)" } }
    }

    func partChannel(_ channel: String) {
        try? client?.part(channel: channel)
    }

    func sendMessage(target: String, text: String) {
        guard !text.isEmpty else { return }
        // Clear typing indicator for remote users
        sendRaw("@+typing=done TAGMSG \(target)")
        lastTypingSent = .distantPast

        // Check for edit mode
        if let editing = editingMessage {
            sendRaw("PRIVMSG \(target) :\(text)\r\n")
            // Actually send with edit tag via raw
            let escaped = text.replacingOccurrences(of: "\r", with: "").replacingOccurrences(of: "\n", with: " ")
            sendRaw("@+draft/edit=\(editing.id) PRIVMSG \(target) :\(escaped)")
            editingMessage = nil
            return
        }

        // Check for reply mode
        if let reply = replyingTo {
            let escaped = text.replacingOccurrences(of: "\r", with: "").replacingOccurrences(of: "\n", with: " ")
            sendRaw("@+reply=\(reply.id) PRIVMSG \(target) :\(escaped)")
            replyingTo = nil
            return
        }

        do { try client?.sendMessage(target: target, text: text) }
        catch { DispatchQueue.main.async { self.errorMessage = "Send failed" } }
    }

    func sendRaw(_ line: String) {
        try? client?.sendRaw(line: line)
    }

    func sendReaction(target: String, msgId: String, emoji: String) {
        sendRaw("@+react=\(emoji);+reply=\(msgId) TAGMSG \(target)")
    }

    func deleteMessage(target: String, msgId: String) {
        sendRaw("@+draft/delete=\(msgId) TAGMSG \(target)")
    }

    func sendTyping(target: String) {
        let now = Date()
        guard now.timeIntervalSince(lastTypingSent) > 3 else { return }
        lastTypingSent = now
        sendRaw("@+typing=active TAGMSG \(target)")
    }

    func requestHistory(channel: String) {
        sendRaw("CHATHISTORY LATEST \(channel) * 50")
    }

    func markRead(_ channel: String) {
        unreadCounts[channel] = 0
        // Persist last-read message ID
        if let state = channels.first(where: { $0.name == channel }) ?? dmBuffers.first(where: { $0.name == channel }),
           let lastMsg = state.messages.last {
            lastReadMessageIds[channel] = lastMsg.id
            UserDefaults.standard.set(lastReadMessageIds, forKey: "freeq.readPositions")
        }
    }

    func toggleTheme() {
        isDarkTheme.toggle()
        UserDefaults.standard.set(isDarkTheme, forKey: "freeq.darkTheme")
    }

    func incrementUnread(_ channel: String) {
        if activeChannel != channel {
            unreadCounts[channel, default: 0] += 1
        }
    }

    func getOrCreateChannel(_ name: String) -> ChannelState {
        if let existing = channels.first(where: { $0.name.lowercased() == name.lowercased() }) {
            return existing
        }
        let channel = ChannelState(name: name)
        channels.append(channel)
        return channel
    }

    func getOrCreateDM(_ nick: String) -> ChannelState {
        if let existing = dmBuffers.first(where: { $0.name.lowercased() == nick.lowercased() }) {
            return existing
        }
        let dm = ChannelState(name: nick)
        dmBuffers.append(dm)
        return dm
    }

    private func pruneTypingIndicators() {
        let cutoff = Date().addingTimeInterval(-5)
        for ch in channels + dmBuffers {
            let stale = ch.typingUsers.filter { $0.value < cutoff }
            if !stale.isEmpty {
                for key in stale.keys {
                    ch.typingUsers.removeValue(forKey: key)
                }
            }
        }
    }
}

/// Bridges Rust SDK events to SwiftUI state updates on main thread.
final class SwiftEventHandler: @unchecked Sendable, EventHandler {
    private weak var appState: AppState?

    init(appState: AppState) {
        self.appState = appState
    }

    func onEvent(event: FreeqEvent) {
        DispatchQueue.main.async { [weak self] in
            self?.handleEvent(event)
        }
    }

    private func handleEvent(_ event: FreeqEvent) {
        guard let state = appState else { return }

        switch event {
        case .connected:
            state.connectionState = .connected

        case .registered(let nick):
            state.connectionState = .registered
            state.nick = nick
            // Auto-join saved channels
            for channel in state.autoJoinChannels {
                state.joinChannel(channel)
            }

        case .authenticated(let did):
            state.authenticatedDID = did

        case .authFailed(let reason):
            state.errorMessage = "Auth failed: \(reason)"

        case .joined(let channel, let nick):
            let ch = state.getOrCreateChannel(channel)
            if nick.lowercased() == state.nick.lowercased() {
                if state.activeChannel == nil {
                    state.activeChannel = channel
                }
                if !state.autoJoinChannels.contains(where: { $0.lowercased() == channel.lowercased() }) {
                    state.autoJoinChannels.append(channel)
                    UserDefaults.standard.set(state.autoJoinChannels, forKey: "freeq.channels")
                }
                // Request history
                state.requestHistory(channel: channel)
            }
            let msg = ChatMessage(
                id: UUID().uuidString, from: "", text: "\(nick) joined",
                isAction: false, timestamp: Date(), replyTo: nil
            )
            ch.appendIfNew(msg)

        case .parted(let channel, let nick):
            if nick.lowercased() == state.nick.lowercased() {
                state.channels.removeAll { $0.name == channel }
                state.autoJoinChannels.removeAll { $0.lowercased() == channel.lowercased() }
                UserDefaults.standard.set(state.autoJoinChannels, forKey: "freeq.channels")
                if state.activeChannel == channel {
                    state.activeChannel = state.channels.first?.name
                }
            } else {
                let ch = state.getOrCreateChannel(channel)
                ch.appendIfNew(ChatMessage(
                    id: UUID().uuidString, from: "", text: "\(nick) left",
                    isAction: false, timestamp: Date(), replyTo: nil
                ))
                ch.members.removeAll { $0.nick.lowercased() == nick.lowercased() }
            }

        case .message(let ircMsg):
            let target = ircMsg.target
            let from = ircMsg.fromNick
            let isSelf = from.lowercased() == state.nick.lowercased()

            let msg = ChatMessage(
                id: ircMsg.msgid ?? UUID().uuidString,
                from: from,
                text: ircMsg.text,
                isAction: ircMsg.isAction,
                timestamp: Date(timeIntervalSince1970: Double(ircMsg.timestampMs) / 1000.0),
                replyTo: ircMsg.replyTo
            )

            if target.hasPrefix("#") {
                let ch = state.getOrCreateChannel(target)
                ch.appendIfNew(msg)
                state.incrementUnread(target)
                ch.typingUsers.removeValue(forKey: from)

                // Notify on mention
                if !isSelf && ircMsg.text.lowercased().contains(state.nick.lowercased()) {
                    NotificationManager.shared.sendMessageNotification(
                        from: from, text: ircMsg.text, channel: target
                    )
                }
            } else {
                let bufferName = isSelf ? target : from
                let dm = state.getOrCreateDM(bufferName)
                dm.appendIfNew(msg)
                state.incrementUnread(bufferName)

                // Always notify on DMs
                if !isSelf {
                    NotificationManager.shared.sendMessageNotification(
                        from: from, text: ircMsg.text, channel: bufferName
                    )
                }
            }

        case .names(let channel, let members):
            let ch = state.getOrCreateChannel(channel)
            ch.members = members.map { MemberInfo(nick: $0.nick, isOp: $0.isOp, isVoiced: $0.isVoiced) }
            // Prefetch avatars for all channel members
            let nicks = members.map { $0.nick }
            Task { @MainActor in
                AvatarCache.shared.prefetchAll(nicks)
            }

        case .topicChanged(let channel, let topic):
            let ch = state.getOrCreateChannel(channel)
            ch.topic = topic.text

        case .modeChanged(_, _, _, _):
            break

        case .kicked(let channel, let nick, let by, let reason):
            if nick.lowercased() == state.nick.lowercased() {
                state.channels.removeAll { $0.name == channel }
                state.autoJoinChannels.removeAll { $0.lowercased() == channel.lowercased() }
                UserDefaults.standard.set(state.autoJoinChannels, forKey: "freeq.channels")
                if state.activeChannel == channel {
                    state.activeChannel = state.channels.first?.name
                }
                state.errorMessage = "Kicked from \(channel) by \(by): \(reason)"
            } else {
                let ch = state.getOrCreateChannel(channel)
                ch.appendIfNew(ChatMessage(
                    id: UUID().uuidString, from: "",
                    text: "\(nick) was kicked by \(by) (\(reason))",
                    isAction: false, timestamp: Date(), replyTo: nil
                ))
                ch.members.removeAll { $0.nick.lowercased() == nick.lowercased() }
            }

        case .userQuit(let nick, _):
            for ch in state.channels {
                ch.members.removeAll { $0.nick.lowercased() == nick.lowercased() }
                ch.typingUsers.removeValue(forKey: nick)
            }

        case .notice(let text):
            if !text.isEmpty { print("Notice: \(text)") }

        case .disconnected(let reason):
            state.connectionState = .disconnected
            if !reason.isEmpty {
                state.errorMessage = "Disconnected: \(reason)"
            }
        }
    }
}
