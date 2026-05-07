package com.freeq.model

import com.freeq.model.BufferRouter.Target
import org.junit.Assert.assertEquals
import org.junit.Test

/**
 * Pure-JVM tests for the channel-vs-DM routing predicate. Mirrors the
 * iOS BufferRoutingTests at the predicate level so the same class of
 * bug — bare nicks polluting the Channels pane, channel names showing
 * up in DMs — is impossible by construction.
 */
class BufferRouterTest {

    @Test fun classifies_hash_prefixed_names_as_channel() {
        assertEquals(Target.CHANNEL, BufferRouter.classify("#freeq"))
    }

    @Test fun classifies_amp_prefixed_names_as_channel() {
        assertEquals(Target.CHANNEL, BufferRouter.classify("&local"))
    }

    @Test fun classifies_bare_nick_as_dm() {
        assertEquals(Target.DM, BufferRouter.classify("alice"))
    }

    @Test fun classifies_handle_with_dot_as_dm() {
        // Bluesky handles look like `chadfowler.com` — no channel prefix,
        // so they must route to DM.
        assertEquals(Target.DM, BufferRouter.classify("chadfowler.com"))
    }

    @Test fun classifies_at_prefixed_name_as_dm() {
        // The compose box accepts `@yokota` style mentions; routing must
        // not accidentally treat `@` as a channel sigil.
        assertEquals(Target.DM, BufferRouter.classify("@yokota"))
    }

    @Test fun trims_whitespace_before_classifying() {
        assertEquals(Target.CHANNEL, BufferRouter.classify("  #freeq  "))
        assertEquals(Target.DM, BufferRouter.classify("  alice  "))
    }

    @Test fun classifies_empty_or_blank_as_invalid() {
        assertEquals(Target.INVALID, BufferRouter.classify(""))
        assertEquals(Target.INVALID, BufferRouter.classify("   "))
        assertEquals(Target.INVALID, BufferRouter.classify("\t\n"))
    }

    @Test fun double_hash_is_still_a_channel() {
        // `##special` is a valid IRC channel name (some networks use it
        // for "channel of channels"); the router must not get clever.
        assertEquals(Target.CHANNEL, BufferRouter.classify("##special"))
    }

    @Test fun only_leading_prefix_counts() {
        // A `#` mid-name (e.g. someone typing a hashtag) must NOT route
        // to the channel path.
        assertEquals(Target.DM, BufferRouter.classify("alice#bob"))
    }
}
