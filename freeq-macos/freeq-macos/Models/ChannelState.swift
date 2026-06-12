import Foundation
import SwiftUI

/// A channel with its messages and members.
@Observable
class ChannelState: Identifiable {
    let name: String
    var messages: [ChatMessage] = []
    var members: [MemberInfo] = []
    var topic: String = ""
    var topicSetBy: String?
    var pinnedMessages: [ChatMessage] = []
    var typingUsers: [String: Date] = [:]
    var lastActivity: Date = Date()
    var isEncrypted: Bool = false

    var id: String { name }
    var isChannel: Bool { name.hasPrefix("#") }
    var isDM: Bool { !name.hasPrefix("#") }

    var activeTypers: [String] {
        let cutoff = Date().addingTimeInterval(-5)
        return typingUsers.filter { $0.value > cutoff }.map(\.key).sorted()
    }

    private var messageIds: Set<String> = []

    init(name: String) {
        self.name = name
    }

    func findMessage(byId id: String) -> Int? {
        messages.firstIndex(where: { $0.id == id })
    }

    func memberInfo(for nick: String) -> MemberInfo? {
        members.first(where: { $0.nick.lowercased() == nick.lowercased() })
    }

    /// Append a message only if its ID hasn't been seen before.
    func appendIfNew(_ msg: ChatMessage) {
        guard !messageIds.contains(msg.id) else { return }
        messageIds.insert(msg.id)

        if let last = messages.last, msg.timestamp < last.timestamp {
            let idx = messages.firstIndex(where: { $0.timestamp > msg.timestamp }) ?? messages.endIndex
            messages.insert(msg, at: idx)
        } else {
            messages.append(msg)
        }
        if msg.timestamp > lastActivity {
            lastActivity = msg.timestamp
        }
    }

    func applyEdit(originalId: String, newId: String?, newText: String) {
        if let idx = findMessage(byId: originalId) {
            messages[idx].text = newText
            messages[idx].isEdited = true
            if let newId {
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

    /// Idempotently add `from` to a message's reaction set. Used for both the
    /// optimistic local update and inbound `+react` events, so a self-echo of
    /// our own reaction is a harmless no-op (rather than toggling it back off).
    func addReaction(msgId: String, emoji: String, from: String) {
        if let idx = findMessage(byId: msgId) {
            var reactions = messages[idx].reactions
            var nicks = reactions[emoji] ?? Set()
            nicks.insert(from)
            reactions[emoji] = nicks
            messages[idx].reactions = reactions
        }
    }

    /// Whether `from` has already reacted to `msgId` with `emoji`.
    func hasReaction(msgId: String, emoji: String, from: String) -> Bool {
        guard let idx = findMessage(byId: msgId) else { return false }
        return messages[idx].reactions[emoji]?.contains(from) ?? false
    }

    /// Explicitly remove a reaction (from a `+freeq.at/unreact` tag), as
    /// opposed to the toggle in `applyReaction`.
    func removeReaction(msgId: String, emoji: String, from: String) {
        if let idx = findMessage(byId: msgId) {
            var reactions = messages[idx].reactions
            guard var nicks = reactions[emoji] else { return }
            nicks.remove(from)
            if nicks.isEmpty { reactions.removeValue(forKey: emoji) }
            else { reactions[emoji] = nicks }
            messages[idx].reactions = reactions
        }
    }
}
