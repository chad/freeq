package com.freeq.model

import com.freeq.ffi.IrcMessage
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * Pure-JVM tests for the FFI-IrcMessage → ChatMessage mapping.
 *
 * The signing badge silently regressed for months because nothing
 * exercised this plumbing — the FFI carried `isSigned` through but
 * the UI ChatMessage didn't have a field for it (commit 8869ca9).
 * These tests pin every wire field that needs to surface.
 */
class MessageMapperTest {

    private fun ircMsg(
        fromNick: String = "alice",
        target: String = "#freeq",
        text: String = "hello",
        msgid: String? = "01HX",
        replyTo: String? = null,
        replacesMsgid: String? = null,
        editOf: String? = null,
        batchId: String? = null,
        pinMsgid: String? = null,
        unpinMsgid: String? = null,
        isAction: Boolean = false,
        isSigned: Boolean = false,
        timestampMs: Long = 1_700_000_000_000L,
        account: String? = null,
    ) = IrcMessage(
        fromNick = fromNick,
        target = target,
        text = text,
        msgid = msgid,
        replyTo = replyTo,
        replacesMsgid = replacesMsgid,
        editOf = editOf,
        batchId = batchId,
        pinMsgid = pinMsgid,
        unpinMsgid = unpinMsgid,
        isAction = isAction,
        isSigned = isSigned,
        timestampMs = timestampMs,
        account = account,
    )

    @Test fun preserves_basic_fields() {
        val out = MessageMapper.fromIrc(ircMsg(
            fromNick = "alice",
            text = "hello world",
            timestampMs = 1_700_000_000_000L,
        ))
        assertEquals("alice", out.from)
        assertEquals("hello world", out.text)
        assertEquals(1_700_000_000_000L, out.timestamp.time)
    }

    @Test fun preserves_msgid_when_present() {
        val out = MessageMapper.fromIrc(ircMsg(msgid = "01HXABC123"))
        assertEquals("01HXABC123", out.id)
    }

    @Test fun synthesizes_uuid_when_msgid_is_null() {
        val out = MessageMapper.fromIrc(ircMsg(msgid = null))
        assertNotNull(out.id)
        assertTrue(
            "synthesized id should be UUID-shaped",
            out.id.matches(Regex("[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}"))
        )
    }

    @Test fun isSigned_propagates() {
        // The signing-badge regression hid here: FFI had isSigned, our
        // ChatMessage ignored it. This test pins the bit end-to-end.
        assertTrue(MessageMapper.fromIrc(ircMsg(isSigned = true)).isSigned)
        assertFalse(MessageMapper.fromIrc(ircMsg(isSigned = false)).isSigned)
    }

    @Test fun isAction_propagates() {
        assertTrue(MessageMapper.fromIrc(ircMsg(isAction = true)).isAction)
        assertFalse(MessageMapper.fromIrc(ircMsg(isAction = false)).isAction)
    }

    @Test fun replyTo_propagates() {
        assertEquals("01HXABCPARENT", MessageMapper.fromIrc(ircMsg(replyTo = "01HXABCPARENT")).replyTo)
        assertNull(MessageMapper.fromIrc(ircMsg(replyTo = null)).replyTo)
    }

    @Test fun output_has_zero_reactions_and_unedited_undeleted() {
        val out = MessageMapper.fromIrc(ircMsg())
        assertTrue(out.reactions.isEmpty())
        assertFalse(out.isEdited)
        assertFalse(out.isDeleted)
    }
}
