import SwiftUI
import AuthenticationServices

/// AT Protocol login via OAuth using ASWebAuthenticationSession.
/// Flow: user enters handle → opens system browser → OAuth → callback → web-token → SASL auth
struct ATLoginSheet: View {
    @EnvironmentObject var appState: AppState
    @Environment(\.dismiss) var dismiss
    @State private var handle: String = ""
    @State private var loading = false
    @State private var error: String? = nil
    @FocusState private var focused: Bool

    var body: some View {
        NavigationView {
            ZStack {
                Theme.bgPrimary.ignoresSafeArea()

                VStack(spacing: 24) {
                    // Bluesky logo area
                    VStack(spacing: 12) {
                        Image(systemName: "person.badge.key.fill")
                            .font(.system(size: 40))
                            .foregroundColor(Theme.accent)

                        Text("Sign in with AT Protocol")
                            .font(.system(size: 18, weight: .semibold))
                            .foregroundColor(Theme.textPrimary)

                        Text("Use your Bluesky handle to authenticate.\nYour identity is verified cryptographically.")
                            .font(.system(size: 13))
                            .foregroundColor(Theme.textSecondary)
                            .multilineTextAlignment(.center)
                            .padding(.horizontal, 20)
                    }
                    .padding(.top, 16)

                    // Handle input
                    VStack(alignment: .leading, spacing: 8) {
                        Text("AT HANDLE")
                            .font(.system(size: 11, weight: .bold))
                            .foregroundColor(Theme.textMuted)
                            .kerning(1)

                        HStack(spacing: 10) {
                            Text("@")
                                .font(.system(size: 18, weight: .medium))
                                .foregroundColor(Theme.textMuted)

                            TextField("", text: $handle, prompt: Text("alice.bsky.social").foregroundColor(Theme.textMuted))
                                .foregroundColor(Theme.textPrimary)
                                .font(.system(size: 16))
                                .autocapitalization(.none)
                                .disableAutocorrection(true)
                                .keyboardType(.URL)
                                .focused($focused)
                                .onSubmit { startLogin() }
                        }
                        .padding(.horizontal, 14)
                        .padding(.vertical, 12)
                        .background(Theme.bgTertiary)
                        .cornerRadius(10)
                        .overlay(
                            RoundedRectangle(cornerRadius: 10)
                                .stroke(focused ? Theme.accent : Theme.border, lineWidth: 1)
                        )
                    }
                    .padding(.horizontal, 20)

                    if let error = error {
                        HStack(spacing: 6) {
                            Image(systemName: "exclamationmark.triangle.fill")
                                .font(.system(size: 12))
                                .foregroundColor(Theme.danger)
                            Text(error)
                                .font(.system(size: 13))
                                .foregroundColor(Theme.danger)
                        }
                        .padding(.horizontal, 20)
                    }

                    // Login button
                    Button(action: startLogin) {
                        HStack(spacing: 8) {
                            if loading {
                                ProgressView().tint(.white).scaleEffect(0.85)
                            }
                            Text(loading ? "Authenticating..." : "Sign In")
                                .font(.system(size: 16, weight: .semibold))
                        }
                        .frame(maxWidth: .infinity)
                        .padding(.vertical, 14)
                        .background(
                            handle.isEmpty || loading
                                ? AnyShapeStyle(Theme.textMuted.opacity(0.3))
                                : AnyShapeStyle(Theme.accent)
                        )
                        .foregroundColor(.white)
                        .cornerRadius(10)
                    }
                    .disabled(handle.isEmpty || loading)
                    .padding(.horizontal, 20)

                    Spacer()
                }
            }
            .navigationTitle("AT Protocol Login")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") { dismiss() }
                        .foregroundColor(Theme.accent)
                }
            }
            .toolbarBackground(Theme.bgSecondary, for: .navigationBar)
            .toolbarBackground(.visible, for: .navigationBar)
        }
        .onAppear { focused = true }
        .preferredColorScheme(.dark)
    }

    private func startLogin() {
        guard !handle.isEmpty else { return }
        loading = true
        error = nil

        // The server's /auth/login endpoint handles the OAuth flow.
        // We use ASWebAuthenticationSession to open the browser and get the callback.
        let serverBase = appState.serverAddress.contains(":6667")
            ? "https://irc.freeq.at"  // Production
            : "http://127.0.0.1:8080" // Local dev

        let loginURL = "\(serverBase)/auth/login?handle=\(handle.addingPercentEncoding(withAllowedCharacters: .urlQueryAllowed) ?? handle)&mobile=1"

        guard let url = URL(string: loginURL) else {
            error = "Invalid handle"
            loading = false
            return
        }

        // ASWebAuthenticationSession opens Safari sheet, handles redirect
        let session = ASWebAuthenticationSession(url: url, callback: .customScheme("freeq")) { callbackURL, err in
            DispatchQueue.main.async {
                loading = false

                if let err = err {
                    if (err as NSError).code == ASWebAuthenticationSessionError.canceledLogin.rawValue {
                        // User cancelled — not an error
                        return
                    }
                    error = "Login failed: \(err.localizedDescription)"
                    return
                }

                guard let callbackURL = callbackURL,
                      let components = URLComponents(url: callbackURL, resolvingAgainstBaseURL: false),
                      let token = components.queryItems?.first(where: { $0.name == "token" })?.value,
                      let nick = components.queryItems?.first(where: { $0.name == "nick" })?.value,
                      let did = components.queryItems?.first(where: { $0.name == "did" })?.value
                else {
                    error = "Invalid response from server"
                    return
                }

                // Connect with the web-token for SASL auth
                appState.pendingWebToken = token
                appState.authenticatedDID = did
                appState.serverAddress = serverBase.contains("127.0.0.1") ? "127.0.0.1:6667" : "irc.freeq.at:6667"
                appState.connect(nick: nick)
                dismiss()
            }
        }

        session.presentationContextProvider = ASPresentationContextProvider.shared
        session.prefersEphemeralWebBrowserSession = false
        session.start()
    }
}

/// Provides the window for ASWebAuthenticationSession.
class ASPresentationContextProvider: NSObject, ASWebAuthenticationPresentationContextProviding {
    static let shared = ASPresentationContextProvider()

    func presentationAnchor(for session: ASWebAuthenticationSession) -> ASPresentationAnchor {
        guard let scene = UIApplication.shared.connectedScenes.first as? UIWindowScene,
              let window = scene.windows.first else {
            return ASPresentationAnchor()
        }
        return window
    }
}
