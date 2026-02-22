import SwiftUI
import AuthenticationServices

struct ConnectView: View {
    @EnvironmentObject var appState: AppState
    @State private var handle: String = ""
    @State private var loading = false
    @State private var error: String? = nil
    @State private var showGuestLogin = false
    @State private var guestNick: String = ""
    @State private var guestServer: String = "irc.freeq.at:6667"
    @FocusState private var handleFocused: Bool
    @FocusState private var nickFocused: Bool

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

            ScrollView {
                VStack(spacing: 0) {
                    Spacer(minLength: 60)

                    // Logo
                    VStack(spacing: 16) {
                        ZStack {
                            Circle()
                                .fill(Theme.accent.opacity(0.15))
                                .frame(width: 120, height: 120)
                                .blur(radius: 30)

                            Image("FreeqLogo")
                                .resizable()
                                .scaledToFit()
                                .frame(width: 80, height: 80)
                                .clipShape(RoundedRectangle(cornerRadius: 18))
                                .shadow(color: Theme.accent.opacity(0.3), radius: 20)
                        }

                        VStack(spacing: 6) {
                            Text("freeq")
                                .font(.system(size: 36, weight: .bold, design: .rounded))
                                .foregroundColor(Theme.textPrimary)

                            Text("Decentralized chat")
                                .font(.system(size: 15))
                                .foregroundColor(Theme.textSecondary)
                        }
                    }
                    .padding(.bottom, 40)

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
                                HStack(spacing: 6) {
                                    Image(systemName: "exclamationmark.triangle.fill")
                                        .font(.system(size: 12))
                                        .foregroundColor(Theme.danger)
                                    Text(error)
                                        .font(.system(size: 13))
                                        .foregroundColor(Theme.danger)
                                }
                                .frame(maxWidth: .infinity, alignment: .leading)
                            }

                            if let error = appState.errorMessage {
                                HStack(spacing: 6) {
                                    Image(systemName: "exclamationmark.triangle.fill")
                                        .font(.system(size: 12))
                                        .foregroundColor(Theme.danger)
                                    Text(error)
                                        .font(.system(size: 13))
                                        .foregroundColor(Theme.danger)
                                }
                                .frame(maxWidth: .infinity, alignment: .leading)
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
                            }

                            if let error = appState.errorMessage {
                                HStack(spacing: 6) {
                                    Image(systemName: "exclamationmark.triangle.fill")
                                        .font(.system(size: 12))
                                        .foregroundColor(Theme.danger)
                                    Text(error)
                                        .font(.system(size: 13))
                                        .foregroundColor(Theme.danger)
                                }
                                .frame(maxWidth: .infinity, alignment: .leading)
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

                    Spacer(minLength: 40)

                    // Footer
                    Text("Open source · IRC compatible · AT Protocol identity")
                        .font(.system(size: 11))
                        .foregroundColor(Theme.textMuted)
                        .padding(.bottom, 16)
                }
                .frame(minHeight: UIScreen.main.bounds.height)
            }
            .scrollDismissesKeyboard(.interactively)
        }
        .onTapGesture {
            handleFocused = false
            nickFocused = false
        }
        .preferredColorScheme(.dark)
    }

    private func startLogin() {
        guard !handle.isEmpty else { return }
        loading = true
        error = nil

        let serverBase = "https://irc.freeq.at"
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
                      let nick = components.queryItems?.first(where: { $0.name == "nick" })?.value,
                      let did = components.queryItems?.first(where: { $0.name == "did" })?.value
                else {
                    error = "Invalid response from server"
                    return
                }

                appState.pendingWebToken = token
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
        guard let scene = UIApplication.shared.connectedScenes.first as? UIWindowScene,
              let window = scene.windows.first else {
            return ASPresentationAnchor()
        }
        return window
    }
}
