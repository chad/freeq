package com.freeq.model

import kotlinx.coroutines.*
import org.json.JSONObject
import java.net.URL
import java.net.URLEncoder
import java.util.concurrent.ConcurrentHashMap

object AvatarCache {
    private val cache = ConcurrentHashMap<String, String>()  // nick -> avatar URL
    private val pending = ConcurrentHashMap.newKeySet<String>()
    private val failed = ConcurrentHashMap.newKeySet<String>()
    private val scope = CoroutineScope(Dispatchers.IO + SupervisorJob())

    fun avatarUrl(nick: String): String? = cache[nick.lowercase()]

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
            val url = resolveAvatar(handle)
            if (url != null) {
                cache[key] = url
                pending.remove(key)
                return
            }
        }
        failed.add(key)
        pending.remove(key)
    }

    private fun resolveAvatar(handle: String): String? {
        return try {
            val encoded = URLEncoder.encode(handle, "UTF-8")
            val url = URL("https://public.api.bsky.app/xrpc/app.bsky.actor.getProfile?actor=$encoded")
            val conn = url.openConnection().apply {
                connectTimeout = 5000
                readTimeout = 5000
            }
            val text = conn.getInputStream().bufferedReader().readText()
            val json = JSONObject(text)
            json.optString("avatar", null as String?)
        } catch (_: Exception) {
            null
        }
    }
}
