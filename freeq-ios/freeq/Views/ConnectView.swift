import SwiftUI

struct ConnectView: View {
    @EnvironmentObject var appState: AppState
    @State private var nick: String = ""
    @State private var server: String = "irc.freeq.at:6667"
    @FocusState private var nickFocused: Bool
    @State private var showATLogin = false

    var body: some View {
        ZStack {
            // Background gradient
            LinearGradient(
                colors: [Theme.bgPrimary, Color(hex: "0f0f1e")],
                startPoint: .top,
                endPoint: .bottom
            )
            .ignoresSafeArea()

            // Subtle grid pattern overlay
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

            VStack(spacing: 0) {
                Spacer()

                // Logo + tagline
                VStack(spacing: 16) {
                    // Glow circle behind logo
                    ZStack {
                        Circle()
                            .fill(Theme.accent.opacity(0.15))
                            .frame(width: 100, height: 100)
                            .blur(radius: 30)

                        Text("fq")
                            .font(.system(size: 44, weight: .black, design: .rounded))
                            .foregroundColor(Theme.accent)
                    }

                    Text("freeq")
                        .font(.system(size: 36, weight: .bold, design: .rounded))
                        .foregroundColor(Theme.textPrimary)

                    Text("Decentralized chat powered by IRC + AT Protocol")
                        .font(.system(size: 14))
                        .foregroundColor(Theme.textSecondary)
                        .multilineTextAlignment(.center)
                        .padding(.horizontal, 40)
                }
                .padding(.bottom, 44)

                // Card
                VStack(spacing: 20) {
                    // Nickname field
                    VStack(alignment: .leading, spacing: 8) {
                        Text("NICKNAME")
                            .font(.system(size: 11, weight: .bold))
                            .foregroundColor(Theme.textMuted)
                            .kerning(1)

                        HStack(spacing: 10) {
                            Image(systemName: "person.fill")
                                .foregroundColor(Theme.textMuted)
                                .font(.system(size: 14))

                            TextField("", text: $nick, prompt: Text("Choose a nickname").foregroundColor(Theme.textMuted))
                                .foregroundColor(Theme.textPrimary)
                                .font(.system(size: 16))
                                .autocapitalization(.none)
                                .disableAutocorrection(true)
                                .textContentType(.username)
                                .focused($nickFocused)
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

                    // Server field
                    VStack(alignment: .leading, spacing: 8) {
                        Text("SERVER")
                            .font(.system(size: 11, weight: .bold))
                            .foregroundColor(Theme.textMuted)
                            .kerning(1)

                        HStack(spacing: 10) {
                            Image(systemName: "server.rack")
                                .foregroundColor(Theme.textMuted)
                                .font(.system(size: 14))

                            TextField("", text: $server, prompt: Text("irc.freeq.at:6667").foregroundColor(Theme.textMuted))
                                .foregroundColor(Theme.textPrimary)
                                .font(.system(size: 16))
                                .autocapitalization(.none)
                                .disableAutocorrection(true)
                                .keyboardType(.URL)
                        }
                        .padding(.horizontal, 14)
                        .padding(.vertical, 12)
                        .background(Theme.bgPrimary)
                        .cornerRadius(10)
                        .overlay(
                            RoundedRectangle(cornerRadius: 10)
                                .stroke(Theme.border, lineWidth: 1)
                        )
                    }

                    // Error
                    if let error = appState.errorMessage {
                        HStack(spacing: 8) {
                            Image(systemName: "exclamationmark.triangle.fill")
                                .foregroundColor(Theme.danger)
                                .font(.system(size: 12))
                            Text(error)
                                .font(.system(size: 13))
                                .foregroundColor(Theme.danger)
                        }
                        .frame(maxWidth: .infinity, alignment: .leading)
                    }

                    // Connect button
                    Button(action: {
                        appState.serverAddress = server
                        appState.connect(nick: nick)
                    }) {
                        HStack(spacing: 8) {
                            if appState.connectionState == .connecting {
                                ProgressView()
                                    .tint(.white)
                                    .scaleEffect(0.85)
                            }
                            Text(appState.connectionState == .connecting ? "Connecting..." : "Connect")
                                .font(.system(size: 16, weight: .semibold))
                        }
                        .frame(maxWidth: .infinity)
                        .padding(.vertical, 14)
                        .background(
                            nick.isEmpty
                                ? AnyShapeStyle(Theme.textMuted.opacity(0.3))
                                : AnyShapeStyle(LinearGradient(colors: [Theme.accent, Theme.accentLight], startPoint: .leading, endPoint: .trailing))
                        )
                        .foregroundColor(.white)
                        .cornerRadius(10)
                    }
                    .disabled(nick.isEmpty || appState.connectionState == .connecting)
                }
                .padding(24)
                .background(Theme.bgSecondary)
                .cornerRadius(16)
                .overlay(
                    RoundedRectangle(cornerRadius: 16)
                        .stroke(Theme.border, lineWidth: 1)
                )
                .padding(.horizontal, 24)

                Spacer()

                // AT Protocol login
                VStack(spacing: 8) {
                    HStack {
                        Rectangle().fill(Theme.border).frame(height: 1)
                        Text("or")
                            .font(.system(size: 12))
                            .foregroundColor(Theme.textMuted)
                        Rectangle().fill(Theme.border).frame(height: 1)
                    }
                    .padding(.horizontal, 40)

                    Button(action: { showATLogin = true }) {
                        HStack(spacing: 8) {
                            Image(systemName: "person.badge.key.fill")
                                .font(.system(size: 14))
                            Text("Sign in with Bluesky")
                                .font(.system(size: 15, weight: .medium))
                        }
                        .frame(maxWidth: .infinity)
                        .padding(.vertical, 12)
                        .background(Color.clear)
                        .foregroundColor(Theme.accent)
                        .overlay(
                            RoundedRectangle(cornerRadius: 10)
                                .stroke(Theme.accent, lineWidth: 1.5)
                        )
                        .cornerRadius(10)
                    }
                    .padding(.horizontal, 24)
                }
                .padding(.top, 12)

                Spacer()

                // Footer
                Text("Open source · IRC compatible · AT Protocol identity")
                    .font(.system(size: 11))
                    .foregroundColor(Theme.textMuted)
                    .padding(.bottom, 16)
            }
        }
        .preferredColorScheme(.dark)
        .sheet(isPresented: $showATLogin) {
            ATLoginSheet()
                .presentationDetents([.medium, .large])
        }
    }
}
