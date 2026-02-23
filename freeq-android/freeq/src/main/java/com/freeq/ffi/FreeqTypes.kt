package com.freeq.ffi

// TODO: Replace with real UniFFI-generated bindings when native .so libs are integrated.
// These types match the freeq.udl interface definition exactly.

data class IrcMessage(
    val fromNick: String,
    val target: String,
    val text: String,
    val msgid: String?,
    val replyTo: String?,
    val isAction: Boolean,
    val timestampMs: Long
)

data class IrcMember(
    val nick: String,
    val isOp: Boolean,
    val isVoiced: Boolean
)

data class ChannelTopic(
    val text: String,
    val setBy: String?
)

sealed class FreeqEvent {
    object Connected : FreeqEvent()
    data class Registered(val nick: String) : FreeqEvent()
    data class Authenticated(val did: String) : FreeqEvent()
    data class AuthFailed(val reason: String) : FreeqEvent()
    data class Joined(val channel: String, val nick: String) : FreeqEvent()
    data class Parted(val channel: String, val nick: String) : FreeqEvent()
    data class Message(val msg: IrcMessage) : FreeqEvent()
    data class Names(val channel: String, val members: List<IrcMember>) : FreeqEvent()
    data class TopicChanged(val channel: String, val topic: ChannelTopic) : FreeqEvent()
    data class ModeChanged(val channel: String, val mode: String, val arg: String?, val setBy: String) : FreeqEvent()
    data class Kicked(val channel: String, val nick: String, val by: String, val reason: String) : FreeqEvent()
    data class UserQuit(val nick: String, val reason: String) : FreeqEvent()
    data class Notice(val text: String) : FreeqEvent()
    data class Disconnected(val reason: String) : FreeqEvent()
}

sealed class FreeqError : Exception() {
    object ConnectionFailed : FreeqError()
    object NotConnected : FreeqError()
    object SendFailed : FreeqError()
    object InvalidArgument : FreeqError()
}
