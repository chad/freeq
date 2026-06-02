import Foundation

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
        // Compare the mutable fields too — comparing only `id` makes a message
        // with a new reaction/edit/deletion look unchanged, which can cause
        // SwiftUI to diff-skip the row's re-render.
        lhs.id == rhs.id
            && lhs.text == rhs.text
            && lhs.isEdited == rhs.isEdited
            && lhs.isDeleted == rhs.isDeleted
            && lhs.isSigned == rhs.isSigned
            && lhs.reactions == rhs.reactions
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

    var id: String { nick }

    var prefix: String {
        if isOp { return "@" }
        if isHalfop { return "%" }
        if isVoiced { return "+" }
        return ""
    }

    var isAway: Bool { awayMsg != nil }
    var isVerified: Bool { did != nil }
}
