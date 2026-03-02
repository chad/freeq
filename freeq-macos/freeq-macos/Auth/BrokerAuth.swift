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
        let url = URL(string: "\(brokerBase)/session")!
        var request = URLRequest(url: url)
        request.httpMethod = "POST"
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        request.httpBody = try JSONEncoder().encode(["broker_token": brokerToken])

        let (data, response) = try await URLSession.shared.data(for: request)
        let httpResponse = response as! HTTPURLResponse

        // Retry once on 502 (DPoP nonce rotation)
        if httpResponse.statusCode == 502 {
            let (retryData, retryResponse) = try await URLSession.shared.data(for: request)
            let retryHttp = retryResponse as! HTTPURLResponse
            guard retryHttp.statusCode == 200 else {
                throw BrokerError.sessionFailed("Status \(retryHttp.statusCode)")
            }
            return try parseSession(retryData)
        }

        guard httpResponse.statusCode == 200 else {
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
    static func startOAuth(brokerBase: String, handle: String) async throws -> (brokerToken: String, session: BrokerSession) {
        let callbackScheme = "freeq"
        let encodedHandle = handle.addingPercentEncoding(withAllowedCharacters: .urlQueryAllowed) ?? handle
        let returnTo = "https://irc.freeq.at/auth/mobile".addingPercentEncoding(withAllowedCharacters: .urlQueryAllowed) ?? ""
        let loginURL = URL(string: "\(brokerBase)/auth/login?handle=\(encodedHandle)&return_to=\(returnTo)&popup=1")!

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
                      let components = URLComponents(url: url, resolvingAgainstBaseURL: false),
                      let token = components.queryItems?.first(where: { $0.name == "broker_token" })?.value,
                      let did = components.queryItems?.first(where: { $0.name == "did" })?.value,
                      let nick = components.queryItems?.first(where: { $0.name == "nick" })?.value else {
                    continuation.resume(throwing: BrokerError.sessionFailed("No token in callback"))
                    return
                }

                let handle = components.queryItems?.first(where: { $0.name == "handle" })?.value ?? ""

                // Now fetch a web-token using the broker token
                Task {
                    do {
                        let session = try await fetchSession(brokerBase: brokerBase, brokerToken: token)
                        continuation.resume(returning: (brokerToken: token, session: session))
                    } catch {
                        // Return what we have from the callback
                        let session = BrokerSession(token: "", nick: nick, did: did, handle: handle)
                        continuation.resume(returning: (brokerToken: token, session: session))
                    }
                }
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
    case sessionFailed(String)
}
