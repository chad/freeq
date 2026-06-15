import Foundation

enum ChannelHydration {
    static let defaultHistoryLimit = 50

    static func historyCommand(for channel: String, limit: Int = defaultHistoryLimit) -> String? {
        let trimmed = channel.trimmingCharacters(in: .whitespacesAndNewlines)
        guard trimmed.hasPrefix("#") || trimmed.hasPrefix("&") else { return nil }
        return "CHATHISTORY LATEST \(trimmed) * \(limit)"
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
