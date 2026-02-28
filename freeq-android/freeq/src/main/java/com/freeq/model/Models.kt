package com.freeq.model

import android.app.Application
import android.content.Context
import android.content.SharedPreferences
import androidx.security.crypto.EncryptedSharedPreferences
import androidx.security.crypto.MasterKey
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.mutableStateOf
import androidx.lifecycle.AndroidViewModel
import com.freeq.ffi.*
import kotlinx.coroutines.*
import java.util.*

// ── Data models ──

data class ChatMessage(
    val id: String,
    val from: String,
    var text: String,
    val isAction: Boolean,
    val timestamp: Date,
    val replyTo: String? = null,
    var isEdited: Boolean = false,
    var isDeleted: Boolean = false,
    val reactions: MutableMap<String, MutableSet<String>> = mutableMapOf()
)

data class MemberInfo(
    val nick: String,
    val isOp: Boolean,
    val isHalfop: Boolean = false,
    val isVoiced: Boolean,
    val awayMsg: String? = null,
    val did: String? = null
) {
    val prefix: String
        get() = when {
            isOp -> "@"
            isHalfop -> "%"
            isVoiced -> "+"
            else -> ""
        }
}

// ── Channel state ──

class ChannelState(val name: String) {
    val messages = mutableStateListOf<ChatMessage>()
    val members = mutableStateListOf<MemberInfo>()
    var topic = mutableStateOf("")
    val typingUsers = mutableStateMapOf<String, Date>()
    var lastActivityTime: Long = System.currentTimeMillis()

    private val messageIds = mutableSetOf<String>()

    val activeTypers: List<String>
        get() {
            val cutoff = Date().time - 5000
            return typingUsers.filter { it.value.time > cutoff }.keys.sorted()
        }

    fun findMessage(byId: String): Int? {
        return messages.indexOfFirst { it.id == byId }.takeIf { it >= 0 }
    }

    fun appendIfNew(msg: ChatMessage) {
        if (messageIds.contains(msg.id)) return
        messageIds.add(msg.id)
        if (messages.isNotEmpty() && msg.timestamp < messages.last().timestamp) {
            val idx = messages.indexOfFirst { it.timestamp > msg.timestamp }
            if (idx >= 0) messages.add(idx, msg) else messages.add(msg)
        } else {
            messages.add(msg)
        }
        if (msg.timestamp.time > lastActivityTime) {
            lastActivityTime = msg.timestamp.time
        }
    }

    fun applyEdit(originalId: String, newId: String?, newText: String) {
        val idx = findMessage(originalId) ?: return
        messages[idx] = messages[idx].copy(text = newText, isEdited = true)
        if (newId != null) messageIds.add(newId)
    }

    fun applyDelete(msgId: String) {
        val idx = findMessage(msgId) ?: return
        messages[idx] = messages[idx].copy(isDeleted = true, text = "")
    }

    fun applyReaction(msgId: String, emoji: String, from: String): Boolean {
        val idx = findMessage(msgId) ?: return true
        val msg = messages[idx]
        // Build entirely new collections — mutating in place causes old.equals(new)
        // to be true on the data class, so LazyColumn skips recomposition.
        val newReactions = mutableMapOf<String, MutableSet<String>>()
        var added = true
        for ((e, nicks) in msg.reactions) {
            if (e == emoji) {
                val newNicks = nicks.toMutableSet()
                if (from in newNicks) { newNicks.remove(from); added = false }
                else { newNicks.add(from); added = true }
                if (newNicks.isNotEmpty()) newReactions[e] = newNicks
            } else {
                newReactions[e] = nicks.toMutableSet()
            }
        }
        if (emoji !in msg.reactions) {
            newReactions[emoji] = mutableSetOf(from)
            added = true
        }
        messages[idx] = msg.copy(reactions = newReactions)
        return added
    }
}

// ── Connection state ──

enum class ConnectionState {
    Disconnected,
    Connecting,
    Connected,
    Registered
}

// ── AppState ViewModel ──

class AppState(application: Application) : AndroidViewModel(application) {
    var connectionState = mutableStateOf(ConnectionState.Disconnected)
    var nick = mutableStateOf("")
    var serverAddress = mutableStateOf("irc.freeq.at:6667")
    val channels = mutableStateListOf<ChannelState>()
    var activeChannel = mutableStateOf<String?>(null)
    var errorMessage = mutableStateOf<String?>(null)
    var authenticatedDID = mutableStateOf<String?>(null)
    val dmBuffers = mutableStateListOf<ChannelState>()
    val autoJoinChannels = mutableStateListOf<String>()
    val unreadCounts = mutableStateMapOf<String, Int>()

    var replyingTo = mutableStateOf<ChatMessage?>(null)
    var editingMessage = mutableStateOf<ChatMessage?>(null)

    var pendingWebToken: String? = null
    var pendingNavigation = mutableStateOf<String?>(null)
    var brokerToken: String? = null
    var authBrokerBase: String = "https://irc.freeq.at/auth/broker"
    private var brokerRetryCount = 0

    val hasSavedSession: Boolean
        get() = brokerToken != null && nick.value.isNotEmpty()
    val lastReadMessageIds = mutableStateMapOf<String, String>()
    val lastReadTimestamps = mutableStateMapOf<String, Long>()
    var isDarkTheme = mutableStateOf(true)

    val batches = mutableMapOf<String, BatchBuffer>()
    data class BatchBuffer(val target: String, val messages: MutableList<ChatMessage> = mutableListOf())

    // MOTD
    val motdLines = mutableStateListOf<String>()
    var showMotd = mutableStateOf(false)
    internal var collectingMotd = false

    private var client: FreeqClient? = null
    private var lastTypingSent: Long = 0
    var reconnectAttempts = 0
    internal val scope = CoroutineScope(Dispatchers.Main + SupervisorJob())
    val notificationManager = FreeqNotificationManager(application)
    val networkMonitor = NetworkMonitor(application).also { it.bind(this) }

    internal val prefs: SharedPreferences
        get() = getApplication<Application>().getSharedPreferences("freeq", Context.MODE_PRIVATE)

    internal val securePrefs: SharedPreferences by lazy {
        val masterKey = MasterKey.Builder(getApplication<Application>())
            .setKeyScheme(MasterKey.KeyScheme.AES256_GCM)
            .build()
        EncryptedSharedPreferences.create(
            getApplication(),
            "freeq_secure",
            masterKey,
            EncryptedSharedPreferences.PrefKeyEncryptionScheme.AES256_SIV,
            EncryptedSharedPreferences.PrefValueEncryptionScheme.AES256_GCM
        )
    }

    val activeChannelState: ChannelState?
        get() {
            val name = activeChannel.value ?: return null
            return channels.firstOrNull { it.name.equals(name, ignoreCase = true) }
                ?: dmBuffers.firstOrNull { it.name.equals(name, ignoreCase = true) }
        }

    init {
        // Migrate secrets from plain prefs to encrypted prefs (one-time)
        if (prefs.contains("brokerToken") || prefs.contains("did")) {
            prefs.getString("brokerToken", null)?.let { securePrefs.edit().putString("brokerToken", it).apply() }
            prefs.getString("did", null)?.let { securePrefs.edit().putString("did", it).apply() }
            prefs.edit().remove("brokerToken").remove("did").apply()
        }

        // Load secrets from encrypted storage
        brokerToken = securePrefs.getString("brokerToken", null)
        authenticatedDID.value = securePrefs.getString("did", null)

        // Restore persisted state
        nick.value = prefs.getString("nick", "") ?: ""
        serverAddress.value = prefs.getString("server", "irc.freeq.at:6667") ?: "irc.freeq.at:6667"
        prefs.getStringSet("channels", setOf("#general"))?.forEach { ch ->
            if (ch !in autoJoinChannels) autoJoinChannels.add(ch)
        }
        if (autoJoinChannels.isEmpty()) autoJoinChannels.add("#general")
        isDarkTheme.value = prefs.getBoolean("darkTheme", true)

        // Restore read positions
        prefs.getStringSet("readPositionKeys", emptySet())?.forEach { key ->
            prefs.getString("readPos_$key", null)?.let { lastReadMessageIds[key] = it }
            val ts = prefs.getLong("readPosTime_$key", 0L)
            if (ts > 0) lastReadTimestamps[key] = ts
        }

        // Prune stale typing indicators every 3 seconds
        scope.launch {
            while (isActive) {
                delay(3000)
                pruneTypingIndicators()
            }
        }
    }

    override fun onCleared() {
        super.onCleared()
        scope.cancel()
        networkMonitor.destroy()
        client?.disconnect()
    }

    // ── Connection ──

    fun connect(nickName: String) {
        nick.value = nickName
        connectionState.value = ConnectionState.Connecting
        errorMessage.value = null

        prefs.edit().putString("nick", nickName).putString("server", serverAddress.value).apply()

        try {
            val handler = AndroidEventHandler(this)
            client = FreeqClient(serverAddress.value, nickName, handler)
            client?.setPlatform("freeq android")

            pendingWebToken?.let { token ->
                client?.setWebToken(token)
                pendingWebToken = null
            }

            client?.connect()
        } catch (e: Exception) {
            connectionState.value = ConnectionState.Disconnected
            errorMessage.value = "Connection failed: ${e.message}"
        }
    }

    fun disconnect() {
        client?.disconnect()
        connectionState.value = ConnectionState.Disconnected
        channels.clear()
        dmBuffers.clear()
        activeChannel.value = null
        replyingTo.value = null
        editingMessage.value = null
        authenticatedDID.value = null
    }

    fun logout() {
        disconnect()
        brokerToken = null
        securePrefs.edit().remove("brokerToken").remove("did").apply()
        prefs.edit().remove("nick").apply()
        nick.value = ""
    }

    fun reconnectSavedSession() {
        if (!hasSavedSession || connectionState.value != ConnectionState.Disconnected) return
        if (pendingWebToken != null) { connect(nick.value); return }

        val token = brokerToken ?: return
        connectionState.value = ConnectionState.Connecting

        scope.launch {
            try {
                val session = withContext(Dispatchers.IO) { fetchBrokerSession(token) }
                brokerRetryCount = 0
                pendingWebToken = session.token
                authenticatedDID.value = session.did
                securePrefs.edit().putString("did", session.did).apply()
                connect(session.nick)
            } catch (e: Exception) {
                brokerRetryCount++
                if (brokerRetryCount <= 4) {
                    val delayMs = 3000L * (1L shl (brokerRetryCount - 1)) // 3, 6, 12, 24s
                    connectionState.value = ConnectionState.Disconnected
                    delay(delayMs)
                    if (connectionState.value == ConnectionState.Disconnected) {
                        reconnectSavedSession()
                    }
                } else {
                    connectionState.value = ConnectionState.Disconnected
                }
            }
        }
    }

    private data class BrokerSessionResponse(val token: String, val nick: String, val did: String)

    private fun fetchBrokerSession(brokerToken: String): BrokerSessionResponse {
        // Retry once on 502 (DPoP nonce rotation)
        for (attempt in 0..1) {
            val url = java.net.URL("$authBrokerBase/session")
            val conn = (url.openConnection() as java.net.HttpURLConnection).apply {
                requestMethod = "POST"
                doOutput = true
                connectTimeout = 10_000
                readTimeout = 10_000
                setRequestProperty("Content-Type", "application/json")
            }
            conn.outputStream.use { out ->
                out.write("""{"broker_token":"$brokerToken"}""".toByteArray())
            }
            val status = conn.responseCode
            if (status == 502 && attempt == 0) {
                Thread.sleep(300)
                continue
            }
            if (status != 200) {
                throw Exception("Broker returned $status")
            }
            val body = conn.inputStream.bufferedReader().readText()
            val json = org.json.JSONObject(body)
            return BrokerSessionResponse(
                token = json.getString("token"),
                nick = json.getString("nick"),
                did = json.getString("did")
            )
        }
        throw Exception("Broker failed after retries")
    }

    // ── Channel operations ──

    fun joinChannel(channel: String) {
        val ch = if (channel.startsWith("#")) channel else "#$channel"
        try {
            client?.join(ch)
        } catch (_: Exception) {
            errorMessage.value = "Failed to join $ch"
        }
    }

    fun partChannel(channel: String) {
        try {
            client?.part(channel)
        } catch (_: Exception) {}
    }

    // ── Messaging ──

    fun sendMessage(target: String, text: String) {
        if (text.isEmpty()) return
        sendRaw("@+typing=done TAGMSG $target")
        lastTypingSent = 0

        // Edit mode
        val editing = editingMessage.value
        if (editing != null) {
            val escaped = text.replace("\r", "").replace("\n", " ")
            sendRaw("@+draft/edit=${editing.id} PRIVMSG $target :$escaped")
            editingMessage.value = null
            return
        }

        // Reply mode
        val reply = replyingTo.value
        if (reply != null) {
            val escaped = text.replace("\r", "").replace("\n", " ")
            sendRaw("@+reply=${reply.id} PRIVMSG $target :$escaped")
            replyingTo.value = null
            return
        }

        try {
            client?.sendMessage(target, text)
        } catch (_: Exception) {
            errorMessage.value = "Send failed"
        }
    }

    fun sendRaw(line: String) {
        try {
            client?.sendRaw(line)
        } catch (_: Exception) {}
    }

    fun sendReaction(target: String, msgId: String, emoji: String) {
        val ch = channels.firstOrNull { it.name.equals(target, ignoreCase = true) }
            ?: dmBuffers.firstOrNull { it.name.equals(target, ignoreCase = true) }
        val added = ch?.applyReaction(msgId, emoji, nick.value) ?: true
        if (added) {
            sendRaw("@+react=$emoji;+reply=$msgId TAGMSG $target")
        }
    }

    fun deleteMessage(target: String, msgId: String) {
        // Optimistic local delete — server doesn't echo TAGMSG to sender
        val ch = channels.firstOrNull { it.name.equals(target, ignoreCase = true) }
            ?: dmBuffers.firstOrNull { it.name.equals(target, ignoreCase = true) }
        ch?.applyDelete(msgId)
        sendRaw("@+draft/delete=$msgId TAGMSG $target")
    }

    fun sendTyping(target: String) {
        val now = System.currentTimeMillis()
        if (now - lastTypingSent < 3000) return
        lastTypingSent = now
        sendRaw("@+typing=active TAGMSG $target")
    }

    fun requestHistory(channel: String) {
        sendRaw("CHATHISTORY LATEST $channel * 50")
    }

    // ── Read tracking ──

    fun markRead(channel: String) {
        unreadCounts[channel] = 0
        val state = channels.firstOrNull { it.name == channel }
            ?: dmBuffers.firstOrNull { it.name == channel }
        // Prefer the last real message (has a sender) — system messages use random UUIDs
        // that don't survive CHATHISTORY replay
        val lastMsg = state?.messages?.lastOrNull { it.from.isNotEmpty() }
            ?: state?.messages?.lastOrNull()
        lastMsg?.let {
            lastReadMessageIds[channel] = it.id
            lastReadTimestamps[channel] = it.timestamp.time
            persistReadPositions()
        }
    }

    fun incrementUnread(channel: String) {
        if (activeChannel.value != channel) {
            unreadCounts[channel] = (unreadCounts[channel] ?: 0) + 1
        }
    }

    // ── Theme ──

    fun toggleTheme() {
        isDarkTheme.value = !isDarkTheme.value
        prefs.edit().putBoolean("darkTheme", isDarkTheme.value).apply()
    }

    // ── Channel helpers ──

    fun getOrCreateChannel(name: String): ChannelState {
        channels.firstOrNull { it.name.equals(name, ignoreCase = true) }?.let { return it }
        val channel = ChannelState(name)
        channels.add(channel)
        return channel
    }

    fun getOrCreateDM(nick: String): ChannelState {
        dmBuffers.firstOrNull { it.name.equals(nick, ignoreCase = true) }?.let { return it }
        val dm = ChannelState(nick)
        dmBuffers.add(dm)
        requestHistory(nick)
        return dm
    }

    // ── Persistence ──

    internal fun persistChannels() {
        prefs.edit().putStringSet("channels", autoJoinChannels.toSet()).apply()
    }

    private fun persistReadPositions() {
        val editor = prefs.edit()
        editor.putStringSet("readPositionKeys", lastReadMessageIds.keys.toSet())
        lastReadMessageIds.forEach { (key, value) -> editor.putString("readPos_$key", value) }
        lastReadTimestamps.forEach { (key, value) -> editor.putLong("readPosTime_$key", value) }
        editor.apply()
    }

    private fun pruneTypingIndicators() {
        val cutoff = Date().time - 5000
        for (ch in channels + dmBuffers) {
            val stale = ch.typingUsers.filter { it.value.time < cutoff }.keys.toList()
            stale.forEach { ch.typingUsers.remove(it) }
        }
    }

    fun renameUser(oldNick: String, newNick: String) {
        for (ch in channels) {
            val idx = ch.members.indexOfFirst { it.nick.equals(oldNick, ignoreCase = true) }
            if (idx >= 0) {
                ch.members[idx] = ch.members[idx].copy(nick = newNick)
            }
            ch.typingUsers.remove(oldNick)?.let { ch.typingUsers[newNick] = it }
        }
        val dmIdx = dmBuffers.indexOfFirst { it.name.equals(oldNick, ignoreCase = true) }
        if (dmIdx >= 0) {
            val old = dmBuffers[dmIdx]
            val renamed = ChannelState(newNick)
            renamed.messages.addAll(old.messages)
            renamed.members.addAll(old.members)
            renamed.topic.value = old.topic.value
            renamed.typingUsers.putAll(old.typingUsers)
            dmBuffers.removeAt(dmIdx)
            dmBuffers.add(renamed)
            unreadCounts.remove(old.name)?.let { unreadCounts[newNick] = it }
        }
        if (nick.value.equals(oldNick, ignoreCase = true)) {
            nick.value = newNick
        }
    }

    fun awayMessage(nick: String): String? {
        for (ch in channels) {
            val member = ch.members.firstOrNull { it.nick.equals(nick, ignoreCase = true) }
            if (member?.awayMsg != null) return member.awayMsg
        }
        return null
    }

    fun updateAwayStatus(nick: String, awayMsg: String?) {
        for (ch in channels) {
            val idx = ch.members.indexOfFirst { it.nick.equals(nick, ignoreCase = true) }
            if (idx >= 0) {
                ch.members[idx] = ch.members[idx].copy(awayMsg = awayMsg)
            }
        }
    }
}

// ── Event handler ──

class AndroidEventHandler(private val state: AppState) : EventHandler {
    override fun onEvent(event: FreeqEvent) {
        CoroutineScope(Dispatchers.Main).launch {
            handleEvent(event)
        }
    }

    private fun handleEvent(event: FreeqEvent) {
        when (event) {
            is FreeqEvent.Connected -> {
                state.connectionState.value = ConnectionState.Connected
            }

            is FreeqEvent.Registered -> {
                state.reconnectAttempts = 0
                // If authenticated user got Guest nick, token was stale — retry broker
                if (state.authenticatedDID.value != null
                    && event.nick.startsWith("Guest", ignoreCase = true)) {
                    state.disconnect()
                    state.scope.launch {
                        delay(2000)
                        if (state.connectionState.value == ConnectionState.Disconnected
                            && state.hasSavedSession) {
                            state.pendingWebToken = null
                            state.reconnectSavedSession()
                        }
                    }
                    return
                }
                state.connectionState.value = ConnectionState.Registered
                state.nick.value = event.nick
                state.autoJoinChannels.toList().forEach { state.joinChannel(it) }
            }

            is FreeqEvent.Authenticated -> {
                state.authenticatedDID.value = event.did
            }

            is FreeqEvent.AuthFailed -> {
                state.errorMessage.value = "Auth failed: ${event.reason}"
            }

            is FreeqEvent.Joined -> {
                val ch = state.getOrCreateChannel(event.channel)
                ch.lastActivityTime = System.currentTimeMillis()
                if (event.nick.equals(state.nick.value, ignoreCase = true)) {
                    if (state.activeChannel.value == null) {
                        state.activeChannel.value = event.channel
                    }
                    if (state.autoJoinChannels.none { it.equals(event.channel, ignoreCase = true) }) {
                        state.autoJoinChannels.add(event.channel)
                        state.persistChannels()
                    }
                    state.requestHistory(event.channel)
                }
                ch.appendIfNew(ChatMessage(
                    id = UUID.randomUUID().toString(),
                    from = "",
                    text = "${event.nick} joined",
                    isAction = false,
                    timestamp = Date()
                ))
            }

            is FreeqEvent.Parted -> {
                if (event.nick.equals(state.nick.value, ignoreCase = true)) {
                    state.channels.removeAll { it.name == event.channel }
                    state.autoJoinChannels.removeAll { it.equals(event.channel, ignoreCase = true) }
                    state.persistChannels()
                    if (state.activeChannel.value == event.channel) {
                        state.activeChannel.value = state.channels.firstOrNull()?.name
                    }
                } else {
                    val ch = state.getOrCreateChannel(event.channel)
                    ch.appendIfNew(ChatMessage(
                        id = UUID.randomUUID().toString(),
                        from = "",
                        text = "${event.nick} left",
                        isAction = false,
                        timestamp = Date()
                    ))
                    ch.members.removeAll { it.nick.equals(event.nick, ignoreCase = true) }
                }
            }

            is FreeqEvent.Message -> {
                val ircMsg = event.msg
                val isSelf = ircMsg.fromNick.equals(state.nick.value, ignoreCase = true)

                val msg = ChatMessage(
                    id = ircMsg.msgid ?: UUID.randomUUID().toString(),
                    from = ircMsg.fromNick,
                    text = ircMsg.text,
                    isAction = ircMsg.isAction,
                    timestamp = Date(ircMsg.timestampMs),
                    replyTo = ircMsg.replyTo
                )

                // Handle edits (prefer editOf, fall back to replacesMsgid)
                val editTarget = ircMsg.editOf ?: ircMsg.replacesMsgid
                if (editTarget != null) {
                    val batchId = ircMsg.batchId
                    if (batchId != null) {
                        state.batches[batchId]?.let { batch ->
                            val idx = batch.messages.indexOfFirst { it.id == editTarget }
                            if (idx >= 0) {
                                batch.messages[idx] = batch.messages[idx].copy(text = ircMsg.text, isEdited = true)
                            } else {
                                batch.messages.add(msg)
                            }
                        }
                        return
                    }
                    val ch = if (ircMsg.target.startsWith("#")) {
                        state.channels.firstOrNull { it.name.equals(ircMsg.target, ignoreCase = true) }
                    } else {
                        val bufferName = if (isSelf) ircMsg.target else ircMsg.fromNick
                        state.dmBuffers.firstOrNull { it.name.equals(bufferName, ignoreCase = true) }
                    }
                    ch?.applyEdit(editTarget, ircMsg.msgid, ircMsg.text)
                    ch?.typingUsers?.remove(ircMsg.fromNick)
                    return
                }

                // If part of CHATHISTORY batch, buffer for later merge
                val batchId = ircMsg.batchId
                if (batchId != null && state.batches.containsKey(batchId)) {
                    state.batches[batchId]?.messages?.add(msg)
                    return
                }

                if (ircMsg.target.startsWith("#")) {
                    val ch = state.getOrCreateChannel(ircMsg.target)
                    ch.appendIfNew(msg)
                    state.incrementUnread(ircMsg.target)
                    ch.typingUsers.remove(ircMsg.fromNick)

                    if (!isSelf && ircMsg.text.contains(state.nick.value, ignoreCase = true)) {
                        state.notificationManager.sendMessageNotification(
                            from = ircMsg.fromNick, text = ircMsg.text, channel = ircMsg.target
                        )
                    }
                } else {
                    val bufferName = if (isSelf) ircMsg.target else ircMsg.fromNick
                    val dm = state.getOrCreateDM(bufferName)
                    dm.appendIfNew(msg)
                    state.incrementUnread(bufferName)

                    if (!isSelf) {
                        state.notificationManager.sendMessageNotification(
                            from = ircMsg.fromNick, text = ircMsg.text, channel = bufferName
                        )
                    }
                }
            }

            is FreeqEvent.Names -> {
                val ch = state.getOrCreateChannel(event.channel)
                ch.members.clear()
                ch.members.addAll(event.members.map {
                    MemberInfo(nick = it.nick, isOp = it.isOp, isHalfop = it.isHalfop, isVoiced = it.isVoiced, awayMsg = it.awayMsg)
                })
                AvatarCache.prefetchAll(event.members.map { it.nick })
            }

            is FreeqEvent.TopicChanged -> {
                val ch = state.getOrCreateChannel(event.channel)
                ch.topic.value = event.topic.text
                ch.lastActivityTime = System.currentTimeMillis()
            }

            is FreeqEvent.ModeChanged -> {
                val nick = event.arg ?: return
                val ch = state.channels.firstOrNull { it.name.equals(event.channel, ignoreCase = true) } ?: return
                val idx = ch.members.indexOfFirst { it.nick.equals(nick, ignoreCase = true) }
                if (idx >= 0) {
                    val m = ch.members[idx]
                    ch.members[idx] = when (event.mode) {
                        "+o" -> m.copy(isOp = true)
                        "-o" -> m.copy(isOp = false)
                        "+h" -> m.copy(isHalfop = true)
                        "-h" -> m.copy(isHalfop = false)
                        "+v" -> m.copy(isVoiced = true)
                        "-v" -> m.copy(isVoiced = false)
                        else -> m
                    }
                }
            }

            is FreeqEvent.Kicked -> {
                if (event.nick.equals(state.nick.value, ignoreCase = true)) {
                    state.channels.removeAll { it.name == event.channel }
                    state.autoJoinChannels.removeAll { it.equals(event.channel, ignoreCase = true) }
                    state.persistChannels()
                    if (state.activeChannel.value == event.channel) {
                        state.activeChannel.value = state.channels.firstOrNull()?.name
                    }
                    state.errorMessage.value = "Kicked from ${event.channel} by ${event.by}: ${event.reason}"
                } else {
                    val ch = state.getOrCreateChannel(event.channel)
                    ch.appendIfNew(ChatMessage(
                        id = UUID.randomUUID().toString(),
                        from = "",
                        text = "${event.nick} was kicked by ${event.by} (${event.reason})",
                        isAction = false,
                        timestamp = Date()
                    ))
                    ch.members.removeAll { it.nick.equals(event.nick, ignoreCase = true) }
                }
            }

            is FreeqEvent.UserQuit -> {
                for (ch in state.channels) {
                    ch.members.removeAll { it.nick.equals(event.nick, ignoreCase = true) }
                    ch.typingUsers.remove(event.nick)
                }
            }

            is FreeqEvent.Notice -> {
                val text = event.text
                if (text == "MOTD:START") {
                    state.collectingMotd = true
                    state.motdLines.clear()
                } else if (text == "MOTD:END") {
                    state.collectingMotd = false
                    if (state.motdLines.isNotEmpty()) {
                        val content = state.motdLines.joinToString("\n")
                        val hash = content.hashCode().toString(36)
                        val seenHash = state.prefs.getString("motd_seen_hash", null)
                        if (hash != seenHash) {
                            state.showMotd.value = true
                        }
                    }
                } else if (text.startsWith("MOTD:") && state.collectingMotd) {
                    state.motdLines.add(text.removePrefix("MOTD:"))
                } else if (!state.collectingMotd && text.isNotBlank()) {
                    // Server error or notice — show to user
                    state.errorMessage.value = text
                }
            }

            is FreeqEvent.Disconnected -> {
                state.connectionState.value = ConnectionState.Disconnected
                if (event.reason.isNotEmpty()) {
                    state.errorMessage.value = "Disconnected: ${event.reason}"
                }
                // Auto-reconnect: prefer broker session restore, fall back to plain reconnect
                if (state.nick.value.isNotEmpty()) {
                    state.reconnectAttempts++
                    val delay = minOf(1L shl minOf(state.reconnectAttempts, 5), 30L)
                    state.scope.launch {
                        kotlinx.coroutines.delay(delay * 1000)
                        if (state.connectionState.value == ConnectionState.Disconnected
                            && state.nick.value.isNotEmpty()) {
                            if (state.hasSavedSession) {
                                state.reconnectSavedSession()
                            } else {
                                state.connect(state.nick.value)
                            }
                        }
                    }
                }
            }

            is FreeqEvent.TagMsg -> {
                val tags = event.msg.tags.associate { it.key to it.value }
                val target = event.msg.target
                val from = event.msg.from
                // Typing indicators (ignore self)
                tags["+typing"]?.let { typing ->
                    if (!from.equals(state.nick.value, ignoreCase = true)) {
                        val bufferName = if (target.startsWith("#")) target else from
                        val ch = if (bufferName.startsWith("#"))
                            state.channels.firstOrNull { it.name.equals(bufferName, ignoreCase = true) }
                        else
                            state.dmBuffers.firstOrNull { it.name.equals(bufferName, ignoreCase = true) }
                        ch?.let {
                            if (typing == "active") it.typingUsers[from] = Date()
                            else if (typing == "done") it.typingUsers.remove(from)
                        }
                    }
                }

                // Message deletion (ignore self — already handled optimistically by deleteMessage)
                tags["+draft/delete"]?.let { deleteId ->
                    if (!from.equals(state.nick.value, ignoreCase = true)) {
                        val bufferName = if (target.startsWith("#")) target else from
                        val ch = if (bufferName.startsWith("#"))
                            state.channels.firstOrNull { it.name.equals(bufferName, ignoreCase = true) }
                        else
                            state.dmBuffers.firstOrNull { it.name.equals(bufferName, ignoreCase = true) }
                        ch?.applyDelete(deleteId)
                    }
                }

                // Reactions (ignore self — already handled optimistically by sendReaction)
                val emoji = tags["+react"]
                val replyId = tags["+reply"]
                if (emoji != null && replyId != null && !from.equals(state.nick.value, ignoreCase = true)) {
                    val bufferName = if (target.startsWith("#")) target else from
                    val ch = if (bufferName.startsWith("#"))
                        state.channels.firstOrNull { it.name.equals(bufferName, ignoreCase = true) }
                    else
                        state.dmBuffers.firstOrNull { it.name.equals(bufferName, ignoreCase = true) }
                    ch?.applyReaction(replyId, emoji, from)
                }
            }

            is FreeqEvent.NickChanged -> {
                state.renameUser(event.oldNick, event.newNick)
            }

            is FreeqEvent.AwayChanged -> {
                state.updateAwayStatus(event.nick, event.awayMsg)
            }

            is FreeqEvent.BatchStart -> {
                state.batches[event.id] = AppState.BatchBuffer(target = event.target)
            }

            is FreeqEvent.BatchEnd -> {
                val batch = state.batches.remove(event.id) ?: return
                val sorted = batch.messages.sortedBy { it.timestamp }
                val ch = if (batch.target.startsWith("#"))
                    state.getOrCreateChannel(batch.target)
                else
                    state.getOrCreateDM(batch.target)
                sorted.forEach { ch.appendIfNew(it) }
            }
        }
    }
}
