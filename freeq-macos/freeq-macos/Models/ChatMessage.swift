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
    // Origin server name when relayed from a federated peer (+freeq.at/origin).
    // nil = locally-originated. Drives "via {origin}" + suppresses the local
    // verified/signed badges, which would overstate trust for a peer-vouched msg.
    var origin: String? = nil
    var reactions: [String: Set<String>] = [:]  // emoji -> set of nicks

    static func == (lhs: ChatMessage, rhs: ChatMessage) -> Bool {
        lhs.id == rhs.id
            && lhs.from == rhs.from
            && lhs.text == rhs.text
            && lhs.isAction == rhs.isAction
            && lhs.timestamp == rhs.timestamp
            && lhs.replyTo == rhs.replyTo
            && lhs.isEdited == rhs.isEdited
            && lhs.isDeleted == rhs.isDeleted
            && lhs.isSigned == rhs.isSigned
            && lhs.origin == rhs.origin
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
