package com.freeq.model

/** Central server configuration - change here to point to a different server */
object ServerConfig {
    /** IRC server host:port (default: production) */
    var ircServer: String = "irc.freeq.at:6667"

    /** HTTPS API base URL (derived from ircServer) */
    val apiBaseUrl: String
        get() = "https://" + ircServer.substringBefore(":")

    /** Secure WebSocket IRC URL (derived from ircServer host). */
    val wssServer: String
        get() = "wss://" + ircServer.substringBefore(":") + "/irc"

    /** Auth broker base URL (default: production standalone broker) */
    // For deployments using embedded auth (no standalone broker), use apiBaseUrl:
    // val authBrokerBase: String get() = apiBaseUrl
    val authBrokerBase: String = "https://auth.freeq.at"
}
