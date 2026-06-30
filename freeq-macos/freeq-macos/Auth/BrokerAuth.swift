import Foundation
import AuthenticationServices

/// Auth broker session response.
struct BrokerSession {
    let token: String
    let nick: String
    let did: String
    let handle: String
}

/// Handles AT Protocol OAuth via the auth broker.
enum BrokerAuth {
    /// Fetch a web-token from the broker using a stored broker token.
    static func fetchSession(brokerBase: String, brokerToken: String) async throws -> BrokerSession {
        guard let url = Validation.brokerSessionURL(brokerBase: brokerBase) else {
            throw BrokerError.sessionFailed("Invalid broker URL: \(brokerBase)")
        }
        var request = URLRequest(url: url)
        request.httpMethod = "POST"
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        request.httpBody = try JSONEncoder().encode(["broker_token": brokerToken])

        let (data, response) = try await URLSession.shared.data(for: request)
        guard let httpResponse = response as? HTTPURLResponse else {
            throw BrokerError.sessionFailed("Non-HTTP response from broker")
        }

        // Retry once on 502 (DPoP nonce rotation)
        if httpResponse.statusCode == 502 {
            let (retryData, retryResponse) = try await URLSession.shared.data(for: request)
            guard let retryHttp = retryResponse as? HTTPURLResponse else {
                throw BrokerError.sessionFailed("Non-HTTP retry response from broker")
            }
            guard retryHttp.statusCode == 200 else {
                if retryHttp.statusCode == 401 {
                    throw BrokerError.invalidToken
                }
                throw BrokerError.sessionFailed("Status \(retryHttp.statusCode)")
            }
            return try parseSession(retryData)
        }

        guard httpResponse.statusCode == 200 else {
            if httpResponse.statusCode == 401 {
                throw BrokerError.invalidToken
            }
            throw BrokerError.sessionFailed("Status \(httpResponse.statusCode)")
        }
        return try parseSession(data)
    }

    private static func parseSession(_ data: Data) throws -> BrokerSession {
        let json = try JSONSerialization.jsonObject(with: data) as? [String: Any] ?? [:]
        guard let token = json["token"] as? String,
              let nick = json["nick"] as? String,
              let did = json["did"] as? String else {
            throw BrokerError.sessionFailed("Invalid response")
        }
        return BrokerSession(
            token: token,
            nick: nick,
            did: did,
            handle: json["handle"] as? String ?? ""
        )
    }

    /// Start OAuth flow via the auth broker.
    /// Opens a browser window for AT Protocol login, receives callback.
    @MainActor
    static func startOAuth(brokerBase: String, handle: String) async throws -> (brokerToken: String?, session: BrokerSession) {
        let callbackScheme = "freeq"
        // return_to must match the active deployment's host; a hardcoded
        // host breaks login on any other deployment.
        guard let loginURL = Validation.brokerLoginURL(
            brokerBase: brokerBase,
            handle: handle,
            returnTo: "\(ServerConfig.apiBaseUrl)/auth/mobile"
        ) else {
            throw BrokerError.sessionFailed("Invalid broker URL or handle")
        }

        return try await withCheckedThrowingContinuation { continuation in
            let session = ASWebAuthenticationSession(
                url: loginURL,
                callbackURLScheme: callbackScheme
            ) { callbackURL, error in
                if let error {
                    continuation.resume(throwing: error)
                    return
                }
                guard let url = callbackURL,
                      let components = URLComponents(url: url, resolvingAgainstBaseURL: false) else {
                    continuation.resume(throwing: BrokerError.sessionFailed("No callback URL"))
                    return
                }
                let items = components.queryItems ?? []
                func value(_ name: String) -> String? { items.first(where: { $0.name == name })?.value }

                if let err = value("error") {
                    continuation.resume(throwing: BrokerError.sessionFailed(err))
                    return
                }

                // The broker completes via
                // freeq://auth?token=<web token>&nick=&did=[&broker_token=&handle=].
                // `token` (the web token) is what we connect with; `broker_token`
                // is optional — the standalone broker includes it (used to re-mint
                // web tokens on reconnect), the embedded broker does not. Mirrors
                // the iOS handler.
                guard let webToken = value("token"),
                      let nick = value("nick"),
                      let did = value("did") else {
                    continuation.resume(throwing: BrokerError.sessionFailed("Invalid auth response"))
                    return
                }

                let session = BrokerSession(
                    token: webToken,
                    nick: nick,
                    did: did,
                    handle: value("handle") ?? nick
                )
                continuation.resume(returning: (brokerToken: value("broker_token"), session: session))
            }
            session.prefersEphemeralWebBrowserSession = false

            // On macOS, we need a presentation context
            let provider = MacPresentationContextProvider()
            session.presentationContextProvider = provider
            session.start()

            // Keep provider alive
            objc_setAssociatedObject(session, "provider", provider, .OBJC_ASSOCIATION_RETAIN)
        }
    }
}

/// Provides the window for ASWebAuthenticationSession on macOS.
class MacPresentationContextProvider: NSObject, ASWebAuthenticationPresentationContextProviding {
    func presentationAnchor(for session: ASWebAuthenticationSession) -> ASPresentationAnchor {
        NSApplication.shared.keyWindow ?? ASPresentationAnchor()
    }
}

enum BrokerError: Error {
    case invalidToken
    case sessionFailed(String)
}
