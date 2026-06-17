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
    var accessDeniedReason: String?
    var isHydratingHistory: Bool = false

    var id: String { name }
    var isChannel: Bool { name.hasPrefix("#") }
    var isDM: Bool { !name.hasPrefix("#") }
    var hasVisibleMessages: Bool { messages.contains { !$0.isDeleted } }

    var activeTypers: [String] {
        let cutoff = Date().addingTimeInterval(-5)
        return typingUsers.filter { $0.value > cutoff }.map(\.key).sorted()
    }

    private var messageIds: Set<String> = []
    private var messageIndexById: [String: Int] = [:]

    init(name: String) {
        self.name = name
    }

    func findMessage(byId id: String) -> Int? {
        messageIndexById[id]
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
            rebuildMessageIndex()
        } else {
            messages.append(msg)
            messageIndexById[msg.id] = messages.count - 1
        }
        if msg.timestamp > lastActivity {
            lastActivity = msg.timestamp
        }
    }

    /// Replace an optimistic local echo with the authoritative server echo.
    /// Keeps sending instant while avoiding duplicate self messages when the
    /// server returns the real msgid/signature/timestamp.
    @discardableResult
    func replacePendingEcho(with msg: ChatMessage, maxAge: TimeInterval = 120) -> Bool {
        guard let idx = messages.firstIndex(where: { pending in
            pending.id.hasPrefix("pending-")
                && pending.from.lowercased() == msg.from.lowercased()
                && pending.text == msg.text
                && abs(pending.timestamp.timeIntervalSince(msg.timestamp)) <= maxAge
        }) else { return false }

        let pendingId = messages[idx].id
        messageIds.remove(pendingId)
        messageIndexById.removeValue(forKey: pendingId)
        messageIds.insert(msg.id)
        messages[idx] = msg
        messageIndexById[msg.id] = idx
        if msg.timestamp > lastActivity {
            lastActivity = msg.timestamp
        }
        return true
    }

    func applyEdit(originalId: String, newId: String?, newText: String) {
        if let idx = findMessage(byId: originalId) {
            messages[idx].text = newText
            messages[idx].isEdited = true
            if let newId {
                messageIndexById.removeValue(forKey: messages[idx].id)
                messages[idx].id = newId
                messageIds.insert(newId)
                messageIndexById[newId] = idx
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
            var nicks = reactions[emoji] ?? Set()
            if nicks.contains(from) {
                nicks.remove(from)
                if nicks.isEmpty { reactions.removeValue(forKey: emoji) }
                else { reactions[emoji] = nicks }
            } else {
                nicks.insert(from)
                reactions[emoji] = nicks
            }
            messages[idx].reactions = reactions
        }
    }

    func addReaction(msgId: String, emoji: String, from: String) {
        guard !emoji.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty,
              let idx = findMessage(byId: msgId) else { return }
        var reactions = messages[idx].reactions
        var nicks = reactions[emoji] ?? Set()
        nicks.insert(from)
        reactions[emoji] = nicks
        messages[idx].reactions = reactions
    }

    func removeReaction(msgId: String, emoji: String, from: String) {
        guard let idx = findMessage(byId: msgId),
              var nicks = messages[idx].reactions[emoji] else { return }
        nicks.remove(from)
        if nicks.isEmpty {
            messages[idx].reactions.removeValue(forKey: emoji)
        } else {
            messages[idx].reactions[emoji] = nicks
        }
    }

    func hasReaction(msgId: String, emoji: String, from: String) -> Bool {
        guard let idx = findMessage(byId: msgId) else { return false }
        return messages[idx].reactions[emoji]?.contains(from) ?? false
    }

    private func rebuildMessageIndex() {
        messageIndexById = Dictionary(
            uniqueKeysWithValues: messages.enumerated().map { ($0.element.id, $0.offset) }
        )
    }
}
