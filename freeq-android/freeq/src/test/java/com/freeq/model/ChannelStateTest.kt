package com.freeq.model

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import java.util.Date

/**
 * Pure-JVM unit tests for `ChannelState`. The class only uses Compose
 * runtime (`mutableStateListOf` etc.) which works on the host JVM, so no
 * Robolectric or instrumented runtime is needed.
 *
 * Covers the hot ingest paths the IRC event handler calls into:
 * `appendIfNew`, `applyEdit`, `applyDelete`, `applyReaction`.
 */
class ChannelStateTest {

    private fun msg(
        id: String = "m-${idCounter++}",
        from: String = "alice",
        text: String = "hello",
        timestamp: Date = Date(idCounter * 1000L),
        isSigned: Boolean = false,
    ) = ChatMessage(
        id = id,
        from = from,
        text = text,
        isAction = false,
        timestamp = timestamp,
        isSigned = isSigned,
    )

    private var idCounter = 0L

    // ── appendIfNew ──

    @Test fun appendIfNew_appends_new_message_in_order() {
        val ch = ChannelState("#test")
        ch.appendIfNew(msg(id = "a", timestamp = Date(1)))
        ch.appendIfNew(msg(id = "b", timestamp = Date(2)))
        ch.appendIfNew(msg(id = "c", timestamp = Date(3)))
        assertEquals(listOf("a", "b", "c"), ch.messages.map { it.id })
    }

    @Test fun appendIfNew_dedups_by_id() {
        val ch = ChannelState("#test")
        ch.appendIfNew(msg(id = "x", text = "first"))
        ch.appendIfNew(msg(id = "x", text = "should-be-ignored"))
        assertEquals(1, ch.messages.size)
        assertEquals("first", ch.messages[0].text)
    }

    @Test fun appendIfNew_inserts_out_of_order_message_in_timestamp_position() {
        // History replay can deliver an older message after newer ones have
        // already been appended; the channel must keep messages in
        // chronological order so CHATHISTORY backfill renders cleanly.
        val ch = ChannelState("#test")
        ch.appendIfNew(msg(id = "newer", timestamp = Date(100)))
        ch.appendIfNew(msg(id = "newest", timestamp = Date(200)))
        ch.appendIfNew(msg(id = "older", timestamp = Date(50)))
        assertEquals(listOf("older", "newer", "newest"), ch.messages.map { it.id })
    }

    @Test fun appendIfNew_only_real_messages_update_lastActivityTime() {
        // System join/part messages have empty `from`; they must NOT bump
        // the "recent activity" indicator the chat-list uses for sorting.
        val ch = ChannelState("#test")
        val before = ch.lastActivityTime.value
        ch.appendIfNew(msg(id = "sys", from = "", timestamp = Date(1000)))
        assertEquals(before, ch.lastActivityTime.value)

        ch.appendIfNew(msg(id = "real", from = "alice", timestamp = Date(2000)))
        assertEquals(2000L, ch.lastActivityTime.value)
    }

    // ── applyEdit ──

    @Test fun applyEdit_updates_text_and_marks_edited() {
        val ch = ChannelState("#test")
        ch.appendIfNew(msg(id = "a", text = "original"))
        ch.applyEdit(originalId = "a", newId = null, newText = "edited")
        assertEquals("edited", ch.messages[0].text)
        assertTrue(ch.messages[0].isEdited)
    }

    @Test fun applyEdit_registers_new_id_so_followup_dedup_works() {
        val ch = ChannelState("#test")
        ch.appendIfNew(msg(id = "a", text = "v1"))
        ch.applyEdit(originalId = "a", newId = "a-edit-1", newText = "v2")
        // A re-delivery of the edit shouldn't append a duplicate.
        ch.appendIfNew(msg(id = "a-edit-1", text = "duplicate"))
        assertEquals(1, ch.messages.size)
    }

    @Test fun applyEdit_no_op_when_message_id_unknown() {
        val ch = ChannelState("#test")
        ch.appendIfNew(msg(id = "a", text = "hi"))
        ch.applyEdit(originalId = "missing", newId = null, newText = "x")
        assertEquals("hi", ch.messages[0].text)
        assertFalse(ch.messages[0].isEdited)
    }

    // ── applyDelete ──

    @Test fun applyDelete_clears_text_and_sets_flag() {
        val ch = ChannelState("#test")
        ch.appendIfNew(msg(id = "a", text = "private!"))
        ch.applyDelete("a")
        assertEquals("", ch.messages[0].text)
        assertTrue(ch.messages[0].isDeleted)
    }

    @Test fun applyDelete_no_op_when_message_id_unknown() {
        val ch = ChannelState("#test")
        ch.appendIfNew(msg(id = "a", text = "hi"))
        ch.applyDelete("missing")
        assertEquals(1, ch.messages.size)
        assertFalse(ch.messages[0].isDeleted)
    }

    // ── applyReaction ──

    @Test fun applyReaction_adds_first_reaction() {
        val ch = ChannelState("#test")
        ch.appendIfNew(msg(id = "a"))
        val added = ch.applyReaction("a", "👍", "alice")
        assertTrue(added)
        assertEquals(setOf("alice"), ch.messages[0].reactions["👍"])
    }

    @Test fun applyReaction_toggles_off_when_same_user_reacts_again() {
        val ch = ChannelState("#test")
        ch.appendIfNew(msg(id = "a"))
        ch.applyReaction("a", "👍", "alice")
        val secondAdd = ch.applyReaction("a", "👍", "alice")
        assertFalse("toggling off must report `added=false`", secondAdd)
        assertNull(ch.messages[0].reactions["👍"])
    }

    @Test fun applyReaction_keeps_other_users_reactions_when_one_toggles_off() {
        val ch = ChannelState("#test")
        ch.appendIfNew(msg(id = "a"))
        ch.applyReaction("a", "👍", "alice")
        ch.applyReaction("a", "👍", "bob")
        ch.applyReaction("a", "👍", "alice") // alice toggles off
        assertEquals(setOf("bob"), ch.messages[0].reactions["👍"])
    }

    @Test fun applyReaction_replaces_message_object_so_compose_recomposes() {
        // A LazyColumn reading via mutableStateListOf only recomposes when
        // the element identity changes (data class .equals would otherwise
        // make pre-/post- look identical to Compose). Verify the message
        // reference is replaced, not mutated in place.
        val ch = ChannelState("#test")
        ch.appendIfNew(msg(id = "a"))
        val before = ch.messages[0]
        ch.applyReaction("a", "👍", "alice")
        val after = ch.messages[0]
        assertNotNull(after.reactions["👍"])
        // Different instance — the helper builds a new map.
        assertFalse(before === after)
    }

    @Test fun applyReaction_returns_true_for_unknown_message() {
        // Existing behavior: unknown msg id returns true (no-op,
        // documented in the helper). Documenting it here so future
        // changes go through a deliberate decision.
        val ch = ChannelState("#test")
        assertTrue(ch.applyReaction("missing", "👍", "alice"))
    }

    // ── findMessage ──

    @Test fun findMessage_returns_index_when_present() {
        val ch = ChannelState("#test")
        ch.appendIfNew(msg(id = "a"))
        ch.appendIfNew(msg(id = "b"))
        ch.appendIfNew(msg(id = "c"))
        assertEquals(0, ch.findMessage("a"))
        assertEquals(2, ch.findMessage("c"))
    }

    @Test fun findMessage_returns_null_when_absent() {
        val ch = ChannelState("#test")
        assertNull(ch.findMessage("nope"))
    }
}
