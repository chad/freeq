import SwiftUI

struct ConnectView: View {
    @EnvironmentObject var appState: AppState
    @State private var nick: String = ""
    @State private var server: String = "irc.freeq.at:6667"

    var body: some View {
        VStack(spacing: 0) {
            Spacer()

            // Logo area
            VStack(spacing: 12) {
                Text("freeq")
                    .font(.system(size: 48, weight: .bold, design: .rounded))
                    .foregroundColor(.accentColor)

                Text("IRC + AT Protocol")
                    .font(.subheadline)
                    .foregroundColor(.secondary)
            }
            .padding(.bottom, 48)

            // Form
            VStack(spacing: 16) {
                VStack(alignment: .leading, spacing: 6) {
                    Text("NICKNAME")
                        .font(.caption)
                        .fontWeight(.bold)
                        .foregroundColor(.secondary)

                    TextField("Enter a nickname", text: $nick)
                        .textFieldStyle(.roundedBorder)
                        .font(.body)
                        .autocapitalization(.none)
                        .disableAutocorrection(true)
                        .textContentType(.username)
                }

                VStack(alignment: .leading, spacing: 6) {
                    Text("SERVER")
                        .font(.caption)
                        .fontWeight(.bold)
                        .foregroundColor(.secondary)

                    TextField("irc.freeq.at:6667", text: $server)
                        .textFieldStyle(.roundedBorder)
                        .font(.body)
                        .autocapitalization(.none)
                        .disableAutocorrection(true)
                        .keyboardType(.URL)
                }

                if let error = appState.errorMessage {
                    Text(error)
                        .font(.caption)
                        .foregroundColor(.red)
                        .multilineTextAlignment(.center)
                }
            }
            .padding(.horizontal, 32)

            Spacer().frame(height: 32)

            // Connect button
            Button(action: {
                appState.serverAddress = server
                appState.connect(nick: nick)
            }) {
                HStack {
                    if appState.connectionState == .connecting {
                        ProgressView()
                            .tint(.white)
                            .padding(.trailing, 4)
                    }
                    Text(appState.connectionState == .connecting ? "Connecting..." : "Connect")
                        .fontWeight(.semibold)
                }
                .frame(maxWidth: .infinity)
                .padding(.vertical, 14)
                .background(nick.isEmpty ? Color.gray : Color.accentColor)
                .foregroundColor(.white)
                .cornerRadius(12)
            }
            .disabled(nick.isEmpty || appState.connectionState == .connecting)
            .padding(.horizontal, 32)

            Spacer()
        }
        .background(Color(.systemBackground))
    }
}
