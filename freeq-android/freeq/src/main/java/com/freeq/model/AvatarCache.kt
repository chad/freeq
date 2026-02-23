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

    suspend fun fetchProfileIfNeeded(nick: String): BlueskyProfile? {
        val key = nick.lowercase()
        profileCache[key]?.let { return it }
        fetchAvatar(nick, key)
        return profileCache[key]
    }

    fun prefetch(nick: String) {
        val key = nick.lowercase()
        if (cache.containsKey(key) || pending.contains(key) || failed.contains(key)) return
        pending.add(key)
        scope.launch { fetchAvatar(nick, key) }
    }

    fun prefetchAll(nicks: List<String>) {
        nicks.forEach { prefetch(it) }
    }

    private suspend fun fetchAvatar(nick: String, key: String) {
        val handles = if (nick.contains(".")) listOf(nick) else listOf("$nick.bsky.social")

        for (handle in handles) {
            val result = resolveProfile(handle)
            if (result != null) {
                profileCache[key] = result
                result.avatar?.let { cache[key] = it }
                pending.remove(key)
                return
            }
        }
        failed.add(key)
        pending.remove(key)
    }

    private fun resolveProfile(handle: String): BlueskyProfile? {
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
