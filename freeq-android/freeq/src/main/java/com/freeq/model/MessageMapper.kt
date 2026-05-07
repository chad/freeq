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
    fun fromIrc(ircMsg: IrcMessage): ChatMessage = ChatMessage(
        id = ircMsg.msgid ?: UUID.randomUUID().toString(),
        from = ircMsg.fromNick,
        text = ircMsg.text,
        isAction = ircMsg.isAction,
        timestamp = Date(ircMsg.timestampMs),
        replyTo = ircMsg.replyTo,
        isSigned = ircMsg.isSigned,
    )
}
