import Foundation

enum ChannelHydration {
    static let defaultHistoryLimit = 50

    static func historyCommand(for channel: String, limit: Int = defaultHistoryLimit) -> String? {
        let trimmed = channel.trimmingCharacters(in: .whitespacesAndNewlines)
        guard trimmed.hasPrefix("#") || trimmed.hasPrefix("&") else { return nil }
        return "CHATHISTORY LATEST \(trimmed) * \(limit)"
    }
}

enum MessageVisibility {
    static func visibleMessages(from messages: [ChatMessage]) -> [ChatMessage] {
        messages.filter { !$0.isDeleted }
    }

    static func shouldShowWelcome(messages: [ChatMessage]) -> Bool {
        visibleMessages(from: messages).isEmpty
    }
}

enum HistoryBatchRouting {
    static func shouldBuffer(batchType: String, target: String) -> Bool {
        if !target.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            return true
        }

        // `CHATHISTORY TARGETS` uses a batch only as a list delimiter; it
        // emits dedicated ChatHistoryTarget events, not messages for a buffer.
        let normalizedType = batchType.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        return normalizedType == "chathistory"
            || normalizedType == "draft/chathistory"
            || normalizedType == "freeq.at/search"
    }

    static func resolvedTarget(batchTarget: String, messageTarget: String) -> String {
        let trimmedBatchTarget = batchTarget.trimmingCharacters(in: .whitespacesAndNewlines)
        if !trimmedBatchTarget.isEmpty { return trimmedBatchTarget }
        return messageTarget.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    static func shouldApplyBatch(target: String, messageCount: Int) -> Bool {
        messageCount > 0 && !target.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    }

    @discardableResult
    static func apply(
        buffer: HistoryBatchBuffer,
        channels: inout [ChannelState],
        dmBuffers: inout [ChannelState]
    ) -> Bool {
        let target = buffer.target.trimmingCharacters(in: .whitespacesAndNewlines)
        guard shouldApplyBatch(target: target, messageCount: buffer.messages.count) else {
            return false
        }

        let destination: ChannelState
        if target.hasPrefix("#") || target.hasPrefix("&") {
            if let existing = channels.first(where: { $0.name.lowercased() == target.lowercased() }) {
                destination = existing
            } else {
                destination = ChannelState(name: target)
                channels.append(destination)
            }
        } else {
            if let existing = dmBuffers.first(where: { $0.name.lowercased() == target.lowercased() }) {
                destination = existing
            } else {
                destination = ChannelState(name: target)
                dmBuffers.append(destination)
            }
        }

        for message in buffer.messages.sorted(by: { $0.timestamp < $1.timestamp }) {
            destination.appendIfNew(message)
        }
        return true
    }
}

struct HistoryBatchBuffer {
    var target: String
    var messages: [ChatMessage] = []

    mutating func learnTarget(from messageTarget: String) {
        target = HistoryBatchRouting.resolvedTarget(
            batchTarget: target,
            messageTarget: messageTarget
        )
    }

    mutating func append(_ message: ChatMessage, messageTarget: String) {
        learnTarget(from: messageTarget)
        messages.append(message)
    }
}

/// Decides when the client should ask the server for authenticated DM targets.
/// Registration and DID-authentication can arrive in either order, so the
/// request must wait for both and still only fire once per TCP connection.
struct DmTargetBootstrap {
    static let command = "CHATHISTORY TARGETS * * 50"

    static func shouldRequest(isRegistered: Bool, authenticatedDID: String?, alreadyRequested: Bool) -> Bool {
        isRegistered && authenticatedDID != nil && !alreadyRequested
    }
}

enum BlueskyProfileBootstrap {
    static func actor(nick: String, did: String?) -> String? {
        if let did, did.hasPrefix("did:plc:") {
            return did
        }
        if let did, did.hasPrefix("did:key:") {
            return nil
        }

        let trimmed = nick.trimmingCharacters(in: .whitespacesAndNewlines)
        guard looksLikeHandle(trimmed) else { return nil }
        return trimmed
    }

    static func looksLikeHandle(_ value: String) -> Bool {
        let trimmed = value.trimmingCharacters(in: .whitespacesAndNewlines)
        guard trimmed.contains("."),
              !trimmed.contains(" "),
              !trimmed.hasPrefix("."),
              !trimmed.hasSuffix("."),
              !trimmed.hasPrefix("#"),
              !trimmed.hasPrefix("&"),
              !trimmed.hasPrefix("did:") else {
            return false
        }
        return trimmed.allSatisfy { char in
            char.isLetter || char.isNumber || char == "." || char == "-"
        }
    }
}

enum AvCommandAction: Equatable {
    case startOrJoin
    case leave
    case mute
    case camera
    case screenShare
    case help
}

enum AvCommandParser {
    static func action(for argument: String) -> AvCommandAction {
        let subcommand = argument
            .split(separator: " ")
            .first
            .map { String($0).lowercased() } ?? ""
        switch subcommand {
        case "", "start", "join":
            return .startOrJoin
        case "leave", "end", "hangup":
            return .leave
        case "mute":
            return .mute
        case "camera", "video":
            return .camera
        case "screen", "share", "screenshare":
            return .screenShare
        default:
            return .help
        }
    }
}

enum ServerNoticeRoute: Equatable {
    case ignore
    case motdStart
    case motdLine(String)
    case motdEnd
    case namesEnd(String)
    case apiBearer(String)
    case display(String)
}

enum ServerNoticeRouter {
    static func route(_ text: String) -> ServerNoticeRoute {
        if text.isEmpty { return .ignore }
        if text == "MOTD:START" { return .motdStart }
        if text == "MOTD:END" { return .motdEnd }
        if text.hasPrefix("MOTD:") {
            return .motdLine(String(text.dropFirst("MOTD:".count)))
        }
        if text.hasPrefix("__NAMES_END__") {
            return .namesEnd(String(text.dropFirst("__NAMES_END__".count)))
        }
        if text.hasPrefix("API-BEARER ") {
            let token = text.dropFirst("API-BEARER ".count)
                .trimmingCharacters(in: .whitespacesAndNewlines)
            return token.isEmpty ? .ignore : .apiBearer(token)
        }
        return .display(text)
    }
}
