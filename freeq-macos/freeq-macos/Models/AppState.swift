import Foundation
import SwiftUI
import UserNotifications

/// Connection transport type.
enum TransportType: Equatable {
    case tcp
    case tls
    case iroh
}

/// Connection state.
enum ConnectionState: Equatable {
    case disconnected
    case connecting
    case connected
    case registered
}

/// Main application state — bridges the Rust SDK to SwiftUI via @Observable.
@Observable
class AppState {
    // MARK: - Connection
    var connectionState: ConnectionState = .disconnected
    var transportType: TransportType = .tcp
    var nick: String = ""
    var serverAddress: String = "irc.freeq.at:6697"
    var authenticatedDID: String?
    var irohEndpointId: String?
    var reconnectAttempts: Int = 0

    // MARK: - Channels & DMs
    var channels: [ChannelState] = []
    var dmBuffers: [ChannelState] = []
    var activeChannel: String? = nil
    var unreadCounts: [String: Int] = [:]
    var mentionCounts: [String: Int] = [:]
    var autoJoinChannels: [String] = ["#freeq"]

    // MARK: - Favorites, Muted, Bookmarks
    var favorites: Set<String> = []  // lowercase channel names
    var mutedChannels: Set<String> = []  // lowercase channel names
    var bookmarks: [Bookmark] = []
    var lastReadMsgId: [String: String] = [:]  // lowercase channel → last read msgid

    struct Bookmark: Identifiable, Codable {
        var id: String { msgId }
        let channel: String
        let msgId: String
        let from: String
        let text: String
        let timestamp: Date
    }

    // MARK: - P2P
    var p2pEndpointId: String?
    var p2pConnectedPeers: Set<String> = []
    var p2pDMActive: Set<String> = []

    // MARK: - UI State
    var showDetailPanel: Bool = true
    var showQuickSwitcher: Bool = false
    var showJoinSheet: Bool = false
    var errorMessage: String?

    // MARK: - Compose state (editing/replying)
    var editingMessageId: String?
    var editingText: String?
    var replyingToMessage: ChatMessage?
    var scrollToMessageId: String?
    var showSearch: Bool = false
    var motd: String = ""
    var showMotd: Bool = false

    // MARK: - Auth
    var authBrokerBase: String = "https://auth.freeq.at"
    var brokerToken: String?
    var pendingWebToken: String?

    // MARK: - Batches (CHATHISTORY)
    struct BatchBuffer {
        let target: String
        var messages: [ChatMessage]
    }
    var batches: [String: BatchBuffer] = [:]

    // MARK: - Names accumulator (353 lines come in multiple events)
    var pendingNames: [String: [MemberInfo]] = [:]

    // MARK: - Profile cache
    var profileCache = ProfileCache.shared

    // MARK: - Typing debounce
    private var lastTypingSent: [String: Date] = [:]

    // MARK: - Private
    private var client: FreeqClient?
    private var p2p: FreeqP2p?

    // MARK: - Computed

    var activeChannelState: ChannelState? {
        guard let name = activeChannel else { return nil }
        return channels.first { $0.name.lowercased() == name.lowercased() }
            ?? dmBuffers.first { $0.name.lowercased() == name.lowercased() }
    }

    var allBuffers: [ChannelState] {
        channels + dmBuffers
    }

    var totalUnread: Int {
        unreadCounts.values.reduce(0, +)
    }

    var isP2pActive: Bool { p2pEndpointId != nil }

    var hasSavedSession: Bool {
        brokerToken != nil && !nick.isEmpty
    }

    // MARK: - Init

    init() {
        loadSavedState()
        requestNotificationPermission()
    }

    private func loadSavedState() {
        if let saved = UserDefaults.standard.string(forKey: "freeq.nick") {
            nick = saved
        }
        if let saved = UserDefaults.standard.string(forKey: "freeq.server") {
            serverAddress = saved
        }
        if let saved = UserDefaults.standard.stringArray(forKey: "freeq.channels"), !saved.isEmpty {
            autoJoinChannels = saved
        }
        brokerToken = KeychainHelper.load(key: "brokerToken")
        authenticatedDID = KeychainHelper.load(key: "did")
        favorites = Set(UserDefaults.standard.stringArray(forKey: "freeq.favorites") ?? [])
        mutedChannels = Set(UserDefaults.standard.stringArray(forKey: "freeq.muted") ?? [])
        if let data = UserDefaults.standard.data(forKey: "freeq.bookmarks"),
           let saved = try? JSONDecoder().decode([Bookmark].self, from: data) {
            bookmarks = saved
        }

        // Wake from sleep → reconnect
        NSWorkspace.shared.notificationCenter.addObserver(
            forName: NSWorkspace.didWakeNotification, object: nil, queue: .main
        ) { [weak self] _ in
            guard let self, self.connectionState == .disconnected, self.hasSavedSession else { return }
            Log.irc.info("System wake detected — reconnecting")
            DispatchQueue.main.asyncAfter(deadline: .now() + 2) {
                self.reconnectIfSaved()
            }
        }
    }

    private func requestNotificationPermission() {
        UNUserNotificationCenter.current().requestAuthorization(options: [.alert, .sound, .badge]) { _, _ in }
    }

    // MARK: - Connection

    func connect(nick: String, webToken: String? = nil) {
        Log.irc.info("Connecting as \(nick, privacy: .public)")
        self.nick = nick
        connectionState = .connecting
        UserDefaults.standard.set(nick, forKey: "freeq.nick")

        let handler = AppEventHandler(appState: self)

        do {
            let c = try FreeqClient(
                server: serverAddress,
                nick: nick,
                handler: handler
            )
            self.client = c

            if let token = webToken ?? pendingWebToken {
                try c.setWebToken(token: token)
                pendingWebToken = nil
            }

            try c.setPlatform(platform: "macOS")
            try c.connect()
        } catch {
            connectionState = .disconnected
            errorMessage = "Connection failed: \(error.localizedDescription)"
        }
    }

    func disconnect() {
        client?.disconnect()
        client = nil
        connectionState = .disconnected
        shutdownP2p()
    }

    func logout() {
        disconnect()
        brokerToken = nil
        authenticatedDID = nil
        pendingWebToken = nil
        KeychainHelper.delete(key: "brokerToken")
        KeychainHelper.delete(key: "did")
        channels.removeAll()
        dmBuffers.removeAll()
        activeChannel = nil
    }

    func reconnectIfSaved() {
        guard connectionState == .disconnected, hasSavedSession else { return }

        Task {
            do {
                let session = try await BrokerAuth.fetchSession(
                    brokerBase: authBrokerBase,
                    brokerToken: brokerToken!
                )
                await MainActor.run {
                    self.pendingWebToken = session.token
                    self.authenticatedDID = session.did
                    KeychainHelper.save(key: "did", value: session.did)
                    self.connect(nick: session.nick)
                }
            } catch {
                // Silent — will retry on next attempt
            }
        }
    }

    // MARK: - Send

    func sendMessage(to target: String, text: String) {
        // Try P2P first for DMs
        if !target.hasPrefix("#"),
           let peerEndpoint = p2pEndpointForNick(target) {
            try? p2p?.sendMessage(peerId: peerEndpoint, text: text)
            let msg = ChatMessage(
                id: UUID().uuidString,
                from: nick,
                text: text,
                isAction: false,
                timestamp: Date(),
                replyTo: nil
            )
            getOrCreateDM(target).appendIfNew(msg)
            return
        }

        // Server-relayed
        do {
            try client?.sendMessage(target: target, text: text)
        } catch {
            errorMessage = "Send failed: \(error.localizedDescription)"
        }
    }

    func sendAction(to target: String, text: String) {
        sendRaw("PRIVMSG \(target) :\u{01}ACTION \(text)\u{01}")
    }

    func editMessage(target: String, msgId: String, newText: String) {
        sendRaw("@+draft/edit=\(msgId) PRIVMSG \(target) :\(newText)")
    }

    func deleteMessage(target: String, msgId: String) {
        sendRaw("@+draft/delete=\(msgId) TAGMSG \(target)")
    }

    func sendReaction(target: String, msgId: String, emoji: String) {
        sendRaw("@+react=\(emoji);+reply=\(msgId) TAGMSG \(target)")
    }

    func sendTyping(target: String) {
        let now = Date()
        let key = target.lowercased()
        if let last = lastTypingSent[key], now.timeIntervalSince(last) < 3 { return }
        lastTypingSent[key] = now
        sendRaw("@+typing=active TAGMSG \(target)")
    }

    func joinChannel(_ channel: String) {
        let ch = channel.hasPrefix("#") ? channel : "#\(channel)"
        do {
            try client?.join(channel: ch)
        } catch {
            errorMessage = "Join failed: \(error.localizedDescription)"
        }
    }

    func partChannel(_ channel: String) {
        do {
            try client?.part(channel: channel)
            channels.removeAll { $0.name.lowercased() == channel.lowercased() }
            autoJoinChannels.removeAll { $0.lowercased() == channel.lowercased() }
            UserDefaults.standard.set(autoJoinChannels, forKey: "freeq.channels")
            if activeChannel?.lowercased() == channel.lowercased() {
                activeChannel = channels.first?.name
            }
        } catch {
            errorMessage = "Part failed: \(error.localizedDescription)"
        }
    }

    func sendRaw(_ line: String) {
        try? client?.sendRaw(line: line)
    }

    func requestHistory(channel: String, before: Date? = nil) {
        if let before {
            let iso = ISO8601DateFormatter().string(from: before)
            sendRaw("CHATHISTORY BEFORE \(channel) timestamp=\(iso) 50")
        } else {
            sendRaw("CHATHISTORY LATEST \(channel) * 50")
        }
    }

    func setAway(_ reason: String?) {
        if let reason {
            sendRaw("AWAY :\(reason)")
        } else {
            sendRaw("AWAY")
        }
    }

    func kickUser(_ channel: String, _ nick: String, reason: String? = nil) {
        if let reason {
            sendRaw("KICK \(channel) \(nick) :\(reason)")
        } else {
            sendRaw("KICK \(channel) \(nick)")
        }
    }

    func setMode(_ channel: String, _ mode: String, _ nick: String) {
        sendRaw("MODE \(channel) \(mode) \(nick)")
    }

    func inviteUser(_ channel: String, _ nick: String) {
        sendRaw("INVITE \(nick) \(channel)")
    }

    func sendWhois(_ nick: String) {
        sendRaw("WHOIS \(nick)")
    }

    // MARK: - P2P (iroh)

    func startP2p() {
        guard p2p == nil else { return }
        let handler = AppP2pHandler(appState: self)
        do {
            let p2p = try FreeqP2p(handler: handler)
            self.p2p = p2p
            self.p2pEndpointId = try p2p.endpointId()
        } catch {
            // P2P is optional — don't show error
        }
    }

    func shutdownP2p() {
        p2p?.shutdown()
        p2p = nil
        p2pEndpointId = nil
        p2pConnectedPeers.removeAll()
        p2pDMActive.removeAll()
    }

    func connectP2pPeer(_ endpointId: String) {
        do {
            try p2p?.connectPeer(endpointId: endpointId)
        } catch {
            errorMessage = "P2P connect failed: \(error.localizedDescription)"
        }
    }

    private func p2pEndpointForNick(_ nick: String) -> String? {
        nil // TODO: maintain nick -> iroh endpoint ID mapping
    }

    // MARK: - Channel helpers

    func getOrCreateChannel(_ name: String) -> ChannelState {
        let lower = name.lowercased()
        if let ch = channels.first(where: { $0.name.lowercased() == lower }) {
            return ch
        }
        let ch = ChannelState(name: name)
        channels.append(ch)
        channels.sort { $0.name.lowercased() < $1.name.lowercased() }
        return ch
    }

    func getOrCreateDM(_ nick: String) -> ChannelState {
        let lower = nick.lowercased()
        if let dm = dmBuffers.first(where: { $0.name.lowercased() == lower }) {
            return dm
        }
        let dm = ChannelState(name: nick)
        dmBuffers.append(dm)
        return dm
    }

    func switchToChannelByIndex(_ index: Int) {
        let all = allBuffers
        guard index < all.count else { return }
        activeChannel = all[index].name
    }

    func incrementUnread(_ channel: String) {
        guard channel.lowercased() != activeChannel?.lowercased() else { return }
        unreadCounts[channel.lowercased(), default: 0] += 1
    }

    func clearUnread(_ channel: String) {
        unreadCounts[channel.lowercased()] = 0
        mentionCounts[channel.lowercased()] = 0
        // Track read position
        let ch = channels.first(where: { $0.name.lowercased() == channel.lowercased() })
            ?? dmBuffers.first(where: { $0.name.lowercased() == channel.lowercased() })
        if let lastMsg = ch?.messages.last {
            lastReadMsgId[channel.lowercased()] = lastMsg.id
        }
    }

    func isNickOnline(_ nick: String) -> Bool {
        let lower = nick.lowercased()
        return channels.contains { ch in
            ch.members.contains { $0.nick.lowercased() == lower }
        }
    }

    func awayStatus(for nick: String) -> String? {
        let lower = nick.lowercased()
        for ch in channels {
            if let m = ch.members.first(where: { $0.nick.lowercased() == lower }) {
                return m.awayMsg
            }
        }
        return nil
    }

    // MARK: - Sounds

    enum SoundType { case mention, dm, connect, disconnect }

    func playSound(_ type: SoundType) {
        guard UserDefaults.standard.bool(forKey: "freeq.soundsEnabled") != false else { return }
        let name: String
        switch type {
        case .mention: name = "Ping"
        case .dm: name = "Tink"
        case .connect: name = "Pop"
        case .disconnect: name = "Basso"
        }
        NSSound(named: NSSound.Name(name))?.play()
    }

    func toggleFavorite(_ channel: String) {
        let key = channel.lowercased()
        if favorites.contains(key) { favorites.remove(key) } else { favorites.insert(key) }
        UserDefaults.standard.set(Array(favorites), forKey: "freeq.favorites")
    }

    func toggleMuted(_ channel: String) {
        let key = channel.lowercased()
        if mutedChannels.contains(key) { mutedChannels.remove(key) } else { mutedChannels.insert(key) }
        UserDefaults.standard.set(Array(mutedChannels), forKey: "freeq.muted")
    }

    func addBookmark(channel: String, msg: ChatMessage) {
        guard !bookmarks.contains(where: { $0.msgId == msg.id }) else { return }
        bookmarks.append(Bookmark(channel: channel, msgId: msg.id, from: msg.from, text: msg.text, timestamp: msg.timestamp))
        saveBookmarks()
    }

    func removeBookmark(msgId: String) {
        bookmarks.removeAll { $0.msgId == msgId }
        saveBookmarks()
    }

    private func saveBookmarks() {
        if let data = try? JSONEncoder().encode(bookmarks) {
            UserDefaults.standard.set(data, forKey: "freeq.bookmarks")
        }
    }

    /// Get the last message from self in the active channel (for edit-last).
    func lastOwnMessage(in target: String) -> ChatMessage? {
        let ch = channels.first { $0.name.lowercased() == target.lowercased() }
            ?? dmBuffers.first { $0.name.lowercased() == target.lowercased() }
        return ch?.messages.last { $0.from.lowercased() == nick.lowercased() && !$0.isDeleted }
    }

    // MARK: - WHOIS for DID discovery

    private var whoisedNicks: Set<String> = []
    private var whoisQueue: [String] = []
    private var whoisTimerActive = false

    /// Queue WHOIS for members we haven't checked yet (to discover DIDs).
    func whoisMembers(_ nicks: [String]) {
        for nick in nicks {
            let lower = nick.lowercased()
            guard !whoisedNicks.contains(lower), lower != self.nick.lowercased() else { continue }
            whoisedNicks.insert(lower)
            whoisQueue.append(nick)
        }
        startWhoisDrain()
    }

    /// Drain the WHOIS queue one at a time, 2 seconds apart.
    private func startWhoisDrain() {
        guard !whoisTimerActive, !whoisQueue.isEmpty else { return }
        whoisTimerActive = true
        drainNextWhois()
    }

    private func drainNextWhois() {
        guard !whoisQueue.isEmpty else {
            whoisTimerActive = false
            return
        }
        let nick = whoisQueue.removeFirst()
        sendWhois(nick)
        DispatchQueue.main.asyncAfter(deadline: .now() + 2.0) { [weak self] in
            self?.drainNextWhois()
        }
    }

    // MARK: - Notifications

    func sendNotification(title: String, body: String) {
        let content = UNMutableNotificationContent()
        content.title = title
        content.body = body
        content.sound = .default
        let request = UNNotificationRequest(identifier: UUID().uuidString, content: content, trigger: nil)
        UNUserNotificationCenter.current().add(request)
    }
}

// MARK: - IRC Event Handler

class AppEventHandler: EventHandler {
    private weak var appState: AppState?

    init(appState: AppState) {
        self.appState = appState
    }

    func onEvent(event: FreeqEvent) {
        DispatchQueue.main.async { [weak self] in
            guard let state = self?.appState else { return }
            state.handleEvent(event)
        }
    }
}

extension AppState {
    func handleEvent(_ event: FreeqEvent) {
        switch event {
        case .connected:
            connectionState = .connected

        case .registered(let registeredNick):
            Log.irc.info("Registered as \(registeredNick, privacy: .public)")
            connectionState = .registered
            reconnectAttempts = 0
            playSound(.connect)
            nick = registeredNick
            // Auto-join channels
            for ch in autoJoinChannels {
                joinChannel(ch)
            }
            // Request DM targets
            if authenticatedDID != nil {
                sendRaw("CHATHISTORY TARGETS * * 50")
            }
            // Start P2P subsystem
            startP2p()

        case .authenticated(let did):
            authenticatedDID = did
            KeychainHelper.save(key: "did", value: did)

        case .authFailed(let reason):
            errorMessage = "Auth failed: \(reason)"

        case .joined(let channel, let joinNick):
            if joinNick.lowercased() == nick.lowercased() {
                let ch = getOrCreateChannel(channel)
                ch.members.removeAll()
                pendingNames[channel.lowercased()] = []
                if activeChannel == nil || activeChannel == "server" {
                    activeChannel = ch.name
                }
                // Save to auto-join
                if !autoJoinChannels.contains(where: { $0.lowercased() == channel.lowercased() }) {
                    autoJoinChannels.append(channel)
                    UserDefaults.standard.set(autoJoinChannels, forKey: "freeq.channels")
                }
            } else {
                if let ch = channels.first(where: { $0.name.lowercased() == channel.lowercased() }) {
                    if !ch.members.contains(where: { $0.nick.lowercased() == joinNick.lowercased() }) {
                        ch.members.append(MemberInfo(nick: joinNick, isOp: false, isHalfop: false, isVoiced: false, awayMsg: nil, did: nil))
                    }
                    // System message
                    ch.appendIfNew(ChatMessage(
                        id: UUID().uuidString, from: "",
                        text: "\(joinNick) joined",
                        isAction: false, timestamp: Date(), replyTo: nil
                    ))
                }
            }

        case .parted(let channel, let partNick):
            if partNick.lowercased() == nick.lowercased() {
                channels.removeAll { $0.name.lowercased() == channel.lowercased() }
                autoJoinChannels.removeAll { $0.lowercased() == channel.lowercased() }
                UserDefaults.standard.set(autoJoinChannels, forKey: "freeq.channels")
                if activeChannel?.lowercased() == channel.lowercased() {
                    activeChannel = channels.first?.name
                }
            } else {
                if let ch = channels.first(where: { $0.name.lowercased() == channel.lowercased() }) {
                    ch.members.removeAll { $0.nick.lowercased() == partNick.lowercased() }
                    ch.appendIfNew(ChatMessage(
                        id: UUID().uuidString, from: "",
                        text: "\(partNick) left",
                        isAction: false, timestamp: Date(), replyTo: nil
                    ))
                }
            }

        case .message(let msg):
            let isSelf = msg.fromNick.lowercased() == nick.lowercased()
            let message = ChatMessage(
                id: msg.msgid ?? UUID().uuidString,
                from: msg.fromNick,
                text: msg.text,
                isAction: msg.isAction,
                timestamp: Date(timeIntervalSince1970: Double(msg.timestampMs) / 1000.0),
                replyTo: msg.replyTo,
                isEdited: msg.editOf != nil,
                isSigned: msg.isSigned
            )

            // Handle edits
            if let editOf = msg.editOf {
                if let batchId = msg.batchId, var batch = batches[batchId] {
                    if let idx = batch.messages.firstIndex(where: { $0.id == editOf }) {
                        batch.messages[idx].text = msg.text
                        batch.messages[idx].isEdited = true
                        if let newId = msg.msgid { batch.messages[idx].id = newId }
                    } else {
                        batch.messages.append(message)
                    }
                    batches[batchId] = batch
                    return
                }

                let target = msg.target
                if target.hasPrefix("#") {
                    let ch = getOrCreateChannel(target)
                    ch.applyEdit(originalId: editOf, newId: msg.msgid, newText: msg.text)
                } else {
                    let bufName = isSelf ? target : msg.fromNick
                    let dm = getOrCreateDM(bufName)
                    dm.applyEdit(originalId: editOf, newId: msg.msgid, newText: msg.text)
                }
                return
            }

            // Handle batch (CHATHISTORY)
            if let batchId = msg.batchId, var batch = batches[batchId] {
                batch.messages.append(message)
                batches[batchId] = batch
                return
            }

            // Route to channel or DM
            let target = msg.target
            if target.hasPrefix("#") {
                let ch = getOrCreateChannel(target)
                ch.appendIfNew(message)
                ch.typingUsers.removeValue(forKey: msg.fromNick)
                incrementUnread(target)

                // Mention notification
                if !isSelf && msg.text.localizedCaseInsensitiveContains(nick) {
                    mentionCounts[target.lowercased(), default: 0] += 1
                    if !mutedChannels.contains(target.lowercased()) {
                        sendNotification(title: "\(msg.fromNick) in \(target)", body: msg.text)
                        playSound(.mention)
                    }
                }
            } else {
                let bufName = isSelf ? target : msg.fromNick
                let dm = getOrCreateDM(bufName)
                dm.appendIfNew(message)
                incrementUnread(bufName)

                // DM notification
                if !isSelf {
                    sendNotification(title: msg.fromNick, body: msg.text)
                    playSound(.dm)
                }
            }

        case .tagMsg(let tagMsg):
            let tags = Dictionary(uniqueKeysWithValues: tagMsg.tags.map { ($0.key, $0.value) })
            let target = tagMsg.target
            let from = tagMsg.from

            // Typing indicators
            if let typing = tags["+typing"] {
                if from.lowercased() != nick.lowercased() {
                    let bufferName = target.hasPrefix("#") ? target : from
                    let ch = bufferName.hasPrefix("#") ? getOrCreateChannel(bufferName) : getOrCreateDM(bufferName)
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
                let ch = bufferName.hasPrefix("#") ? getOrCreateChannel(bufferName) : getOrCreateDM(bufferName)
                ch.applyDelete(msgId: deleteId)
            }

            // Reactions
            if let emoji = tags["+react"], let replyId = tags["+reply"] {
                let bufferName = target.hasPrefix("#") ? target : from
                let ch = bufferName.hasPrefix("#") ? getOrCreateChannel(bufferName) : getOrCreateDM(bufferName)
                ch.applyReaction(msgId: replyId, emoji: emoji, from: from)
            }

        case .names(let channel, let memberList):
            let key = channel.lowercased()
            var existing = pendingNames[key] ?? []
            existing.append(contentsOf: memberList.map { m in
                MemberInfo(nick: m.nick, isOp: m.isOp, isHalfop: m.isHalfop, isVoiced: m.isVoiced, awayMsg: m.awayMsg, did: nil)
            })
            pendingNames[key] = existing

        case .topicChanged(let channel, let topic):
            if let ch = channels.first(where: { $0.name.lowercased() == channel.lowercased() }) {
                ch.topic = topic.text
                ch.topicSetBy = topic.setBy
                ch.lastActivity = Date()
            }

        case .modeChanged(let channel, let mode, let arg, _):
            guard let targetNick = arg else { break }
            if let ch = channels.first(where: { $0.name.lowercased() == channel.lowercased() }),
               let idx = ch.members.firstIndex(where: { $0.nick.lowercased() == targetNick.lowercased() }) {
                let m = ch.members[idx]
                switch mode {
                case "+o": ch.members[idx] = MemberInfo(nick: m.nick, isOp: true, isHalfop: m.isHalfop, isVoiced: m.isVoiced, awayMsg: m.awayMsg, did: m.did)
                case "-o": ch.members[idx] = MemberInfo(nick: m.nick, isOp: false, isHalfop: m.isHalfop, isVoiced: m.isVoiced, awayMsg: m.awayMsg, did: m.did)
                case "+h": ch.members[idx] = MemberInfo(nick: m.nick, isOp: m.isOp, isHalfop: true, isVoiced: m.isVoiced, awayMsg: m.awayMsg, did: m.did)
                case "-h": ch.members[idx] = MemberInfo(nick: m.nick, isOp: m.isOp, isHalfop: false, isVoiced: m.isVoiced, awayMsg: m.awayMsg, did: m.did)
                case "+v": ch.members[idx] = MemberInfo(nick: m.nick, isOp: m.isOp, isHalfop: m.isHalfop, isVoiced: true, awayMsg: m.awayMsg, did: m.did)
                case "-v": ch.members[idx] = MemberInfo(nick: m.nick, isOp: m.isOp, isHalfop: m.isHalfop, isVoiced: false, awayMsg: m.awayMsg, did: m.did)
                default: break
                }
            }

        case .kicked(let channel, let kickedNick, let by, let reason):
            if kickedNick.lowercased() == nick.lowercased() {
                channels.removeAll { $0.name.lowercased() == channel.lowercased() }
                autoJoinChannels.removeAll { $0.lowercased() == channel.lowercased() }
                UserDefaults.standard.set(autoJoinChannels, forKey: "freeq.channels")
                if activeChannel?.lowercased() == channel.lowercased() {
                    activeChannel = channels.first?.name
                }
                errorMessage = "Kicked from \(channel) by \(by): \(reason)"
            } else {
                if let ch = channels.first(where: { $0.name.lowercased() == channel.lowercased() }) {
                    ch.members.removeAll { $0.nick.lowercased() == kickedNick.lowercased() }
                    ch.appendIfNew(ChatMessage(
                        id: UUID().uuidString, from: "",
                        text: "\(kickedNick) was kicked by \(by)\(reason.isEmpty ? "" : " (\(reason))")",
                        isAction: false, timestamp: Date(), replyTo: nil
                    ))
                }
            }

        case .nickChanged(let oldNick, let newNick):
            if oldNick.lowercased() == nick.lowercased() {
                nick = newNick
                UserDefaults.standard.set(newNick, forKey: "freeq.nick")
            }
            profileCache.renameUser(from: oldNick, to: newNick)
            for ch in allBuffers {
                if let idx = ch.members.firstIndex(where: { $0.nick.lowercased() == oldNick.lowercased() }) {
                    let old = ch.members[idx]
                    ch.members[idx] = MemberInfo(nick: newNick, isOp: old.isOp, isHalfop: old.isHalfop, isVoiced: old.isVoiced, awayMsg: old.awayMsg, did: old.did)
                }
            }

        case .awayChanged(let awayNick, let awayMsg):
            for ch in allBuffers {
                if let idx = ch.members.firstIndex(where: { $0.nick.lowercased() == awayNick.lowercased() }) {
                    let old = ch.members[idx]
                    ch.members[idx] = MemberInfo(nick: old.nick, isOp: old.isOp, isHalfop: old.isHalfop, isVoiced: old.isVoiced, awayMsg: awayMsg, did: old.did)
                }
            }

        case .userQuit(let quitNick, let reason):
            for ch in channels {
                if ch.members.contains(where: { $0.nick.lowercased() == quitNick.lowercased() }) {
                    ch.members.removeAll { $0.nick.lowercased() == quitNick.lowercased() }
                    ch.typingUsers.removeValue(forKey: quitNick)
                    ch.appendIfNew(ChatMessage(
                        id: UUID().uuidString, from: "",
                        text: "\(quitNick) quit\(reason.isEmpty ? "" : " (\(reason))")",
                        isAction: false, timestamp: Date(), replyTo: nil
                    ))
                }
            }

        case .batchStart(let id, _, let target):
            batches[id] = BatchBuffer(target: target, messages: [])

        case .batchEnd(let id):
            guard let batch = batches.removeValue(forKey: id) else { return }
            let target = batch.target
            // Case-insensitive: find existing channel/DM or create
            let ch: ChannelState
            if target.hasPrefix("#") {
                ch = channels.first(where: { $0.name.lowercased() == target.lowercased() }) ?? getOrCreateChannel(target)
            } else {
                ch = dmBuffers.first(where: { $0.name.lowercased() == target.lowercased() }) ?? getOrCreateDM(target)
            }
            for msg in batch.messages.sorted(by: { $0.timestamp < $1.timestamp }) {
                ch.appendIfNew(msg)
            }

        case .chatHistoryTarget(let targetNick, _):
            let _ = getOrCreateDM(targetNick)

        case .whoisReply(let whoisNick, let info):
            // Parse WHOIS for DID: "nick is authenticated as did:plc:xxx"
            // Or "nick is logged in as did:plc:xxx"
            if info.contains("authenticated as ") || info.contains("logged in as ") {
                let parts = info.split(separator: " ")
                if let did = parts.last, did.hasPrefix("did:") {
                    let didStr = String(did)
                    profileCache.setDid(didStr, for: whoisNick)
                    // Update member DID in all channels
                    for ch in channels {
                        if let idx = ch.members.firstIndex(where: { $0.nick.lowercased() == whoisNick.lowercased() }) {
                            let m = ch.members[idx]
                            if m.did == nil {
                                ch.members[idx] = MemberInfo(nick: m.nick, isOp: m.isOp, isHalfop: m.isHalfop, isVoiced: m.isVoiced, awayMsg: m.awayMsg, did: didStr)
                            }
                        }
                    }
                }
            }
            // Show WHOIS in active channel
            if let ch = activeChannelState {
                ch.appendIfNew(ChatMessage(
                    id: UUID().uuidString, from: "server", text: info,
                    isAction: false, timestamp: Date(), replyTo: nil
                ))
            }

        case .notice(let text):
            // MOTD handling
            if text == "MOTD:START" {
                motd = ""
                return
            }
            if text.hasPrefix("MOTD:") && text != "MOTD:START" && text != "MOTD:END" {
                motd += String(text.dropFirst("MOTD:".count)) + "\n"
                return
            }
            if text == "MOTD:END" {
                if !motd.isEmpty { showMotd = true }
                return
            }

            // NamesEnd signal — flush pending members and request history
            if text.hasPrefix("__NAMES_END__") {
                let channel = String(text.dropFirst("__NAMES_END__".count))
                let key = channel.lowercased()
                // Ensure channel exists before flushing
                let ch = getOrCreateChannel(channel)
                if let members = pendingNames.removeValue(forKey: key) {
                    ch.members = members
                    // WHOIS each member to discover DIDs (background, rate-limited)
                    whoisMembers(members.map(\.nick))
                }
                requestHistory(channel: channel)
                return
            }
            if text.isEmpty { return }
            if let ch = activeChannelState {
                ch.appendIfNew(ChatMessage(
                    id: UUID().uuidString,
                    from: "server",
                    text: text,
                    isAction: false,
                    timestamp: Date(),
                    replyTo: nil
                ))
            }

        case .disconnected(let reason):
            connectionState = .disconnected
            if !reason.contains("intentional") && hasSavedSession {
                reconnectAttempts += 1
                let delay = min(Double(1 << min(reconnectAttempts, 5)), 30.0)
                DispatchQueue.main.asyncAfter(deadline: .now() + delay) { [weak self] in
                    guard let self, self.connectionState == .disconnected, self.hasSavedSession else { return }
                    self.reconnectIfSaved()
                }
            }
        }
    }
}

// MARK: - P2P Event Handler

class AppP2pHandler: P2pEventHandler {
    private weak var appState: AppState?

    init(appState: AppState) {
        self.appState = appState
    }

    func onP2pEvent(event: P2pEvent) {
        DispatchQueue.main.async { [weak self] in
            guard let state = self?.appState else { return }
            state.handleP2pEvent(event)
        }
    }
}

extension AppState {
    func handleP2pEvent(_ event: P2pEvent) {
        switch event {
        case .endpointReady(let endpointId):
            p2pEndpointId = endpointId

        case .peerConnected(let peerId):
            p2pConnectedPeers.insert(peerId)

        case .peerDisconnected(let peerId):
            p2pConnectedPeers.remove(peerId)

        case .directMessage(let peerId, let text):
            let short = String(peerId.prefix(8))
            let dm = getOrCreateDM("p2p:\(short)")
            dm.appendIfNew(ChatMessage(
                id: UUID().uuidString,
                from: short,
                text: text,
                isAction: false,
                timestamp: Date(),
                replyTo: nil
            ))
            incrementUnread("p2p:\(short)")

        case .error(let message):
            errorMessage = "P2P: \(message)"
        }
    }
}
