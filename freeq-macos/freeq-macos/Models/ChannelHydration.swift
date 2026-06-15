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
