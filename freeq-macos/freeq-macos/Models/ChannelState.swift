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
}
