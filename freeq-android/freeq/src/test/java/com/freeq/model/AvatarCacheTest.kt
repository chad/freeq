package com.freeq.model

import kotlinx.coroutines.runBlocking
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test

/**
 * Identity must resolve ONLY from the server-verified DID — never from the
 * freely-chosen nick. Resolving a nick (or a guessed "<nick>.bsky.social")
 * surfaced a STRANGER's photo/handle for whoever happened to match — i.e.
 * impersonation. These tests lock that the resolver is only ever invoked with
 * a verified, non-did:key DID, and never with a nick.
 */
class AvatarCacheTest {

    /** Args every resolver invocation was called with, in order. */
    private val resolverArgs = mutableListOf<String>()

    private val fakeProfile = BlueskyProfile(
        handle = "real.bsky.social",
        displayName = "Real Person",
        description = null,
        avatar = "https://cdn/avatar.jpg",
        followersCount = null, followsCount = null, postsCount = null,
    )

    @Before
    fun setUp() {
        AvatarCache.resetForTest()
        resolverArgs.clear()
        AvatarCache.resolveProfile = { arg ->
            resolverArgs.add(arg)
            fakeProfile
        }
    }

    @Test
    fun `no DID does not resolve at all`() = runBlocking {
        val result = AvatarCache.fetchProfileIfNeeded("olive", null)
        assertNull("no DID must yield no profile", result)
        assertTrue("resolver must not be called without a DID", resolverArgs.isEmpty())
    }

    @Test
    fun `empty DID does not resolve`() = runBlocking {
        val result = AvatarCache.fetchProfileIfNeeded("olive", "")
        assertNull(result)
        assertTrue(resolverArgs.isEmpty())
    }

    @Test
    fun `did key never resolves a Bluesky profile`() = runBlocking {
        // Agents / guests are did:key — they have no Bluesky profile, and a
        // matching nick must NOT pull up a stranger's account.
        val result = AvatarCache.fetchProfileIfNeeded("olive", "did:key:z6MkExample")
        assertNull("did:key must yield no profile", result)
        assertTrue("resolver must not be called for did:key", resolverArgs.isEmpty())
    }

    @Test
    fun `verified DID resolves by the DID, never by the nick`() = runBlocking {
        val result = AvatarCache.fetchProfileIfNeeded("olive", "did:plc:abc123")
        assertEquals(fakeProfile, result)
        // The single resolver call must be the DID — never "olive" nor
        // "olive.bsky.social".
        assertEquals(listOf("did:plc:abc123"), resolverArgs)
    }
}
