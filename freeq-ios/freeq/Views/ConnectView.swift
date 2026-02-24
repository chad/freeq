import SwiftUI
import AuthenticationServices

struct ConnectView: View {
    @EnvironmentObject var appState: AppState
    @State private var handle: String = UserDefaults.standard.string(forKey: "freeq.handle") ?? ""
    @State private var loading = false
    @State private var error: String? = nil
    @State private var showGuestLogin = false
    @State private var guestNick: String = ""
    @State private var guestServer: String = "irc.freeq.at:6667"
    @FocusState private var handleFocused: Bool
    @FocusState private var nickFocused: Bool

    private var keyboardActive: Bool { handleFocused || nickFocused }

    var body: some View {
        ZStack {
            // Background
            LinearGradient(
                colors: [Theme.bgPrimary, Color(hex: "0f0f1e")],
                startPoint: .top,
                endPoint: .bottom
            )
            .ignoresSafeArea()

            // Grid
            GeometryReader { geo in
                Path { path in
                    let spacing: CGFloat = 40
                    for x in stride(from: 0, through: geo.size.width, by: spacing) {
                        path.move(to: CGPoint(x: x, y: 0))
                        path.addLine(to: CGPoint(x: x, y: geo.size.height))
                    }
                    for y in stride(from: 0, through: geo.size.height, by: spacing) {
                        path.move(to: CGPoint(x: 0, y: y))
                        path.addLine(to: CGPoint(x: geo.size.width, y: y))
                    }
                }
                .stroke(Color.white.opacity(0.02), lineWidth: 0.5)
            }
            .ignoresSafeArea()

            ScrollViewReader { scrollProxy in
                ScrollView {
                    VStack(spacing: 0) {
                        Spacer(minLength: keyboardActive ? 20 : 60)

                        // Logo — shrinks and text fades when keyboard is up
                        VStack(spacing: keyboardActive ? 8 : 16) {
                            ZStack {
                                Circle()
                                    .fill(Theme.accent.opacity(0.15))
                                    .frame(width: keyboardActive ? 70 : 140, height: keyboardActive ? 70 : 140)
                                    .blur(radius: keyboardActive ? 15 : 30)

                                Image("FreeqLogo")
                                    .resizable()
                                    .scaledToFit()
                                    .frame(width: keyboardActive ? 48 : 100, height: keyboardActive ? 48 : 100)
                                    .shadow(color: Theme.accent.opacity(0.4), radius: keyboardActive ? 10 : 20)
                            }

                            VStack(spacing: 6) {
                                Text("freeq")
                                    .font(.system(size: keyboardActive ? 20 : 36, weight: .bold, design: .rounded))
                                    .foregroundColor(Theme.textPrimary)

                                Text("Decentralized chat")
                                    .font(.system(size: 15))
                                    .foregroundColor(Theme.textSecondary)
                                    .opacity(keyboardActive ? 0 : 1)
                                    .frame(height: keyboardActive ? 0 : nil)
                                    .clipped()
                            }
                        }
                        .animation(.easeInOut(duration: 0.3), value: keyboardActive)
                        .padding(.bottom, keyboardActive ? 16 : 40)

                        if !showGuestLogin {
                            // ── Primary: Bluesky Login ──
                            VStack(spacing: 20) {
                                // Handle input
                                VStack(alignment: .leading, spacing: 8) {
                                    Text("BLUESKY HANDLE")
                                        .font(.system(size: 11, weight: .bold))
                                        .foregroundColor(Theme.textMuted)
                                        .kerning(1)

                                    HStack(spacing: 10) {
                                        Text("@")
                                            .font(.system(size: 18, weight: .medium))
                                            .foregroundColor(Theme.textMuted)

                                        TextField("", text: $handle, prompt: Text("yourname.bsky.social").foregroundColor(Theme.textMuted))
                                            .foregroundColor(Theme.textPrimary)
                                            .font(.system(size: 16))
                                            .autocapitalization(.none)
                                            .disableAutocorrection(true)
                                            .keyboardType(.URL)
                                            .textContentType(.username)
                                            .focused($handleFocused)
                                            .submitLabel(.go)
                                            .onSubmit { startLogin() }
                                    }
                                    .padding(.horizontal, 14)
                                    .padding(.vertical, 12)
                                    .background(Theme.bgPrimary)
                                    .cornerRadius(10)
                                    .overlay(
                                        RoundedRectangle(cornerRadius: 10)
                                            .stroke(handleFocused ? Theme.accent : Theme.border, lineWidth: 1)
                                    )
                                    .id("handleField")
                                }

                                // Error
                                if let error = error {
                                    errorRow(error)
                                }

                                if let error = appState.errorMessage {
                                    errorRow(error)
                                }

                                // Sign in button
                                Button(action: startLogin) {
                                    HStack(spacing: 8) {
                                        if loading || appState.connectionState == .connecting {
                                            ProgressView().tint(.white).scaleEffect(0.85)
                                        } else {
                                            Image(systemName: "person.badge.key.fill")
                                                .font(.system(size: 14))
                                        }
                                        Text(loading ? "Authenticating..." : appState.connectionState == .connecting ? "Connecting..." : "Sign in with Bluesky")
                                            .font(.system(size: 16, weight: .semibold))
                                    }
                                    .frame(maxWidth: .infinity)
                                    .padding(.vertical, 14)
                                    .background(
                                        handle.isEmpty || loading
                                            ? AnyShapeStyle(Theme.textMuted.opacity(0.3))
                                            : AnyShapeStyle(LinearGradient(colors: [Theme.accent, Theme.accentLight], startPoint: .leading, endPoint: .trailing))
                                    )
                                    .foregroundColor(.white)
                                    .cornerRadius(10)
                                }
                                .disabled(handle.isEmpty || loading || appState.connectionState == .connecting)
                            }
                            .padding(24)
                            .background(Theme.bgSecondary)
                            .cornerRadius(16)
                            .overlay(
                                RoundedRectangle(cornerRadius: 16)
                                    .stroke(Theme.border, lineWidth: 1)
                            )
                            .padding(.horizontal, 24)

                            // Guest option
                            Button(action: { withAnimation { showGuestLogin = true } }) {
                                Text("Continue as guest")
                                    .font(.system(size: 14))
                                    .foregroundColor(Theme.textMuted)
                            }
                            .padding(.top, 20)

                        } else {
                            // ── Guest Login ──
                            VStack(spacing: 20) {
                                VStack(alignment: .leading, spacing: 8) {
                                    Text("NICKNAME")
                                        .font(.system(size: 11, weight: .bold))
                                        .foregroundColor(Theme.textMuted)
                                        .kerning(1)

                                    HStack(spacing: 10) {
                                        Image(systemName: "person.fill")
                                            .foregroundColor(Theme.textMuted)
                                            .font(.system(size: 14))

                                        TextField("", text: $guestNick, prompt: Text("Choose a nickname").foregroundColor(Theme.textMuted))
                                            .foregroundColor(Theme.textPrimary)
                                            .font(.system(size: 16))
                                            .autocapitalization(.none)
                                            .disableAutocorrection(true)
                                            .textContentType(.username)
                                            .focused($nickFocused)
                                            .submitLabel(.go)
                                            .onSubmit { connectAsGuest() }
                                    }
                                    .padding(.horizontal, 14)
                                    .padding(.vertical, 12)
                                    .background(Theme.bgPrimary)
                                    .cornerRadius(10)
                                    .overlay(
                                        RoundedRectangle(cornerRadius: 10)
                                            .stroke(nickFocused ? Theme.accent : Theme.border, lineWidth: 1)
                                    )
                                    .id("nickField")
                                }

                                if let error = appState.errorMessage {
                                    errorRow(error)
                                }

                                Button(action: connectAsGuest) {
                                    HStack(spacing: 8) {
                                        if appState.connectionState == .connecting {
                                            ProgressView().tint(.white).scaleEffect(0.85)
                                        }
                                        Text(appState.connectionState == .connecting ? "Connecting..." : "Connect as Guest")
                                            .font(.system(size: 16, weight: .semibold))
                                    }
                                    .frame(maxWidth: .infinity)
                                    .padding(.vertical, 14)
                                    .background(
                                        guestNick.isEmpty
                                            ? AnyShapeStyle(Theme.textMuted.opacity(0.3))
                                            : AnyShapeStyle(LinearGradient(colors: [Theme.accent, Theme.accentLight], startPoint: .leading, endPoint: .trailing))
                                    )
                                    .foregroundColor(.white)
                                    .cornerRadius(10)
                                }
                                .disabled(guestNick.isEmpty || appState.connectionState == .connecting)
                            }
                            .padding(24)
                            .background(Theme.bgSecondary)
                            .cornerRadius(16)
                            .overlay(
                                RoundedRectangle(cornerRadius: 16)
                                    .stroke(Theme.border, lineWidth: 1)
                            )
                            .padding(.horizontal, 24)

                            // Back to Bluesky login
                            Button(action: { withAnimation { showGuestLogin = false } }) {
                                HStack(spacing: 4) {
                                    Image(systemName: "arrow.left")
                                        .font(.system(size: 12))
                                    Text("Sign in with Bluesky instead")
                                        .font(.system(size: 14))
                                }
                                .foregroundColor(Theme.accent)
                            }
                            .padding(.top, 20)
                        }

                        // Extra padding so keyboard doesn't cover the field
                        Color.clear.frame(height: 120).id("bottomPadding")

                        // Footer
                        Text("Open source · IRC compatible · AT Protocol identity")
                            .font(.system(size: 11))
                            .foregroundColor(Theme.textMuted)
                            .padding(.bottom, 16)
                    }
                }
                .scrollDismissesKeyboard(.interactively)
                .onChange(of: handleFocused) {
                    if handleFocused {
                        DispatchQueue.main.asyncAfter(deadline: .now() + 0.3) {
                            withAnimation { scrollProxy.scrollTo("handleField", anchor: .center) }
                        }
                    }
                }
                .onChange(of: nickFocused) {
                    if nickFocused {
                        DispatchQueue.main.asyncAfter(deadline: .now() + 0.3) {
                            withAnimation { scrollProxy.scrollTo("nickField", anchor: .center) }
                        }
                    }
                }
            }
        }
        .onTapGesture {
            handleFocused = false
            nickFocused = false
        }
        .preferredColorScheme(.dark)
        // No auto-login here — ContentView handles session persistence
        // ConnectView only shows when user needs to explicitly log in
    }

    private func errorRow(_ text: String) -> some View {
        HStack(spacing: 6) {
            Image(systemName: "exclamationmark.triangle.fill")
                .font(.system(size: 12))
                .foregroundColor(Theme.danger)
            Text(text)
                .font(.system(size: 13))
                .foregroundColor(Theme.danger)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    private func startLogin() {
        guard !handle.isEmpty else { return }
        loading = true
        error = nil

        let serverBase = appState.authBrokerBase
        let loginURL = "\(serverBase)/auth/login?handle=\(handle.addingPercentEncoding(withAllowedCharacters: .urlQueryAllowed) ?? handle)&mobile=1"

        guard let url = URL(string: loginURL) else {
            error = "Invalid handle"
            loading = false
            return
        }

        let session = ASWebAuthenticationSession(url: url, callback: .customScheme("freeq")) { callbackURL, err in
            DispatchQueue.main.async {
                loading = false

                if let err = err {
                    if (err as NSError).code == ASWebAuthenticationSessionError.canceledLogin.rawValue {
                        return
                    }
                    error = "Login failed: \(err.localizedDescription)"
                    return
                }

                guard let callbackURL = callbackURL,
                      let components = URLComponents(url: callbackURL, resolvingAgainstBaseURL: false),
                      let token = components.queryItems?.first(where: { $0.name == "token" })?.value,
                      let brokerToken = components.queryItems?.first(where: { $0.name == "broker_token" })?.value,
                      let nick = components.queryItems?.first(where: { $0.name == "nick" })?.value,
                      let did = components.queryItems?.first(where: { $0.name == "did" })?.value
                else {
                    error = "Invalid response from server"
                    return
                }

                // Save handle + login time for session persistence
                UserDefaults.standard.set(handle, forKey: "freeq.handle")
                UserDefaults.standard.set(Date().timeIntervalSince1970, forKey: "freeq.lastLogin")
                UserDefaults.standard.set(brokerToken, forKey: "freeq.brokerToken")

                appState.pendingWebToken = token
                appState.brokerToken = brokerToken
                appState.authenticatedDID = did
                appState.serverAddress = "irc.freeq.at:6667"
                appState.connect(nick: nick)
            }
        }

        session.presentationContextProvider = ASPresentationContextProvider.shared
        session.prefersEphemeralWebBrowserSession = false
        session.start()
    }

    private func connectAsGuest() {
        appState.serverAddress = guestServer
        appState.connect(nick: guestNick)
    }
}

/// Provides the window for ASWebAuthenticationSession.
class ASPresentationContextProvider: NSObject, ASWebAuthenticationPresentationContextProviding {
    static let shared = ASPresentationContextProvider()

    func presentationAnchor(for session: ASWebAuthenticationSession) -> ASPresentationAnchor {
        // Try to find the key window from connected scenes
        for scene in UIApplication.shared.connectedScenes {
            if let windowScene = scene as? UIWindowScene {
                // Prefer the key window
                if let keyWindow = windowScene.windows.first(where: { $0.isKeyWindow }) {
                    return keyWindow
                }
                // Fall back to any window
                if let window = windowScene.windows.first {
                    return window
                }
            }
        }
        // Last resort — create a new window (may cause error 3)
        return ASPresentationAnchor()
    }
}
