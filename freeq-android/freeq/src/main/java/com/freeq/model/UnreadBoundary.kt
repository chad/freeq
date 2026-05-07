package com.freeq.model

/**
 * Decides which message in a channel should render the "Unread" divider
 * line. Pulled out of MessageList.kt so the rule can be unit-tested
 * (it has two fallback paths and several "user caught up" edge cases).
 *
 * Returns null if:
 *  - there is no `lastReadId` and no `lastReadTimestamp` to anchor on,
 *  - all unread messages are system join/part chatter (no real `from`),
 *  - or the user has themselves posted a message after the boundary —
 *    they're "caught up" and the divider would just be in the way.
 */
internal object UnreadBoundary {

    fun find(
        messages: List<ChatMessage>,
        lastReadId: String?,
        lastReadTimestamp: Long,
        nick: String,
    ): String? {
        // Primary: find lastReadId in messages.
        if (lastReadId != null) {
            val idx = messages.indexOfFirst { it.id == lastReadId }
            if (idx >= 0 && idx < messages.size - 1) {
                val tail = messages.subList(idx + 1, messages.size)
                val hasRealUnread = tail.any { it.from.isNotEmpty() }
                val userCaughtUp = tail.any { it.from.equals(nick, ignoreCase = true) }
                if (hasRealUnread && !userCaughtUp) return messages[idx + 1].id
            }
        }

        // Fallback: first real message after lastReadTimestamp.
        if (lastReadTimestamp > 0) {
            val idx = messages.indexOfFirst {
                it.timestamp.time > lastReadTimestamp && it.from.isNotEmpty()
            }
            if (idx >= 0) {
                val tail = messages.subList(idx, messages.size)
                val userCaughtUp = tail.any { it.from.equals(nick, ignoreCase = true) }
                if (!userCaughtUp) return messages[idx].id
            }
        }

        return null
    }
}
