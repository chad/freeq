import Foundation

/// Server + auth-broker config, injected via `IRC_SERVER` / `AUTH_BROKER_BASE`
/// (a scheme env var or an Info.plist build setting); defaults to freeq.at.
enum ServerConfig {
    /// `host:port` of the IRC server. Default: freeq.at.
    static let ircServer = configValue("IRC_SERVER") ?? "irc.freeq.at:6697"

    /// Base URL of the auth broker. freeq.at uses a standalone broker
    /// (`auth.freeq.at`); embedded-broker deployments (e.g. zerosum) set
    /// this to their own server origin.
    static let authBrokerBase = configValue("AUTH_BROKER_BASE") ?? "https://auth.freeq.at"

    /// Host portion of `ircServer`, without the port.
    static var host: String {
        ircServer.split(separator: ":").first.map(String.init) ?? ircServer
    }

    /// HTTPS base for the server's own API — used for the OAuth
    /// `return_to` bridge and REST calls. For embedded-broker
    /// deployments this equals `authBrokerBase`.
    static var apiBaseUrl: String { "https://\(host)" }

    /// Stable identifier for the active deployment (server + broker).
    /// Lets the app notice when a build has been retargeted at a
    /// different host so it can drop stale auth/session state — the
    /// freeq.at and zerosum builds share a bundle id (and therefore a
    /// keychain) when distinguished only by build configuration.
    static var deploymentID: String { "\(ircServer)|\(authBrokerBase)" }

    /// Resolves a config value: scheme/process env var first (how the
    /// per-deployment Run schemes set it), then the Info.plist build
    /// setting. Returns nil for missing/empty values or an unsubstituted
    /// `$(VAR)` placeholder, so an un-wired build falls back to freeq.at.
    private static func configValue(_ key: String) -> String? {
        if let env = ProcessInfo.processInfo.environment[key], !env.isEmpty {
            return env
        }
        guard let value = Bundle.main.object(forInfoDictionaryKey: key) as? String,
              !value.isEmpty, !value.hasPrefix("$(") else { return nil }
        return value
    }
}
