package com.freeq.model

import com.freeq.ffi.IrcMessage
import java.util.Date
import java.util.UUID

/**
 * Pure mapper from the FFI's `IrcMessage` to the UI's `ChatMessage`.
 *
 * Pulled out of `AndroidEventHandler.onEvent` so the field plumbing —
 * notably `isSigned`, which silently dropped on the floor before
 * commit 8869ca9 — has its own unit-test.
 */
internal object MessageMapper {
    fun fromIrc(ircMsg: IrcMessage): ChatMessage {
        // Reactions persisted on the message itself (via the server's
        // `+freeq.at/reactions` tag on CHATHISTORY / JOIN replay) ride
        // through `ircMsg.reactions`. Live reactions still arrive as
        // separate TagMsg events and route through `applyReaction`.
        val reactions = mutableMapOf<String, MutableSet<String>>()
        for (tally in ircMsg.reactions) {
            if (tally.emoji.isEmpty() || tally.nicks.isEmpty()) continue
            reactions[tally.emoji] = tally.nicks.toMutableSet()
        }
        return ChatMessage(
            id = ircMsg.msgid ?: UUID.randomUUID().toString(),
            from = ircMsg.fromNick,
            text = ircMsg.text,
            isAction = ircMsg.isAction,
            timestamp = Date(ircMsg.timestampMs),
            replyTo = ircMsg.replyTo,
            isSigned = ircMsg.isSigned,
            reactions = reactions,
        )
    }
}
