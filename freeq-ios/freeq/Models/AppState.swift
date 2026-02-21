import Foundation
import SwiftUI

/// A single chat message.
struct ChatMessage: Identifiable, Equatable {
    let id: String  // msgid or UUID
    let from: String
    let text: String
    let isAction: Bool
    let timestamp: Date
    let replyTo: String?

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

    var id: String { name }

    init(name: String) {
        self.name = name
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

    /// DM buffers (keyed by nick, not channel)
    @Published var dmBuffers: [ChannelState] = []

    private var client: FreeqClient? = nil

    var activeChannelState: ChannelState? {
        if let name = activeChannel {
            return channels.first { $0.name == name } ?? dmBuffers.first { $0.name == name }
        }
        return nil
    }

    func connect(nick: String) {
        self.nick = nick
        self.connectionState = .connecting
        self.errorMessage = nil

        do {
            let handler = SwiftEventHandler(appState: self)
            client = try FreeqClient(
                server: serverAddress,
                nick: nick,
                handler: handler
            )
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
        }
    }

    func joinChannel(_ channel: String) {
        let ch = channel.hasPrefix("#") ? channel : "#\(channel)"
        do {
            try client?.join(channel: ch)
        } catch {
            DispatchQueue.main.async {
                self.errorMessage = "Failed to join \(ch): \(error)"
            }
        }
    }

    func partChannel(_ channel: String) {
        do {
            try client?.part(channel: channel)
        } catch {
            print("Part failed: \(error)")
        }
    }

    func sendMessage(target: String, text: String) {
        guard !text.isEmpty else { return }
        do {
            try client?.sendMessage(target: target, text: text)
        } catch {
            DispatchQueue.main.async {
                self.errorMessage = "Send failed: \(error)"
            }
        }
    }

    func sendRaw(_ line: String) {
        try? client?.sendRaw(line: line)
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
            // Auto-join default channel
            state.joinChannel("#general")

        case .authenticated(let did):
            state.authenticatedDID = did

        case .authFailed(let reason):
            state.errorMessage = "Auth failed: \(reason)"

        case .joined(let channel, let nick):
            let ch = state.getOrCreateChannel(channel)
            if nick.lowercased() == state.nick.lowercased() {
                state.activeChannel = channel
            }
            // Add system message
            let msg = ChatMessage(
                id: UUID().uuidString,
                from: "",
                text: "\(nick) joined \(channel)",
                isAction: false,
                timestamp: Date(),
                replyTo: nil
            )
            ch.messages.append(msg)

        case .parted(let channel, let nick):
            if nick.lowercased() == state.nick.lowercased() {
                state.channels.removeAll { $0.name == channel }
                if state.activeChannel == channel {
                    state.activeChannel = state.channels.first?.name
                }
            } else {
                let ch = state.getOrCreateChannel(channel)
                let msg = ChatMessage(
                    id: UUID().uuidString,
                    from: "",
                    text: "\(nick) left \(channel)",
                    isAction: false,
                    timestamp: Date(),
                    replyTo: nil
                )
                ch.messages.append(msg)
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
                ch.messages.append(msg)
            } else {
                // DM — buffer keyed by the other person
                let bufferName = isSelf ? target : from
                let dm = state.getOrCreateDM(bufferName)
                dm.messages.append(msg)
            }

        case .names(let channel, let members):
            let ch = state.getOrCreateChannel(channel)
            ch.members = members.map { MemberInfo(nick: $0.nick, isOp: $0.isOp, isVoiced: $0.isVoiced) }

        case .topicChanged(let channel, let topic):
            let ch = state.getOrCreateChannel(channel)
            ch.topic = topic.text

        case .modeChanged(_, _, _, _):
            break

        case .kicked(let channel, let nick, let by, let reason):
            if nick.lowercased() == state.nick.lowercased() {
                state.channels.removeAll { $0.name == channel }
                if state.activeChannel == channel {
                    state.activeChannel = state.channels.first?.name
                }
                state.errorMessage = "Kicked from \(channel) by \(by): \(reason)"
            } else {
                let ch = state.getOrCreateChannel(channel)
                let msg = ChatMessage(
                    id: UUID().uuidString,
                    from: "",
                    text: "\(nick) was kicked by \(by) (\(reason))",
                    isAction: false,
                    timestamp: Date(),
                    replyTo: nil
                )
                ch.messages.append(msg)
            }

        case .userQuit(let nick, _):
            // Remove from all channel member lists
            for ch in state.channels {
                ch.members.removeAll { $0.nick.lowercased() == nick.lowercased() }
            }

        case .notice(let text):
            if !text.isEmpty {
                print("Notice: \(text)")
            }

        case .disconnected(let reason):
            state.connectionState = .disconnected
            state.errorMessage = "Disconnected: \(reason)"
        }
    }
}
