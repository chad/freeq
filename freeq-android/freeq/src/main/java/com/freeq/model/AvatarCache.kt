package com.freeq.model

import kotlinx.coroutines.*
import org.json.JSONObject
import java.net.URL
import java.net.URLEncoder
import java.util.concurrent.ConcurrentHashMap

data class BlueskyProfile(
    val handle: String,
    val displayName: String?,
    val description: String?,
    val avatar: String?,
    val followersCount: Int?,
    val followsCount: Int?,
    val postsCount: Int?
)

object AvatarCache {
    private val cache = ConcurrentHashMap<String, String>()  // nick -> avatar URL
    private val profileCache = ConcurrentHashMap<String, BlueskyProfile>()
    private val pending = ConcurrentHashMap.newKeySet<String>()
    private val failed = ConcurrentHashMap.newKeySet<String>()
    private val scope = CoroutineScope(Dispatchers.IO + SupervisorJob())

    fun avatarUrl(nick: String): String? = cache[nick.lowercase()]

    fun profile(nick: String): BlueskyProfile? = profileCache[nick.lowercase()]

    suspend fun fetchProfileIfNeeded(nick: String, did: String?): BlueskyProfile? {
        val key = nick.lowercase()
        profileCache[key]?.let { return it }
        // DID-only: without a verified, non-did:key DID there is no Bluesky
        // profile to show. See fetchAvatar for why we never resolve from a nick.
        if (did.isNullOrEmpty() || did.startsWith("did:key:")) return null
        pending.add(key)
        fetchAvatar(key, did)
        return profileCache[key]
    }

    fun prefetch(nick: String, did: String? = null) {
        val key = nick.lowercase()
        // Skip guest nicks - they're not Bluesky accounts.
        if (key.startsWith("guest") || key.startsWith("web")) return
        if (cache.containsKey(key) || pending.contains(key)) return
        // Identity on freeq is the DID the server bound at SASL — never the
        // freely-settable nick. Without a verified DID there is nothing we can
        // safely resolve, and did:key users (guests, AI beings) have no Bluesky
        // profile. A no-DID call is a no-op (and NOT marked failed) so a later
        // call carrying the account-tag DID is never pre-empted.
        if (did.isNullOrEmpty() || did.startsWith("did:key:")) return
        if (failed.contains(key)) return
        pending.add(key)
        scope.launch { fetchAvatar(key, did) }
    }

    fun prefetchAll(nicks: List<String>) {
        nicks.forEach { prefetch(it) }
    }

    private suspend fun fetchAvatar(key: String, did: String) {
        // Resolve ONLY by the server-verified DID. We must never derive a
        // Bluesky identity from the nick — neither the bare nick as a handle
        // nor a guessed "<nick>.bsky.social". Nicks are freely chosen, so any
        // such guess shows a STRANGER's photo and handle for whoever happens to
        // match (e.g. the AI being "olive" pulling up the unrelated real
        // account olive.bsky.social). That is impersonation.
        val result = resolveProfile(did)
        if (result != null) {
            profileCache[key] = result
            result.avatar?.let { cache[key] = it }
        } else {
            failed.add(key)
        }
        pending.remove(key)
    }

    // Test seam: unit tests override this to assert we only ever resolve by a
    // verified DID (never a nick or a guessed "<nick>.bsky.social"). Defaults
    // to the live Bluesky API call.
    internal var resolveProfile: (String) -> BlueskyProfile? = ::resolveProfileNetwork

    /** Clear all caches and restore the live resolver. Test-only. */
    internal fun resetForTest() {
        cache.clear(); profileCache.clear(); pending.clear(); failed.clear()
        resolveProfile = ::resolveProfileNetwork
    }

    private fun resolveProfileNetwork(handle: String): BlueskyProfile? {
        return try {
            val encoded = URLEncoder.encode(handle, "UTF-8")
            val url = URL("https://public.api.bsky.app/xrpc/app.bsky.actor.getProfile?actor=$encoded")
            val conn = url.openConnection().apply {
                connectTimeout = 5000
                readTimeout = 5000
            }
            val text = conn.getInputStream().bufferedReader().readText()
            val json = JSONObject(text)
            BlueskyProfile(
                handle = json.optString("handle", handle),
                displayName = json.opt("displayName") as? String,
                description = json.opt("description") as? String,
                avatar = json.opt("avatar") as? String,
                followersCount = if (json.has("followersCount")) json.optInt("followersCount") else null,
                followsCount = if (json.has("followsCount")) json.optInt("followsCount") else null,
                postsCount = if (json.has("postsCount")) json.optInt("postsCount") else null
            )
        } catch (_: Exception) {
            null
        }
    }
}
