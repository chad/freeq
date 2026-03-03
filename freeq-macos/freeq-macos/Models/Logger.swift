import os

/// Structured logging for freeq macOS app.
enum Log {
    static let irc = os.Logger(subsystem: "at.freeq.macos", category: "irc")
    static let auth = os.Logger(subsystem: "at.freeq.macos", category: "auth")
    static let p2p = os.Logger(subsystem: "at.freeq.macos", category: "p2p")
    static let ui = os.Logger(subsystem: "at.freeq.macos", category: "ui")
    static let media = os.Logger(subsystem: "at.freeq.macos", category: "media")
    static let profile = os.Logger(subsystem: "at.freeq.macos", category: "profile")
}
