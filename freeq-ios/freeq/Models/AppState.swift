import ActivityKit
import CoreSpotlight
import Foundation
import os.log
import SwiftUI

/// Auth-path diagnostic log. Visible in Console.app attached to the device.
/// We log credential-clearing events with the decision inputs so we can tell
/// after the fact whether a re-OAuth was justified or whether the broker
/// flapped past the 3-strike threshold for transient reasons.
private let authLog = Logger(subsystem: "at.freeq.ios", category: "auth")

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
    var isSigned: Bool = false
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
    @Published var pins: Set<String> = []  // pinned message IDs
    /// Tracks the most recent activity (message, join, topic change, etc.)
    var lastActivity: Date = Date()

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

    func memberInfo(for nick: String) -> MemberInfo? {
        members.first(where: { $0.nick.lowercased() == nick.lowercased() })
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
        // Update last activity for sorting
        if msg.timestamp > lastActivity {
            lastActivity = msg.timestamp
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

    func removeReaction(msgId: String, emoji: String, from: String) {
        guard let idx = findMessage(byId: msgId) else { return }
        var reactions = messages[idx].reactions
        guard var nicks = reactions[emoji] else { return }
        nicks.remove(from)
        if nicks.isEmpty {
            reactions.removeValue(forKey: emoji)
        } else {
            reactions[emoji] = nicks
        }
        messages[idx].reactions = reactions
    }
}

/// Member info for the member list.
struct MemberInfo: Identifiable, Equatable {
    let nick: String
    let isOp: Bool
    let isHalfop: Bool
    let isVoiced: Bool
    let awayMsg: String?
    let did: String?

    var id: String { nick.lowercased() }

    var prefix: String {
        if isOp { return "@" }
        if isHalfop { return "%" }
        if isVoiced { return "+" }
        return ""
    }

    var isAway: Bool { awayMsg != nil }
    var isVerified: Bool { did != nil }
}

/// Connection state.
enum ConnectionState: Equatable {
    case disconnected
    case connecting
    case connected
    case registered
}

/// Main application state — bridges the Rust SDK to SwiftUI.
class AppState: ObservableObject {
    private static let minimumPersistentSessionDuration: TimeInterval = 14 * 24 * 60 * 60  // 14 days
    struct BatchBuffer {
        let target: String
        var messages: [ChatMessage]
    }

    @Published var connectionState: ConnectionState = .disconnected
    @Published var nick: String = ""
    @Published var serverAddress: String = ServerConfig.ircServer
    // For deployments using embedded auth (no standalone broker), use ServerConfig.apiBaseUrl:
    // @Published var authBrokerBase: String = ServerConfig.apiBaseUrl
    @Published var authBrokerBase: String = "https://auth.freeq.at"
    @Published var channels: [ChannelState] = []
    @Published var activeChannel: String? = nil
    @Published var errorMessage: String? = nil
    @Published var authenticatedDID: String? = nil
    @Published var dmBuffers: [ChannelState] = []
    @Published var autoJoinChannels: [String] = ["#general"]
    @Published var unreadCounts: [String: Int] = [:] {
        didSet { UserDefaults.standard.set(unreadCounts, forKey: "freeq.unreadCounts") }
    }

    /// Muted channels — no notifications, no badge increment
    @Published var mutedChannels: Set<String> = [] {
        didSet { UserDefaults.standard.set(Array(mutedChannels), forKey: "freeq.mutedChannels") }
    }

    /// MOTD lines collected from server
    @Published var motdLines: [String] = []
    @Published var showMotd: Bool = false
    fileprivate var collectingMotd: Bool = false

    // In-flight CHATHISTORY batches
    fileprivate var batches: [String: BatchBuffer] = [:]

    /// Pending DM navigation — set by profile "Message" button, consumed by ChatsTab
    @Published var pendingDMNick: String? = nil

    // ── AV (voice/video calls) ──
    @Published var isInCall: Bool = false
    @Published var isMuted: Bool = false
    @Published var isCameraOn: Bool = false
    @Published var callParticipants: [String] = []
    /// channel (lowercased) → active session id, populated from `+freeq.at/av-state` TAGMSGs
    @Published var activeAvSessions: [String: String] = [:]
    /// Channel + session id of the call we're currently in (if any).
    @Published var currentCallChannel: String? = nil
    @Published var currentCallSessionId: String? = nil
    var currentNick: String? { client != nil ? nick : nil }
    fileprivate var avSession: FreeqAv? = nil
    /// Channels where we sent `av-start` and are waiting on the server's `started` echo.
    fileprivate var pendingAvStart: Set<String> = []
    /// Live Activity tracking the in-call state. Drives the Dynamic Island.
    fileprivate var callActivity: Activity<CallActivityAttributes>? = nil

    func startCall(channel: String, sessionId: String) {
        guard client != nil else { return }
        // Use HTTPS API base — MoQ SFU lives behind the same reverse proxy.
        let serverUrl = ServerConfig.apiBaseUrl

        do {
            avSession = try FreeqAv(
                serverUrl: serverUrl,
                sessionId: sessionId,
                nick: nick,
                handler: AvCallbackHandler(appState: self)
            )
            DispatchQueue.main.async {
                self.isInCall = true
                self.currentCallChannel = channel
                self.currentCallSessionId = sessionId
                self.startCallActivity(channel: channel, sessionId: sessionId)
            }
            // Tell peers we joined this session.
            try? client?.sendRaw(line: "@+freeq.at/av-join;+freeq.at/av-id=\(sessionId) TAGMSG \(channel)")
        } catch {
            print("[av] Failed to start call: \(error)")
        }
    }

    func leaveCall() {
        // Send av-leave for the channel we're currently in, if any.
        if let channel = currentCallChannel, let sessionId = currentCallSessionId {
            try? client?.sendRaw(line: "@+freeq.at/av-leave;+freeq.at/av-id=\(sessionId) TAGMSG \(channel)")
        }
        avSession?.leave()
        avSession = nil
        DispatchQueue.main.async {
            self.isInCall = false
            self.isMuted = false
            self.isCameraOn = false
            self.callParticipants = []
            self.currentCallChannel = nil
            self.currentCallSessionId = nil
            self.endCallActivity()
        }
    }

    func toggleMute() {
        isMuted.toggle()
        avSession?.setMuted(muted: isMuted)
        updateCallActivity()
    }

    // MARK: - Live Activity (Dynamic Island)

    /// Start the in-call Live Activity. The Dynamic Island will show the
    /// channel + duration + participant count + mute state until `endCallActivity`.
    fileprivate func startCallActivity(channel: String, sessionId: String) {
        // Make sure no stale activity from a prior call is still alive.
        endCallActivity()
        guard ActivityAuthorizationInfo().areActivitiesEnabled else {
            // User has Live Activities disabled at the OS level.
            return
        }
        let attrs = CallActivityAttributes(channel: channel, sessionId: sessionId)
        let state = CallActivityAttributes.ContentState(
            participantCount: max(callParticipants.count, 1),
            isMuted: isMuted,
            startedAt: Date()
        )
        do {
            callActivity = try Activity<CallActivityAttributes>.request(
                attributes: attrs,
                content: .init(state: state, staleDate: nil),
                pushType: nil
            )
        } catch {
            print("[av] Failed to start Live Activity: \(error)")
        }
    }

    /// Push the current participant / mute state to the Live Activity.
    fileprivate func updateCallActivity() {
        guard let activity = callActivity else { return }
        let started = activity.content.state.startedAt
        let new = CallActivityAttributes.ContentState(
            participantCount: max(callParticipants.count, 1),
            isMuted: isMuted,
            startedAt: started
        )
        Task {
            await activity.update(.init(state: new, staleDate: nil))
        }
    }

    /// End the Live Activity. Called from `leaveCall` and on AV disconnect.
    fileprivate func endCallActivity() {
        guard let activity = callActivity else { return }
        callActivity = nil
        Task {
            await activity.end(nil, dismissalPolicy: .immediate)
        }
    }

    func toggleCamera() {
        isCameraOn.toggle()
    }

    /// Start or join a voice session on a channel.
    /// - If a session is already known to be active, joins it directly.
    /// - Otherwise sends `av-start` and waits for the server's `+freeq.at/av-state=started`
    ///   TAGMSG to learn the session id (handled in the inbound TAGMSG path).
    func startOrJoinVoice(channel: String) {
        guard !isInCall else { return }

        if let sessionId = activeAvSessions[channel.lowercased()] {
            startCall(channel: channel, sessionId: sessionId)
            return
        }

        pendingAvStart.insert(channel.lowercased())
        do {
            try client?.sendRaw(line: "@+freeq.at/av-start TAGMSG \(channel)")
        } catch {
            print("[av] Failed to send av-start: \(error)")
            pendingAvStart.remove(channel.lowercased())
        }
    }

    /// For reply UI
    @Published var replyingTo: ChatMessage? = nil
    /// For edit UI
    @Published var editingMessage: ChatMessage? = nil
    /// Image lightbox
    @Published var lightboxURL: URL? = nil
    /// Pending web-token for SASL auth (from AT Protocol OAuth)
    var pendingWebToken: String? = nil
    /// Persistent broker session token
    var brokerToken: String? = nil
    /// Cached web-token + expiry (reuse across reconnects within TTL)
    fileprivate var cachedWebToken: String? = nil
    fileprivate var cachedWebTokenExpiry: Date = .distantPast

    /// Read position tracking — channel name -> last read message ID
    @Published var lastReadMessageIds: [String: String] = [:]

    /// Theme
    @Published var isDarkTheme: Bool = true

    private var client: FreeqClient? = nil
    private var typingTimer: Timer? = nil
    private var lastTypingSent: Date = .distantPast
    fileprivate var reconnectAttempts: Int = 0

    var activeChannelState: ChannelState? {
        if let name = activeChannel {
            return channels.first { $0.name == name } ?? dmBuffers.first { $0.name == name }
        }
        return nil
    }

    /// Whether we have a saved session that should auto-reconnect.
    /// True if we have a broker token — the durable, long-lived credential.
    /// No expiry window: the broker token is valid until the PDS revokes the
    /// underlying refresh token (typically 90+ days of inactivity).
    var hasSavedSession: Bool {
        // Broker token is the only signal — it's the long-lived credential.
        // Nick might be empty on first launch after migration; the broker
        // session response will provide the correct nick.
        return brokerToken != nil
    }

    /// Singleton handle for App Intents / Spotlight handoff. Set by `init`.
    /// App Intents can't easily inject the SwiftUI environment, so we expose
    /// the live instance here. Always read on main.
    static weak var shared: AppState? = nil

    init() {
        AppState.shared = self
        if let savedNick = UserDefaults.standard.string(forKey: "freeq.nick") {
            nick = savedNick
        }
        // Always boot against the production server defined in ServerConfig.
        // Any legacy `freeq.server` value (from earlier staging builds) is
        // discarded so existing installs don't keep talking to the wrong host.
        serverAddress = ServerConfig.ircServer
        UserDefaults.standard.removeObject(forKey: "freeq.server")
        if let savedChannels = UserDefaults.standard.stringArray(forKey: "freeq.channels") {
            // Drop anything that isn't a channel-prefixed name. Older builds
            // could land bare nicks in here (the @yokota-as-channel bug); we
            // never want to send `JOIN` for those.
            let cleaned = savedChannels.filter { $0.hasPrefix("#") || $0.hasPrefix("&") }
            autoJoinChannels = cleaned
            if cleaned.count != savedChannels.count {
                UserDefaults.standard.set(cleaned, forKey: "freeq.channels")
            }
        }
        if let savedReadPositions = UserDefaults.standard.dictionary(forKey: "freeq.readPositions") as? [String: String] {
            lastReadMessageIds = savedReadPositions
        }
        if let savedUnreads = UserDefaults.standard.dictionary(forKey: "freeq.unreadCounts") as? [String: Int] {
            unreadCounts = savedUnreads
        }
        if let savedMuted = UserDefaults.standard.stringArray(forKey: "freeq.mutedChannels") {
            mutedChannels = Set(savedMuted)
        }
        // Migrate secrets from UserDefaults to Keychain (one-time)
        KeychainHelper.migrateFromUserDefaults(userDefaultsKey: "freeq.did", keychainKey: "did")
        KeychainHelper.migrateFromUserDefaults(userDefaultsKey: "freeq.brokerToken", keychainKey: "brokerToken")

        if let savedDID = KeychainHelper.load(key: "did") {
            authenticatedDID = savedDID
        }
        if let savedBroker = KeychainHelper.load(key: "brokerToken") {
            brokerToken = savedBroker
        }
        if let savedBrokerBase = UserDefaults.standard.string(forKey: "freeq.brokerBase") {
            authBrokerBase = savedBrokerBase
        }
        // Restore cached web token if still valid
        if let savedToken = KeychainHelper.load(key: "webToken"),
           let expiryStr = UserDefaults.standard.string(forKey: "freeq.webTokenExpiry"),
           let expiryTs = Double(expiryStr) {
            let expiry = Date(timeIntervalSince1970: expiryTs)
            if Date() < expiry {
                cachedWebToken = savedToken
                cachedWebTokenExpiry = expiry
            } else {
                KeychainHelper.delete(key: "webToken")
            }
        }
        isDarkTheme = UserDefaults.standard.object(forKey: "freeq.darkTheme") as? Bool ?? true

        // Prune stale typing indicators every 3 seconds
        Timer.scheduledTimer(withTimeInterval: 3, repeats: true) { [weak self] _ in
            DispatchQueue.main.async {
                self?.pruneTypingIndicators()
            }
        }
    }

    /// Reconnect with saved session (requires SASL web-token).
    /// Retries broker fetch with backoff on failure.
    fileprivate var brokerRetryCount = 0

    /// Increments at the start of each `reconnectSavedSession()` invocation.
    /// `ContentView` watches this to reset its "Connecting…" timer per
    /// attempt instead of running it continuously from first appearance.
    @Published var reconnectAttempt: Int = 0

    func reconnectSavedSession() {
        guard hasSavedSession, connectionState == .disconnected else { return }
        reconnectAttempt &+= 1

        // 1. Already have a pending token (e.g., from initial login)
        if pendingWebToken != nil && !nick.isEmpty {
            connect(nick: nick)
            return
        }

        // 2. Reuse cached web-token if still valid (25 min window — token TTL is 30 min)
        if let cached = cachedWebToken, Date() < cachedWebTokenExpiry, !nick.isEmpty {
            pendingWebToken = cached
            connect(nick: nick)
            return
        }

        // 3. Fetch a fresh web-token from broker
        guard let brokerToken else {
            // No broker token at all — must log in fresh
            return
        }
        Task {
            do {
                let session = try await fetchBrokerSession(brokerToken: brokerToken)
                await MainActor.run {
                    self.brokerRetryCount = 0
                    self.pendingWebToken = session.token
                    // Cache for reuse (25 min — conservative vs 30 min server TTL)
                    self.cachedWebToken = session.token
                    let expiry = Date().addingTimeInterval(25 * 60)
                    self.cachedWebTokenExpiry = expiry
                    KeychainHelper.save(key: "webToken", value: session.token)
                    UserDefaults.standard.set(String(expiry.timeIntervalSince1970), forKey: "freeq.webTokenExpiry")
                    self.authenticatedDID = session.did
                    KeychainHelper.save(key: "did", value: session.did)
                    self.connect(nick: session.nick)
                }
            } catch let error as NSError {
                await MainActor.run {
                    // If broker token was cleared (genuinely expired), stop retrying
                    if error.code == 401 && self.brokerToken == nil {
                        // Credentials cleared — show login screen
                        return
                    }

                    self.brokerRetryCount += 1
                    // Keep retrying indefinitely with capped backoff (max 60s)
                    // The user will see "Connecting..." and can cancel after 15s
                    let delay: Double
                    if self.brokerRetryCount <= 3 {
                        delay = Double(self.brokerRetryCount) // 1, 2, 3s
                    } else if self.brokerRetryCount <= 10 {
                        delay = min(Double(self.brokerRetryCount * 2), 20.0) // 8..20s
                    } else {
                        delay = 60.0 // After 10 failures, try once per minute
                    }
                    DispatchQueue.main.asyncAfter(deadline: .now() + delay) {
                        if self.connectionState == .disconnected && self.hasSavedSession {
                            self.reconnectSavedSession()
                        }
                    }
                    // Don't set errorMessage — let it keep trying silently
                }
            }
        }
    }

    /// Tracks whether the current session has already fallen back from
    /// WebSocket to TCP. We only allow one fallback per `connect(nick:)`
    /// call to avoid an infinite loop if both transports fail.
    fileprivate var transportFallbackUsed = false

    func connect(nick: String) {
        // Fresh connect attempt — start by preferring WebSocket again.
        transportFallbackUsed = false
        connect(nick: nick, useWebSocket: true)
    }

    fileprivate func connect(nick: String, useWebSocket: Bool) {
        self.nick = nick
        self.connectionState = .connecting
        self.errorMessage = nil

        UserDefaults.standard.set(nick, forKey: "freeq.nick")
        UserDefaults.standard.set(serverAddress, forKey: "freeq.server")
        UserDefaults.standard.set(authBrokerBase, forKey: "freeq.brokerBase")

        do {
            let handler = SwiftEventHandler(appState: self)
            client = try FreeqClient(
                server: serverAddress,
                nick: nick,
                handler: handler
            )

            // Prefer WebSocket on port 443. Pass an empty string to disable
            // and use the configured TCP server when falling back.
            let wsUrl = useWebSocket ? ServerConfig.wssServer : ""
            try client?.setWebsocketUrl(url: wsUrl)
            print("[freeq.connect] transport=\(useWebSocket ? "ws" : "tcp") wsUrl=\(wsUrl) tcp=\(serverAddress) nick=\(nick) hasToken=\(pendingWebToken != nil)")
            authLog.info("connect transport=\(useWebSocket ? "ws" : "tcp", privacy: .public) ws_url=\(wsUrl, privacy: .public)")

            // Set web-token for SASL auth if available (from AT Protocol OAuth)
            if let token = pendingWebToken {
                try client?.setWebToken(token: token)
                pendingWebToken = nil
            }

            try client?.connect()
            print("[freeq.connect] client?.connect() returned")
        } catch {
            print("[freeq.connect] threw error: \(error)")
            DispatchQueue.main.async {
                self.connectionState = .disconnected
                self.errorMessage = "Connection failed: \(error)"
            }
        }
    }

    /// Fall back from WebSocket to plain TCP if a WS connect fails — but only
    /// once per `connect(nick:)` call. Triggered by `Event.Disconnected` whose
    /// reason starts with "WebSocket".
    fileprivate func attemptTransportFallback(reason: String) -> Bool {
        guard !transportFallbackUsed,
              reason.lowercased().contains("websocket"),
              hasSavedSession,
              !nick.isEmpty else {
            return false
        }
        transportFallbackUsed = true
        authLog.notice("WS connect failed; falling back to TCP. reason=\(reason, privacy: .public)")
        DispatchQueue.main.async {
            // Tear the (failed) client down cleanly before re-issuing connect.
            self.client?.disconnect()
            self.client = nil
            self.connect(nick: self.nick, useWebSocket: false)
        }
        return true
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
        KeychainHelper.delete(key: "did")
        KeychainHelper.delete(key: "brokerToken")
        KeychainHelper.delete(key: "webToken")
        UserDefaults.standard.removeObject(forKey: "freeq.webTokenExpiry")
        UserDefaults.standard.removeObject(forKey: "freeq.nick")
        UserDefaults.standard.removeObject(forKey: "freeq.handle")
        cachedWebToken = nil
        cachedWebTokenExpiry = .distantPast
        SpotlightIndexer.clear()
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
        // Optimistically remove from UI — don't wait for server confirmation
        channels.removeAll { $0.name.lowercased() == channel.lowercased() }
        autoJoinChannels.removeAll { $0.lowercased() == channel.lowercased() }
        UserDefaults.standard.set(autoJoinChannels, forKey: "freeq.channels")
        if activeChannel?.lowercased() == channel.lowercased() {
            activeChannel = channels.first?.name
        }
    }

    /// Send a message. Returns true on success, false on failure (so caller can preserve text).
    @discardableResult
    func sendMessage(target: String, text: String) -> Bool {
        guard !text.isEmpty else { return false }
        // Clear typing indicator for remote users
        sendRaw("@+typing=done TAGMSG \(target)")
        lastTypingSent = .distantPast

        let isMultiline = text.contains("\n")
        let wireText = text.replacingOccurrences(of: "\r", with: "")
        let encoded = isMultiline ? wireText.replacingOccurrences(of: "\n", with: "\\n") : wireText
        let multilineTag = isMultiline ? ";+freeq.at/multiline" : ""

        // Check for edit mode
        if let editing = editingMessage {
            sendRaw("@+draft/edit=\(editing.id)\(multilineTag) PRIVMSG \(target) :\(encoded)")
            editingMessage = nil
            return true
        }

        // Check for reply mode
        if let reply = replyingTo {
            sendRaw("@+reply=\(reply.id)\(multilineTag) PRIVMSG \(target) :\(encoded)")
            replyingTo = nil
            return true
        }

        if isMultiline {
            // Send via raw to include multiline tag
            sendRaw("@+freeq.at/multiline PRIVMSG \(target) :\(encoded)")
            return true
        }

        do {
            try client?.sendMessage(target: target, text: text)
            return true
        } catch {
            DispatchQueue.main.async { self.errorMessage = "Send failed" }
            return false
        }
    }

    func sendRaw(_ line: String) {
        guard let client = client else {
            print("❌ sendRaw: NO CLIENT")
            return
        }
        do {
            try client.sendRaw(line: line)
            print("✅ sendRaw OK: \(line.prefix(50))")
        } catch {
            print("❌ sendRaw ERROR: \(error)")
        }
    }

    /// Resolve a wire target (channel `#…`/`&…` or peer nick) to its local
    /// buffer for optimistic-send updates. Mirrors the inbound routing logic.
    private func bufferForSend(_ target: String) -> ChannelState {
        if target.hasPrefix("#") || target.hasPrefix("&") {
            return getOrCreateChannel(target)
        } else {
            return getOrCreateDM(target)
        }
    }

    func sendReaction(target: String, msgId: String, emoji: String) {
        // Optimistic local update — keeps the UI consistent even if the
        // `echo-message` cap isn't negotiated on the current connection.
        // `applyReaction` is idempotent (Set keyed by nick), so the inbound
        // echo (if any) is a no-op.
        bufferForSend(target).applyReaction(msgId: msgId, emoji: emoji, from: nick)
        sendRaw("@+react=\(emoji);+reply=\(msgId) TAGMSG \(target)")
    }

    func sendUnreaction(target: String, msgId: String, emoji: String) {
        bufferForSend(target).removeReaction(msgId: msgId, emoji: emoji, from: nick)
        sendRaw("@+freeq.at/unreact=\(emoji);+reply=\(msgId) TAGMSG \(target)")
    }

    /// Toggle the current user's reaction on a message: react if absent, unreact if present.
    func toggleReaction(target: String, msgId: String, emoji: String, currentlyMine: Bool) {
        if currentlyMine {
            sendUnreaction(target: target, msgId: msgId, emoji: emoji)
        } else {
            sendReaction(target: target, msgId: msgId, emoji: emoji)
        }
    }

    func deleteMessage(target: String, msgId: String) {
        bufferForSend(target).applyDelete(msgId: msgId)
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

    func fetchPins(channel: String) {
        let name = channel.hasPrefix("#") ? String(channel.dropFirst()) : channel
        guard let encoded = name.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed),
              let url = URL(string: "\(ServerConfig.apiBaseUrl)/api/v1/channels/\(encoded)/pins") else { return }
        Task {
            do {
                let (data, _) = try await URLSession.shared.data(from: url)
                if let json = try JSONSerialization.jsonObject(with: data) as? [String: Any],
                   let pinsArray = json["pins"] as? [[String: Any]] {
                    let msgIds = Set(pinsArray.compactMap { $0["msgid"] as? String })
                    await MainActor.run {
                        if let ch = self.channels.first(where: { $0.name.lowercased() == channel.lowercased() }) {
                            ch.pins = msgIds
                        }
                    }
                }
            } catch { /* network error */ }
        }
    }

    struct BrokerSessionResponse: Decodable {
        let token: String
        let nick: String
        let did: String
        let handle: String
    }

    /// Track consecutive 401s — only clear broker token after multiple failures
    private var consecutive401Count = 0
    private var lastLoginDate: Date? {
        let ts = UserDefaults.standard.double(forKey: "freeq.lastLogin")
        guard ts > 0 else { return nil }
        return Date(timeIntervalSince1970: ts)
    }

    /// Keep users logged in for at least two weeks unless they explicitly log out.
    /// During this window, never clear broker credentials automatically.
    private var canAutoClearBrokerCredentials: Bool {
        guard let lastLoginDate else { return false }
        return Date().timeIntervalSince(lastLoginDate) >= Self.minimumPersistentSessionDuration
    }

    private func fetchBrokerSession(brokerToken: String) async throws -> BrokerSessionResponse {
        // Retry up to 4 times with backoff — DPoP nonce rotation and transient errors
        for attempt in 0..<4 {
            let url = URL(string: "\(authBrokerBase)/session")!
            var request = URLRequest(url: url)
            request.httpMethod = "POST"
            request.setValue("application/json", forHTTPHeaderField: "Content-Type")
            request.httpBody = try JSONSerialization.data(withJSONObject: ["broker_token": brokerToken])

            let data: Data
            let response: URLResponse
            do {
                (data, response) = try await URLSession.shared.data(for: request)
            } catch {
                // Network error (offline, timeout, DNS) — don't clear anything, just throw
                if attempt < 3 {
                    try? await Task.sleep(nanoseconds: UInt64(1_000_000_000 * (attempt + 1)))
                    continue
                }
                throw error
            }

            let status = (response as? HTTPURLResponse)?.statusCode ?? 0

            // Transient gateway / proxy errors: NEVER count as 401 evidence.
            // The 502/503/504 family is usually broker-restart / proxy-flap
            // noise — but on the broker, 502 *also* covers "we tried to
            // refresh your access token at the PDS and that returned an
            // error". If the PDS error body is `invalid_grant` (or any
            // refresh-token-fatal variant), retrying forever is wrong; the
            // user needs to re-OAuth. Read the body and discriminate.
            if (status == 502 || status == 503 || status == 504) {
                let body = String(data: data, encoding: .utf8) ?? ""
                let bodySnippet = String(body.prefix(200))
                let isRefreshFatal = body.contains("invalid_grant")
                    || body.contains("invalid_token")
                    || body.contains("expired")
                    || body.contains("revoked")
                authLog.notice("broker \(status, privacy: .public) attempt=\(attempt, privacy: .public) body=\(bodySnippet, privacy: .public)")
                print("[freeq.broker] \(status) attempt=\(attempt) fatal=\(isRefreshFatal) body=\(bodySnippet)")
                if isRefreshFatal {
                    // The PDS told the broker that this user's refresh token is
                    // dead. No amount of retrying recovers from this — the user
                    // genuinely needs to re-OAuth. Clear credentials immediately
                    // (this is "I logged out at the PDS" semantics, not a flap).
                    authLog.error("Clearing broker credentials: PDS refresh fatal (body=\(bodySnippet, privacy: .public))")
                    await MainActor.run {
                        self.brokerToken = nil
                        self.cachedWebToken = nil
                        self.cachedWebTokenExpiry = .distantPast
                        KeychainHelper.delete(key: "brokerToken")
                        KeychainHelper.delete(key: "webToken")
                        UserDefaults.standard.removeObject(forKey: "freeq.webTokenExpiry")
                    }
                    throw NSError(domain: "Broker", code: 401, userInfo: [NSLocalizedDescriptionKey: "Session expired (PDS refused refresh) — please sign in again"])
                }
                if attempt < 3 {
                    try? await Task.sleep(nanoseconds: UInt64(500_000_000 * (attempt + 1)))
                    continue
                }
                // Out of inner retries with non-fatal 5xx. Fall through to throw
                // so the outer reconnect loop applies its backoff — but DON'T
                // touch credentials.
                throw NSError(domain: "Broker", code: status, userInfo: [NSLocalizedDescriptionKey: "Broker temporarily unavailable"])
            }

            // 401 = broker token might be invalid, but could also be transient
            // (e.g., broker DB was just recreated). Two clear paths:
            //   - past the 14-day grace window: clear after 3 consecutive 401s
            //   - within grace but persistent (>= 8 across reconnect cycles):
            //     clear regardless. This covers broker DB wipes / persistent
            //     storage rotations where every single retry is doomed and
            //     the grace window would otherwise hold the user hostage in
            //     "Connecting…" forever.
            if status == 401 {
                await MainActor.run { self.consecutive401Count += 1 }
                if attempt < 3 {
                    // Retry — the broker might recover (e.g., DB migration, restart)
                    try? await Task.sleep(nanoseconds: UInt64(1_000_000_000 * (attempt + 1)))
                    continue
                }
                let count = await MainActor.run { self.consecutive401Count }
                let lastLogin = await MainActor.run { self.lastLoginDate }
                let withinGrace = !canAutoClearBrokerCredentials
                let escalated = count >= 8
                let shouldClear = (count >= 3 && !withinGrace) || escalated
                if shouldClear {
                    // Genuinely invalid — clear credentials.
                    let sinceLoginHours = lastLogin.map { Date().timeIntervalSince($0) / 3600 } ?? -1
                    authLog.error(
                        "Clearing broker credentials: consecutive401=\(count, privacy: .public) sinceLoginHours=\(sinceLoginHours, privacy: .public) lastStatus=401 escalated=\(escalated, privacy: .public)"
                    )
                    await MainActor.run {
                        self.brokerToken = nil
                        self.cachedWebToken = nil
                        self.cachedWebTokenExpiry = .distantPast
                        KeychainHelper.delete(key: "brokerToken")
                        KeychainHelper.delete(key: "webToken")
                        UserDefaults.standard.removeObject(forKey: "freeq.webTokenExpiry")
                    }
                } else {
                    authLog.notice(
                        "Broker 401 NOT clearing creds: consecutive401=\(count, privacy: .public) withinGraceWindow=\(withinGrace, privacy: .public)"
                    )
                }
                throw NSError(domain: "Broker", code: 401, userInfo: [NSLocalizedDescriptionKey: "Session expired — please sign in again"])
            }
            guard status == 200 else { throw NSError(domain: "Broker", code: status) }
            // Success — reset 401 counter.
            await MainActor.run { self.consecutive401Count = 0 }
            return try JSONDecoder().decode(BrokerSessionResponse.self, from: data)
        }
        throw NSError(domain: "Broker", code: 502)
    }

    func markRead(_ channel: String) {
        unreadCounts[channel] = 0
        updateBadgeCount()
        // Persist last-read message ID
        if let state = channels.first(where: { $0.name == channel }) ?? dmBuffers.first(where: { $0.name == channel }),
           let lastMsg = state.messages.last {
            lastReadMessageIds[channel] = lastMsg.id
            UserDefaults.standard.set(lastReadMessageIds, forKey: "freeq.readPositions")
        }
    }

    func updateBadgeCount() {
        let total = unreadCounts.filter { !mutedChannels.contains($0.key) }.values.reduce(0, +)
        UNUserNotificationCenter.current().setBadgeCount(total)
    }

    func toggleMute(_ channel: String) {
        if mutedChannels.contains(channel) {
            mutedChannels.remove(channel)
        } else {
            mutedChannels.insert(channel)
        }
    }

    func isMuted(_ channel: String) -> Bool {
        mutedChannels.contains(channel)
    }

    func toggleTheme() {
        isDarkTheme.toggle()
        UserDefaults.standard.set(isDarkTheme, forKey: "freeq.darkTheme")
    }

    /// Called when the app transitions between foreground/background.
    func handleScenePhase(_ phase: ScenePhase) {
        switch phase {
        case .active:
            // Returning to foreground — reconnect if needed.
            NotificationManager.shared.clearBadge()
            // FEAT-004: skip the broker round-trip when the SDK still has a
            // live transport. Backgrounded apps with healthy WebSocket /
            // TCP keep their connection across short pauses; tearing it
            // down to re-fetch a web token burns broker requests for
            // nothing — and on a slow broker, lights up the user-visible
            // reconnect UI for what was a working connection.
            if let c = client, c.isConnected() {
                return
            }
            if connectionState == .disconnected && hasSavedSession {
                brokerRetryCount = 0  // Reset retries on foreground
                reconnectSavedSession()
            }
        case .background:
            // Going to background — nothing for now (WebSocket dies naturally)
            break
        case .inactive:
            break
        @unknown default:
            break
        }
    }

    func incrementUnread(_ channel: String) {
        guard activeChannel != channel else { return }
        guard !mutedChannels.contains(channel) else { return }
        unreadCounts[channel, default: 0] += 1
        updateBadgeCount()
    }

    /// IRC channel names must start with `#` (federated) or `&` (local-only).
    /// Anything else is a peer nick and belongs in `dmBuffers`, not here.
    /// We route mis-typed callers automatically so a stray event from the
    /// wire (or a future code path) can't pollute the Channels pane —
    /// that's how `@yokota` ended up showing as a channel.
    func getOrCreateChannel(_ name: String) -> ChannelState {
        let trimmed = name.trimmingCharacters(in: .whitespaces)
        guard trimmed.hasPrefix("#") || trimmed.hasPrefix("&") else {
            return getOrCreateDM(trimmed)
        }
        if let existing = channels.first(where: { $0.name.lowercased() == trimmed.lowercased() }) {
            return existing
        }
        let channel = ChannelState(name: trimmed)
        channels.append(channel)
        SpotlightIndexer.reindex(self)
        return channel
    }

    /// DMs are keyed by peer nick. Refuse anything that looks like a channel —
    /// a `#`/`&` name in `dmBuffers` would render with the DM avatar/style and
    /// silently shadow the real channel.
    func getOrCreateDM(_ nick: String) -> ChannelState {
        let trimmed = nick.trimmingCharacters(in: .whitespaces)
        guard !trimmed.isEmpty else {
            // Return a throwaway buffer — never append empty nicks to the list
            return ChannelState(name: "_empty")
        }
        guard !trimmed.hasPrefix("#"), !trimmed.hasPrefix("&") else {
            // Caller handed us a channel name; route to the channel store instead.
            return getOrCreateChannel(trimmed)
        }
        if let existing = dmBuffers.first(where: { $0.name.lowercased() == trimmed.lowercased() }) {
            return existing
        }
        let dm = ChannelState(name: trimmed)
        dmBuffers.append(dm)
        requestHistory(channel: trimmed)
        SpotlightIndexer.reindex(self)
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

    fileprivate func updateAwayStatus(nick: String, awayMsg: String?) {
        for ch in channels {
            if let idx = ch.members.firstIndex(where: { $0.nick.lowercased() == nick.lowercased() }) {
                let m = ch.members[idx]
                ch.members[idx] = MemberInfo(nick: m.nick, isOp: m.isOp, isHalfop: m.isHalfop, isVoiced: m.isVoiced, awayMsg: awayMsg, did: m.did)
            }
        }
    }

    func awayMessage(for nick: String) -> String? {
        for ch in channels {
            if let m = ch.members.first(where: { $0.nick.lowercased() == nick.lowercased() }) {
                return m.awayMsg
            }
        }
        return nil
    }

    fileprivate func renameUser(oldNick: String, newNick: String) {
        for ch in channels {
            if let idx = ch.members.firstIndex(where: { $0.nick.lowercased() == oldNick.lowercased() }) {
                let m = ch.members[idx]
                ch.members[idx] = MemberInfo(nick: newNick, isOp: m.isOp, isHalfop: m.isHalfop, isVoiced: m.isVoiced, awayMsg: m.awayMsg, did: m.did)
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

    // Internal (not private) so tests in `freeqTests` can invoke the dispatcher
    // directly via `@testable import freeq` without going through the FFI.
    func handleEvent(_ event: FreeqEvent) {
        guard let state = appState else { return }

        switch event {
        case .connected:
            print("[freeq.event] .connected")
            state.connectionState = .connected

        case .registered(let nick):
            print("[freeq.event] .registered nick=\(nick)")
            // (continue to existing handler)
            state.connectionState = .registered
            state.reconnectAttempts = 0
            UINotificationFeedbackGenerator().notificationOccurred(.success)
            // Prefetch our own Bluesky avatar via the authenticated DID. Without
            // this we'd fall back to "<nick>.bsky.social" — which fails for users
            // whose handle is a custom domain (e.g. chadfowler.com), leaving
            // the self-avatar blank while other users' avatars resolve fine
            // because their messages carry account=did tags.
            if let did = state.authenticatedDID {
                Task { @MainActor in
                    AvatarCache.shared.prefetch(nick, did: did)
                }
            }
            // If we expected an authenticated session but got Guest, retry
            // instead of showing login screen. Token may have been stale.
            if state.authenticatedDID != nil && nick.lowercased().hasPrefix("guest") {
                state.disconnect()
                // Invalidate cached token — it was stale
                state.cachedWebToken = nil
                state.cachedWebTokenExpiry = .distantPast
                state.brokerRetryCount = 0
                // Retry via broker — will get a fresh token
                DispatchQueue.main.asyncAfter(deadline: .now() + 1) {
                    if state.connectionState == .disconnected && state.hasSavedSession {
                        state.pendingWebToken = nil  // Force broker refresh
                        state.reconnectSavedSession()
                    }
                }
                return
            }
            state.nick = nick
            // Auto-join saved channels
            for channel in state.autoJoinChannels {
                state.joinChannel(channel)
            }
            // Fetch DM conversation list if authenticated
            if state.authenticatedDID != nil {
                state.sendRaw("CHATHISTORY TARGETS * * 50")
            }

        case .authenticated(let did):
            state.authenticatedDID = did
            KeychainHelper.save(key: "did", value: did)
            // Refresh login timestamp so hasSavedSession stays valid
            UserDefaults.standard.set(Date().timeIntervalSince1970, forKey: "freeq.lastLogin")
            // Self-avatar by DID — covers the case where `.authenticated`
            // arrives after `.registered` so the prefetch in that handler
            // saw a nil DID.
            if !state.nick.isEmpty {
                Task { @MainActor in
                    AvatarCache.shared.prefetch(state.nick, did: did)
                }
            }

        case .authFailed(let reason):
            state.errorMessage = "Auth failed: \(reason)"

        case .joined(let channel, let nick):
            let ch = state.getOrCreateChannel(channel)
            ch.lastActivity = Date()
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
                // Fetch pinned messages
                state.fetchPins(channel: channel)
                // Don't show "you joined" system message — the user knows they joined
            } else {
                let msg = ChatMessage(
                    id: UUID().uuidString, from: "", text: "\(nick) joined",
                    isAction: false, timestamp: Date(), replyTo: nil
                )
                ch.appendIfNew(msg)
                if !ch.members.contains(where: { $0.nick.lowercased() == nick.lowercased() }) {
                    ch.members.append(MemberInfo(nick: nick, isOp: false, isHalfop: false, isVoiced: false, awayMsg: nil, did: nil))
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

            // Prefetch avatar using DID if available (from account-tag)
            if let did = ircMsg.account {
                Task { @MainActor in
                    AvatarCache.shared.prefetch(from, did: did)
                }
            }

            // Decode multiline: \\n → newline (server encodes newlines as literal \n)
            let decodedText = ircMsg.text.replacingOccurrences(of: "\\n", with: "\n")

            let msg = ChatMessage(
                id: ircMsg.msgid ?? UUID().uuidString,
                from: from,
                text: decodedText,
                isAction: ircMsg.isAction,
                timestamp: Date(timeIntervalSince1970: Double(ircMsg.timestampMs) / 1000.0),
                replyTo: ircMsg.replyTo,
                isSigned: ircMsg.isSigned
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

            // Handle pin/unpin notifications (update pins set, show as action message)
            if let pinMsgid = ircMsg.pinMsgid, target.hasPrefix("#") {
                let ch = state.getOrCreateChannel(target)
                ch.pins.insert(pinMsgid)
                ch.appendIfNew(msg)
                return
            }
            if let unpinMsgid = ircMsg.unpinMsgid, target.hasPrefix("#") {
                let ch = state.getOrCreateChannel(target)
                ch.pins.remove(unpinMsgid)
                ch.appendIfNew(msg)
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

                // Notify on mention (skip if muted)
                if !isSelf && ircMsg.text.lowercased().contains(state.nick.lowercased()) && !state.isMuted(target) {
                    NotificationManager.shared.sendMessageNotification(
                        from: from, text: ircMsg.text, channel: target, isMention: true
                    )
                    // Haptic when mentioned in active app
                    UINotificationFeedbackGenerator().notificationOccurred(.warning)
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
            // Deduplicate by lowercased nick (server may send same nick with different cases)
            var seen = Set<String>()
            ch.members = members.compactMap { m -> MemberInfo? in
                let key = m.nick.lowercased()
                guard !seen.contains(key) else { return nil }
                seen.insert(key)
                return MemberInfo(nick: m.nick, isOp: m.isOp, isHalfop: m.isHalfop, isVoiced: m.isVoiced, awayMsg: m.awayMsg, did: nil)
            }
            // Prefetch avatars for all channel members
            let nicks = members.map { $0.nick }
            Task { @MainActor in
                AvatarCache.shared.prefetchAll(nicks)
            }

        case .topicChanged(let channel, let topic):
            let ch = state.getOrCreateChannel(channel)
            ch.topic = topic.text
            ch.lastActivity = Date()

        case .modeChanged(let channel, let mode, let arg, _):
            guard let nick = arg else { break }
            let ch = state.getOrCreateChannel(channel)
            if let idx = ch.members.firstIndex(where: { $0.nick.lowercased() == nick.lowercased() }) {
                let member = ch.members[idx]
                switch mode {
                case "+o": ch.members[idx] = MemberInfo(nick: member.nick, isOp: true, isHalfop: false, isVoiced: member.isVoiced, awayMsg: member.awayMsg, did: member.did)
                case "-o": ch.members[idx] = MemberInfo(nick: member.nick, isOp: false, isHalfop: member.isHalfop, isVoiced: member.isVoiced, awayMsg: member.awayMsg, did: member.did)
                case "+h": ch.members[idx] = MemberInfo(nick: member.nick, isOp: member.isOp, isHalfop: true, isVoiced: member.isVoiced, awayMsg: member.awayMsg, did: member.did)
                case "-h": ch.members[idx] = MemberInfo(nick: member.nick, isOp: member.isOp, isHalfop: false, isVoiced: member.isVoiced, awayMsg: member.awayMsg, did: member.did)
                case "+v": ch.members[idx] = MemberInfo(nick: member.nick, isOp: member.isOp, isHalfop: member.isHalfop, isVoiced: true, awayMsg: member.awayMsg, did: member.did)
                case "-v": ch.members[idx] = MemberInfo(nick: member.nick, isOp: member.isOp, isHalfop: member.isHalfop, isVoiced: false, awayMsg: member.awayMsg, did: member.did)
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
                Task { @MainActor in
                    ToastManager.shared.show("Kicked from \(channel)", icon: "xmark.circle.fill")
                }
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

        case .chatHistoryTarget(let nick, _):
            // Create DM buffer for each conversation partner
            let _ = state.getOrCreateDM(nick)

        case .tagMsg(let tagMsg):
            let tags = Dictionary(uniqueKeysWithValues: tagMsg.tags.map { ($0.key, $0.value) })
            let target = tagMsg.target
            let from = tagMsg.from
            let isSelf = from.lowercased() == state.nick.lowercased()
            // For DMs, an echoed TAGMSG from ourselves carries target=peer, from=self —
            // route to the peer's DM buffer, not a DM keyed by our own nick.
            let dmBuffer = isSelf ? target : from
            let bufferName = target.hasPrefix("#") ? target : dmBuffer

            // Typing indicators
            if let typing = tags["+typing"] {
                if !isSelf {
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
                let ch = bufferName.hasPrefix("#") ? state.getOrCreateChannel(bufferName) : state.getOrCreateDM(bufferName)
                ch.applyDelete(msgId: deleteId)
            }

            // Reactions
            if let emoji = tags["+react"], let replyId = tags["+reply"] {
                let ch = bufferName.hasPrefix("#") ? state.getOrCreateChannel(bufferName) : state.getOrCreateDM(bufferName)
                ch.applyReaction(msgId: replyId, emoji: emoji, from: from)
            }

            // Reaction removal (toggle off)
            if let emoji = tags["+freeq.at/unreact"], let replyId = tags["+reply"] {
                let ch = bufferName.hasPrefix("#") ? state.getOrCreateChannel(bufferName) : state.getOrCreateDM(bufferName)
                ch.removeReaction(msgId: replyId, emoji: emoji, from: from)
            }

            // AV session lifecycle (`+freeq.at/av-state`)
            if let avState = tags["+freeq.at/av-state"],
               let avId = tags["+freeq.at/av-id"],
               target.hasPrefix("#") {
                let avActor = tags["+freeq.at/av-actor"] ?? from
                let chanKey = target.lowercased()
                switch avState {
                case "started":
                    state.activeAvSessions[chanKey] = avId
                    // If we triggered the start, auto-join now that we have an id.
                    if state.pendingAvStart.contains(chanKey)
                       && avActor.lowercased() == state.nick.lowercased() {
                        state.pendingAvStart.remove(chanKey)
                        state.startCall(channel: target, sessionId: avId)
                    }
                case "ended":
                    state.activeAvSessions.removeValue(forKey: chanKey)
                    state.pendingAvStart.remove(chanKey)
                    // If we were in this session, tear it down.
                    if state.isInCall
                       && state.currentCallChannel?.lowercased() == chanKey {
                        state.leaveCall()
                    }
                case "joined", "left":
                    // Session still active; nothing to update at the channel level.
                    break
                default:
                    break
                }
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
            // MOTD collection
            if text == "MOTD:START" {
                state.collectingMotd = true
                state.motdLines = []
            } else if text == "MOTD:END" {
                state.collectingMotd = false
                if !state.motdLines.isEmpty {
                    // Only show if content changed since last dismiss
                    let content = state.motdLines.joined(separator: "\n")
                    let hash = String(content.hashValue, radix: 36)
                    let seenHash = UserDefaults.standard.string(forKey: "freeq.motdSeenHash")
                    if hash != seenHash {
                        state.showMotd = true
                    }
                }
            } else if text.hasPrefix("MOTD:") {
                if state.collectingMotd {
                    state.motdLines.append(String(text.dropFirst(5)))
                }
            } else if !text.isEmpty {
                print("Notice: \(text)")
            }

        case .whoisReply(_, _):
            // WHOIS replies — currently unused in UI
            break

        case .disconnected(let reason):
            print("[freeq.event] .disconnected reason=\(reason)")
            state.connectionState = .disconnected
            if !reason.isEmpty && !reason.contains("EOF") {
                state.errorMessage = "Disconnected: \(reason)"
            }
            // FEAT-003: a WebSocket-named failure on this very attempt means
            // the network is hostile to WS — try plain TCP once before going
            // through the broker / showing the user any error UI.
            if state.attemptTransportFallback(reason: reason) {
                print("[freeq.event] -> falling back to TCP")
                return
            }
            // Auto-reconnect with exponential backoff
            if state.hasSavedSession {
                state.reconnectAttempts += 1
                // Fast first retry (1s), then 2, 4, 8, 15, 15...
                let delay = state.reconnectAttempts <= 1 ? 1.0 : min(Double(1 << min(state.reconnectAttempts - 1, 4)), 15.0)
                DispatchQueue.main.asyncAfter(deadline: .now() + delay) {
                    if state.connectionState == .disconnected && state.hasSavedSession {
                        state.reconnectSavedSession()
                    }
                }
            }
        }
    }
}

// ── AV Event Handler ──

final class AvCallbackHandler: @unchecked Sendable, AvEventHandler {
    private weak var appState: AppState?

    init(appState: AppState) {
        self.appState = appState
    }

    func onAvEvent(event: AvEvent) {
        DispatchQueue.main.async { [weak self] in
            guard let state = self?.appState else { return }

            switch event {
            case .connected:
                state.isInCall = true
                print("[av] Connected to MoQ SFU")

            case .disconnected(let reason):
                state.isInCall = false
                state.callParticipants = []
                state.currentCallChannel = nil
                state.currentCallSessionId = nil
                state.endCallActivity()
                print("[av] Disconnected: \(reason)")

            case .participantJoined(let nick):
                if !state.callParticipants.contains(nick) {
                    state.callParticipants.append(nick)
                }
                state.updateCallActivity()
                print("[av] Participant joined: \(nick)")

            case .participantLeft(let nick):
                state.callParticipants.removeAll { $0 == nick }
                state.updateCallActivity()
                print("[av] Participant left: \(nick)")

            case .audioTrackStarted(let nick):
                print("[av] Audio started: \(nick)")

            case .audioTrackStopped(let nick):
                print("[av] Audio stopped: \(nick)")

            case .error(let message):
                print("[av] Error: \(message)")
            }
        }
    }
}
