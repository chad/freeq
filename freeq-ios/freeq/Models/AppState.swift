import Foundation
import SwiftUI

/// A single chat message.
struct ChatMessage: Identifiable, Equatable {
    var id: String  // msgid or UUID
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
    /// Inserts in timestamp order to handle CHATHISTORY arriving after live messages.
    func appendIfNew(_ msg: ChatMessage) {
        guard !messageIds.contains(msg.id) else { return }
        messageIds.insert(msg.id)

        // If the message is older than the last message, insert in sorted position
        if let last = messages.last, msg.timestamp < last.timestamp {
            let idx = messages.firstIndex(where: { $0.timestamp > msg.timestamp }) ?? messages.endIndex
            messages.insert(msg, at: idx)
        } else {
            messages.append(msg)
        }
    }

    func applyEdit(originalId: String, newId: String?, newText: String) {
        if let idx = findMessage(byId: originalId) {
            messages[idx].text = newText
            messages[idx].isEdited = true
            if let newId = newId {
                messages[idx].id = newId
                messageIds.insert(newId)
            }
        }
    }

    func applyDelete(msgId: String) {
        if let idx = findMessage(byId: msgId) {
            messages[idx].isDeleted = true
            messages[idx].text = ""
        }
    }

    func applyReaction(msgId: String, emoji: String, from: String) {
        if let idx = findMessage(byId: msgId) {
            var reactions = messages[idx].reactions
            var nicks = reactions[emoji] ?? Set<String>()
            nicks.insert(from)
            reactions[emoji] = nicks
            messages[idx].reactions = reactions
        }
    }
}

/// Member info for the member list.
struct MemberInfo: Identifiable, Equatable {
    let nick: String
    let isOp: Bool
    let isHalfop: Bool
    let isVoiced: Bool
    let awayMsg: String?

    var id: String { nick }

    var prefix: String {
        if isOp { return "@" }
        if isHalfop { return "%" }
        if isVoiced { return "+" }
        return ""
    }

    var isAway: Bool { awayMsg != nil }
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
    struct BatchBuffer {
        let target: String
        var messages: [ChatMessage]
    }

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

    // In-flight CHATHISTORY batches
    private var batches: [String: BatchBuffer] = [:]

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

    /// Whether we have a saved session that should auto-reconnect
    var hasSavedSession: Bool {
        let lastLogin = UserDefaults.standard.double(forKey: "freeq.lastLogin")
        let twoWeeks: TimeInterval = 14 * 24 * 60 * 60
        return lastLogin > 0
            && Date().timeIntervalSince1970 - lastLogin < twoWeeks
            && !nick.isEmpty
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
        if let savedDID = UserDefaults.standard.string(forKey: "freeq.did") {
            authenticatedDID = savedDID
        }
        isDarkTheme = UserDefaults.standard.object(forKey: "freeq.darkTheme") as? Bool ?? true

        // Prune stale typing indicators every 3 seconds
        Timer.scheduledTimer(withTimeInterval: 3, repeats: true) { [weak self] _ in
            DispatchQueue.main.async {
                self?.pruneTypingIndicators()
            }
        }
    }

    /// Reconnect with saved session (no SASL — connects as guest with saved nick)
    func reconnectSavedSession() {
        guard hasSavedSession, connectionState == .disconnected else { return }
        connect(nick: nick)
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

    /// Full logout — clears saved session so ConnectView shows next launch
    func logout() {
        disconnect()
        UserDefaults.standard.removeObject(forKey: "freeq.lastLogin")
        UserDefaults.standard.removeObject(forKey: "freeq.did")
        UserDefaults.standard.removeObject(forKey: "freeq.nick")
        UserDefaults.standard.removeObject(forKey: "freeq.handle")
        DispatchQueue.main.async {
            self.authenticatedDID = nil
            self.nick = ""
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

    func requestHistory(channel: String, before: Date? = nil) {
        if let before = before {
            let iso = ISO8601DateFormatter().string(from: before)
            sendRaw("CHATHISTORY BEFORE \(channel) timestamp=\(iso) 50")
        } else {
            sendRaw("CHATHISTORY LATEST \(channel) * 50")
        }
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

    private func updateAwayStatus(nick: String, awayMsg: String?) {
        for ch in channels {
            if let idx = ch.members.firstIndex(where: { $0.nick.lowercased() == nick.lowercased() }) {
                let m = ch.members[idx]
                ch.members[idx] = MemberInfo(nick: m.nick, isOp: m.isOp, isHalfop: m.isHalfop, isVoiced: m.isVoiced, awayMsg: awayMsg)
            }
        }
    }

    private func renameUser(oldNick: String, newNick: String) {
        for ch in channels {
            if let idx = ch.members.firstIndex(where: { $0.nick.lowercased() == oldNick.lowercased() }) {
                let m = ch.members[idx]
                ch.members[idx] = MemberInfo(nick: newNick, isOp: m.isOp, isHalfop: m.isHalfop, isVoiced: m.isVoiced, awayMsg: m.awayMsg)
            }
            if let ts = ch.typingUsers.removeValue(forKey: oldNick) {
                ch.typingUsers[newNick] = ts
            }
        }

        if let idx = dmBuffers.firstIndex(where: { $0.name.lowercased() == oldNick.lowercased() }) {
            let old = dmBuffers[idx]
            let renamed = ChannelState(name: newNick)
            renamed.messages = old.messages
            renamed.members = old.members
            renamed.topic = old.topic
            renamed.typingUsers = old.typingUsers
            dmBuffers.remove(at: idx)
            dmBuffers.append(renamed)

            if let count = unreadCounts.removeValue(forKey: old.name) {
                unreadCounts[newNick] = count
            }
            if let last = lastReadMessageIds.removeValue(forKey: old.name) {
                lastReadMessageIds[newNick] = last
                UserDefaults.standard.set(lastReadMessageIds, forKey: "freeq.readPositions")
            }
        }

        if activeChannel?.lowercased() == oldNick.lowercased() {
            activeChannel = newNick
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
            UserDefaults.standard.set(did, forKey: "freeq.did")

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
                // Don't show "you joined" system message — the user knows they joined
            } else {
                let msg = ChatMessage(
                    id: UUID().uuidString, from: "", text: "\(nick) joined",
                    isAction: false, timestamp: Date(), replyTo: nil
                )
                ch.appendIfNew(msg)
                if !ch.members.contains(where: { $0.nick.lowercased() == nick.lowercased() }) {
                    ch.members.append(MemberInfo(nick: nick, isOp: false, isHalfop: false, isVoiced: false, awayMsg: nil))
                }
            }

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

            // Handle edits
            if let editOf = ircMsg.editOf {
                if let batchId = ircMsg.batchId, var batch = state.batches[batchId] {
                    if let idx = batch.messages.firstIndex(where: { $0.id == editOf }) {
                        batch.messages[idx].text = ircMsg.text
                        batch.messages[idx].isEdited = true
                        if let newId = ircMsg.msgid { batch.messages[idx].id = newId }
                    } else {
                        batch.messages.append(msg)
                    }
                    state.batches[batchId] = batch
                    return
                }

                if target.hasPrefix("#") {
                    let ch = state.getOrCreateChannel(target)
                    ch.applyEdit(originalId: editOf, newId: ircMsg.msgid, newText: ircMsg.text)
                } else {
                    let bufferName = isSelf ? target : from
                    let dm = state.getOrCreateDM(bufferName)
                    dm.applyEdit(originalId: editOf, newId: ircMsg.msgid, newText: ircMsg.text)
                }
                return
            }

            // If part of CHATHISTORY batch, buffer it for later merge
            if let batchId = ircMsg.batchId, var batch = state.batches[batchId] {
                batch.messages.append(msg)
                state.batches[batchId] = batch
                return
            }

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
            ch.members = members.map {
                MemberInfo(nick: $0.nick, isOp: $0.isOp, isHalfop: $0.isHalfop, isVoiced: $0.isVoiced, awayMsg: $0.awayMsg)
            }
            // Prefetch avatars for all channel members
            let nicks = members.map { $0.nick }
            Task { @MainActor in
                AvatarCache.shared.prefetchAll(nicks)
            }

        case .topicChanged(let channel, let topic):
            let ch = state.getOrCreateChannel(channel)
            ch.topic = topic.text

        case .modeChanged(let channel, let mode, let arg, _):
            guard let nick = arg else { break }
            let ch = state.getOrCreateChannel(channel)
            if let idx = ch.members.firstIndex(where: { $0.nick.lowercased() == nick.lowercased() }) {
                let member = ch.members[idx]
                switch mode {
                case "+o": ch.members[idx] = MemberInfo(nick: member.nick, isOp: true, isHalfop: false, isVoiced: member.isVoiced, awayMsg: member.awayMsg)
                case "-o": ch.members[idx] = MemberInfo(nick: member.nick, isOp: false, isHalfop: member.isHalfop, isVoiced: member.isVoiced, awayMsg: member.awayMsg)
                case "+h": ch.members[idx] = MemberInfo(nick: member.nick, isOp: member.isOp, isHalfop: true, isVoiced: member.isVoiced, awayMsg: member.awayMsg)
                case "-h": ch.members[idx] = MemberInfo(nick: member.nick, isOp: member.isOp, isHalfop: false, isVoiced: member.isVoiced, awayMsg: member.awayMsg)
                case "+v": ch.members[idx] = MemberInfo(nick: member.nick, isOp: member.isOp, isHalfop: member.isHalfop, isVoiced: true, awayMsg: member.awayMsg)
                case "-v": ch.members[idx] = MemberInfo(nick: member.nick, isOp: member.isOp, isHalfop: member.isHalfop, isVoiced: false, awayMsg: member.awayMsg)
                default: break
                }
            }

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

        case .batchStart(let id, _, let target):
            state.batches[id] = AppState.BatchBuffer(target: target, messages: [])

        case .batchEnd(let id):
            guard let batch = state.batches.removeValue(forKey: id) else { return }
            let sorted = batch.messages.sorted { $0.timestamp < $1.timestamp }
            if batch.target.hasPrefix("#") {
                let ch = state.getOrCreateChannel(batch.target)
                for msg in sorted { ch.appendIfNew(msg) }
            } else {
                let dm = state.getOrCreateDM(batch.target)
                for msg in sorted { dm.appendIfNew(msg) }
            }

        case .tagMsg(let tagMsg):
            let tags = Dictionary(uniqueKeysWithValues: tagMsg.tags.map { ($0.key, $0.value) })
            let target = tagMsg.target
            let from = tagMsg.from

            // Typing indicators
            if let typing = tags["+typing"] {
                if from.lowercased() != state.nick.lowercased() {
                    let bufferName = target.hasPrefix("#") ? target : from
                    let ch = bufferName.hasPrefix("#") ? state.getOrCreateChannel(bufferName) : state.getOrCreateDM(bufferName)
                    if typing == "active" {
                        ch.typingUsers[from] = Date()
                    } else if typing == "done" {
                        ch.typingUsers.removeValue(forKey: from)
                    }
                }
            }

            // Message deletion
            if let deleteId = tags["+draft/delete"] {
                let bufferName = target.hasPrefix("#") ? target : from
                let ch = bufferName.hasPrefix("#") ? state.getOrCreateChannel(bufferName) : state.getOrCreateDM(bufferName)
                ch.applyDelete(msgId: deleteId)
            }

            // Reactions
            if let emoji = tags["+react"], let replyId = tags["+reply"] {
                let bufferName = target.hasPrefix("#") ? target : from
                let ch = bufferName.hasPrefix("#") ? state.getOrCreateChannel(bufferName) : state.getOrCreateDM(bufferName)
                ch.applyReaction(msgId: replyId, emoji: emoji, from: from)
            }

        case .batchStart(let id, _, let target):
            state.batches[id] = AppState.BatchBuffer(target: target, messages: [])

        case .batchEnd(let id):
            guard let batch = state.batches.removeValue(forKey: id) else { return }
            let sorted = batch.messages.sorted { $0.timestamp < $1.timestamp }
            if batch.target.hasPrefix("#") {
                let ch = state.getOrCreateChannel(batch.target)
                for msg in sorted { ch.appendIfNew(msg) }
            } else {
                let dm = state.getOrCreateDM(batch.target)
                for msg in sorted { dm.appendIfNew(msg) }
            }

        case .nickChanged(let oldNick, let newNick):
            state.renameUser(oldNick: oldNick, newNick: newNick)

        case .awayChanged(let nick, let awayMsg):
            state.updateAwayStatus(nick: nick, awayMsg: awayMsg)

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
            // Auto-reconnect if we have a saved session (e.g. network blip)
            if state.hasSavedSession {
                DispatchQueue.main.asyncAfter(deadline: .now() + 2.0) {
                    if state.connectionState == .disconnected && state.hasSavedSession {
                        state.reconnectSavedSession()
                    }
                }
            }
        }
    }
}
