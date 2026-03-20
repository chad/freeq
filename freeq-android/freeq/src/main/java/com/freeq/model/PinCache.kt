package com.freeq.model

import kotlinx.coroutines.*
import org.json.JSONObject
import java.net.URL
import java.net.URLEncoder
import java.util.concurrent.ConcurrentHashMap

object PinCache {
    private val cache = ConcurrentHashMap<String, Set<String>>()
    private val pending = ConcurrentHashMap.newKeySet<String>()
    private val scope = CoroutineScope(Dispatchers.IO + SupervisorJob())

    fun isPinned(channel: String, msgId: String): Boolean =
        cache[channel.lowercase()]?.contains(msgId) == true

    fun prefetch(channel: String) {
        val key = channel.lowercase()
        if (cache.containsKey(key) || pending.contains(key)) return
        pending.add(key)
        scope.launch { fetchPins(channel, key) }
    }

    fun addPin(channel: String, msgId: String) {
        val key = channel.lowercase()
        cache[key] = (cache[key] ?: emptySet()) + msgId
    }

    fun removePin(channel: String, msgId: String) {
        val key = channel.lowercase()
        cache[key]?.let { cache[key] = it - msgId }
    }

    private suspend fun fetchPins(channel: String, key: String) {
        try {
            val encoded = URLEncoder.encode(channel, "UTF-8")
            val url = URL("https://irc.freeq.at/api/v1/channels/$encoded/pins")
            val conn = url.openConnection().apply {
                connectTimeout = 5000
                readTimeout = 5000
            }
            val text = conn.getInputStream().bufferedReader().readText()
            val json = JSONObject(text)
            val pinsArray = json.optJSONArray("pins")
            if (pinsArray != null) {
                val msgIds = (0 until pinsArray.length()).mapNotNull { i ->
                    pinsArray.getJSONObject(i).optString("msgid").takeIf { it.isNotEmpty() }
                }.toSet()
                cache[key] = msgIds
            }
        } catch (_: Exception) {}
        finally { pending.remove(key) }
    }
}
