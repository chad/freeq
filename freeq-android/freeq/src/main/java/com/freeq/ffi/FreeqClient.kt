package com.freeq.ffi

import kotlinx.coroutines.*

// TODO: Replace with real UniFFI-generated FreeqClient backed by native .so library.
// This stub emulates the SDK for UI development and testing.

class FreeqClient(
    private val server: String,
    private var nick: String,
    private val handler: EventHandler
) {
    private var connected = false
    private var webToken: String? = null
    private val scope = CoroutineScope(Dispatchers.IO + SupervisorJob())

    @Throws(FreeqError::class)
    fun setWebToken(token: String) {
        webToken = token
    }

    @Throws(FreeqError::class)
    fun connect() {
        scope.launch {
            delay(300) // simulate connection delay
            connected = true
            handler.onEvent(FreeqEvent.Connected)

            delay(200)
            handler.onEvent(FreeqEvent.Registered(nick))

            // If web token was set, simulate successful authentication
            webToken?.let {
                delay(100)
                handler.onEvent(FreeqEvent.Authenticated("did:plc:stub-${nick.lowercase()}"))
            }
        }
    }

    fun disconnect() {
        connected = false
        scope.launch {
            handler.onEvent(FreeqEvent.Disconnected("User disconnected"))
        }
        scope.cancel()
    }

    @Throws(FreeqError::class)
    fun join(channel: String) {
        if (!connected) throw FreeqError.NotConnected
        scope.launch {
            handler.onEvent(FreeqEvent.Joined(channel, nick))
            // Simulate a NAMES reply with some fake members
            delay(100)
            handler.onEvent(FreeqEvent.Names(channel, listOf(
                IrcMember(nick, isOp = false, isVoiced = false),
                IrcMember("alice", isOp = true, isVoiced = false),
                IrcMember("bob", isOp = false, isVoiced = true),
                IrcMember("carol", isOp = false, isVoiced = false),
            )))
            // Simulate topic
            delay(50)
            handler.onEvent(FreeqEvent.TopicChanged(channel, ChannelTopic(
                text = "Welcome to $channel!",
                setBy = "alice"
            )))
        }
    }

    @Throws(FreeqError::class)
    fun part(channel: String) {
        if (!connected) throw FreeqError.NotConnected
        scope.launch {
            handler.onEvent(FreeqEvent.Parted(channel, nick))
        }
    }

    @Throws(FreeqError::class)
    fun sendMessage(target: String, text: String) {
        if (!connected) throw FreeqError.NotConnected
        scope.launch {
            // Echo the message back as if the server reflected it
            val msgId = java.util.UUID.randomUUID().toString()
            handler.onEvent(FreeqEvent.Message(IrcMessage(
                fromNick = nick,
                target = target,
                text = text,
                msgid = msgId,
                replyTo = null,
                isAction = false,
                timestampMs = System.currentTimeMillis()
            )))
        }
    }

    @Throws(FreeqError::class)
    fun sendRaw(line: String) {
        // No-op in stub
    }

    @Throws(FreeqError::class)
    fun setTopic(channel: String, topic: String) {
        if (!connected) throw FreeqError.NotConnected
        scope.launch {
            handler.onEvent(FreeqEvent.TopicChanged(channel, ChannelTopic(topic, nick)))
        }
    }

    @Throws(FreeqError::class)
    fun nick(newNick: String) {
        nick = newNick
    }

    fun isConnected(): Boolean = connected

    fun currentNick(): String? = if (connected) nick else null
}
