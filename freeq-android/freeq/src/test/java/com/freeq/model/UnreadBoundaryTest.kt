package com.freeq.model

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test
import java.util.Date

/**
 * Tests for the unread-divider placement rule. Has two fallback paths
 * (lastReadId, lastReadTimestamp) and several "user caught up" guards;
 * easy to silently break.
 */
class UnreadBoundaryTest {

    private fun msg(id: String, from: String = "alice", ts: Long = 0L) = ChatMessage(
        id = id,
        from = from,
        text = "x",
        isAction = false,
        timestamp = Date(ts),
    )

    private fun systemMsg(id: String, ts: Long = 0L) = msg(id, from = "", ts = ts)

    @Test fun returns_id_after_lastReadId_when_there_is_a_real_unread() {
        val msgs = listOf(msg("a", ts = 1), msg("b", ts = 2), msg("c", from = "bob", ts = 3))
        val out = UnreadBoundary.find(msgs, lastReadId = "a", lastReadTimestamp = 0, nick = "me")
        assertEquals("b", out)
    }

    @Test fun null_when_lastReadId_is_at_the_tail() {
        val msgs = listOf(msg("a", ts = 1), msg("b", ts = 2))
        val out = UnreadBoundary.find(msgs, lastReadId = "b", lastReadTimestamp = 0, nick = "me")
        assertNull(out)
    }

    @Test fun null_when_user_has_caught_up_after_lastReadId() {
        // The user themselves posted after lastReadId — no need for a divider.
        val msgs = listOf(msg("a"), msg("b", from = "bob"), msg("c", from = "me"))
        val out = UnreadBoundary.find(msgs, lastReadId = "a", lastReadTimestamp = 0, nick = "me")
        assertNull(out)
    }

    @Test fun caught_up_check_is_case_insensitive() {
        val msgs = listOf(msg("a"), msg("b", from = "bob"), msg("c", from = "ME"))
        val out = UnreadBoundary.find(msgs, lastReadId = "a", lastReadTimestamp = 0, nick = "me")
        assertNull(out)
    }

    @Test fun null_when_only_system_messages_after_lastReadId() {
        val msgs = listOf(msg("a"), systemMsg("sys-1"), systemMsg("sys-2"))
        val out = UnreadBoundary.find(msgs, lastReadId = "a", lastReadTimestamp = 0, nick = "me")
        assertNull(out)
    }

    @Test fun falls_back_to_timestamp_when_lastReadId_not_found() {
        // Cross-session: prior session pruned old messages so the saved
        // ID isn't present anymore. Timestamp fallback kicks in.
        val msgs = listOf(msg("x", ts = 100), msg("y", from = "bob", ts = 200))
        val out = UnreadBoundary.find(msgs, lastReadId = "missing", lastReadTimestamp = 50, nick = "me")
        assertEquals("x", out)
    }

    @Test fun timestamp_fallback_skips_system_messages() {
        // The first thing after lastReadTimestamp is a join/part — keep
        // scanning for the first REAL message.
        val msgs = listOf(systemMsg("sys", ts = 60), msg("real", from = "bob", ts = 70))
        val out = UnreadBoundary.find(msgs, lastReadId = null, lastReadTimestamp = 50, nick = "me")
        assertEquals("real", out)
    }

    @Test fun timestamp_fallback_returns_null_when_user_caught_up() {
        val msgs = listOf(
            msg("a", from = "bob", ts = 100),
            msg("b", from = "me", ts = 200),
        )
        val out = UnreadBoundary.find(msgs, lastReadId = null, lastReadTimestamp = 50, nick = "me")
        assertNull(out)
    }

    @Test fun returns_null_when_no_anchors_at_all() {
        val msgs = listOf(msg("a", from = "bob", ts = 1))
        val out = UnreadBoundary.find(msgs, lastReadId = null, lastReadTimestamp = 0, nick = "me")
        assertNull(out)
    }

    @Test fun returns_null_for_empty_message_list() {
        val out = UnreadBoundary.find(emptyList(), lastReadId = "a", lastReadTimestamp = 100, nick = "me")
        assertNull(out)
    }
}
