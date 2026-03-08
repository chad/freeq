import SwiftUI

struct ConnectView: View {
    @Environment(AppState.self) private var appState
    @State private var handle: String = ""
    @State private var guestNick: String = ""
    @State private var isLoggingIn = false

    var body: some View {
        VStack(spacing: 32) {
            Spacer()

            // Logo area
            VStack(spacing: 12) {
                Image(systemName: "bubble.left.and.bubble.right.fill")
                    .font(.system(size: 56))
                    .foregroundStyle(.tint)

                Text("freeq")
                    .font(.system(size: 36, weight: .bold, design: .rounded))

                Text("IRC with AT Protocol identity")
                    .foregroundStyle(.secondary)
            }

            // AT Protocol Login
            VStack(spacing: 12) {
                Text("Sign in with your AT Protocol handle")
                    .font(.subheadline)
                    .foregroundStyle(.secondary)

                TextField("handle (e.g. alice.bsky.social)", text: $handle)
                    .textFieldStyle(.roundedBorder)
                    .frame(width: 280)
                    .onSubmit { startLogin() }

                Button {
                    startLogin()
                } label: {
                    HStack {
                        if isLoggingIn {
                            ProgressView()
                                .scaleEffect(0.7)
                        } else {
                            Image(systemName: "person.badge.key.fill")
                        }
                        Text("Sign In")
                    }
                    .frame(maxWidth: 280)
                }
                .buttonStyle(.borderedProminent)
                .controlSize(.large)
                .disabled(handle.isEmpty || isLoggingIn)
            }

            // Divider
            HStack {
                Rectangle().fill(.separator).frame(height: 1)
                Text("or").font(.caption).foregroundStyle(.tertiary)
                Rectangle().fill(.separator).frame(height: 1)
            }
            .frame(width: 280)

            // Guest connect
            HStack(spacing: 8) {
                TextField("Nickname", text: $guestNick)
                    .textFieldStyle(.roundedBorder)
                    .frame(width: 160)
                    .onSubmit { connectGuest() }

                Button("Connect as Guest") {
                    connectGuest()
                }
                .disabled(guestNick.isEmpty)
            }

            if let error = appState.errorMessage {
                Text(error)
                    .font(.caption)
                    .foregroundStyle(.red)
                    .multilineTextAlignment(.center)
                    .frame(width: 300)
            }

            Spacer()
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(Color(nsColor: .windowBackgroundColor))
    }

    private func startLogin() {
        guard !handle.isEmpty else { return }
        isLoggingIn = true
        appState.errorMessage = nil

        Task {
            do {
                let (brokerToken, session) = try await BrokerAuth.startOAuth(
                    brokerBase: appState.authBrokerBase,
                    handle: handle
                )
                appState.brokerToken = brokerToken
                KeychainHelper.save(key: "brokerToken", value: brokerToken)
                appState.pendingWebToken = session.token
                appState.authenticatedDID = session.did
                KeychainHelper.save(key: "did", value: session.did)
                appState.connect(nick: session.nick)
            } catch {
                appState.errorMessage = "Login failed: \(error.localizedDescription)"
            }
            isLoggingIn = false
        }
    }

    private func connectGuest() {
        guard !guestNick.isEmpty else { return }
        appState.connect(nick: guestNick)
    }
}
