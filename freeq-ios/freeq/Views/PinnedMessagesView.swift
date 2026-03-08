import SwiftUI

struct PinnedMessage: Identifiable {
    let id: String  // msgid
    let from: String
    let text: String
    let timestamp: Date
    let pinnedBy: String
}

struct PinnedMessagesView: View {
    @EnvironmentObject var appState: AppState
    let channelName: String

    @State private var pins: [PinnedMessage] = []
    @State private var loading = true
    @State private var error: String? = nil

    var body: some View {
        ZStack {
            Theme.bgPrimary.ignoresSafeArea()

            if loading {
                VStack(spacing: 12) {
                    ProgressView().tint(Theme.accent)
                    Text("Loading pins…")
                        .font(.system(size: 14))
                        .foregroundColor(Theme.textMuted)
                }
            } else if let error = error {
                VStack(spacing: 12) {
                    Image(systemName: "exclamationmark.triangle")
                        .font(.system(size: 32))
                        .foregroundColor(Theme.warning)
                    Text(error)
                        .font(.system(size: 14))
                        .foregroundColor(Theme.textSecondary)
                }
            } else if pins.isEmpty {
                VStack(spacing: 12) {
                    Image(systemName: "pin.slash")
                        .font(.system(size: 36))
                        .foregroundColor(Theme.textMuted)
                    Text("No pinned messages")
                        .font(.system(size: 16, weight: .medium))
                        .foregroundColor(Theme.textSecondary)
                    Text("Ops can pin messages with /pin <msgid>")
                        .font(.system(size: 13))
                        .foregroundColor(Theme.textMuted)
                }
            } else {
                ScrollView {
                    LazyVStack(spacing: 0) {
                        ForEach(pins) { pin in
                            VStack(alignment: .leading, spacing: 6) {
                                HStack(spacing: 8) {
                                    UserAvatar(nick: pin.from, size: 32)
                                    VStack(alignment: .leading, spacing: 2) {
                                        Text(pin.from)
                                            .font(.system(size: 14, weight: .semibold))
                                            .foregroundColor(Theme.nickColor(for: pin.from))
                                        Text(pin.timestamp, style: .relative)
                                            .font(.system(size: 11))
                                            .foregroundColor(Theme.textMuted)
                                    }
                                    Spacer()
                                    Image(systemName: "pin.fill")
                                        .font(.system(size: 11))
                                        .foregroundColor(Theme.warning)
                                }

                                Text(pin.text)
                                    .font(.system(size: 15))
                                    .foregroundColor(Theme.textPrimary)
                                    .textSelection(.enabled)

                                HStack {
                                    Text("Pinned by \(pin.pinnedBy)")
                                        .font(.system(size: 11))
                                        .foregroundColor(Theme.textMuted)
                                    Spacer()
                                }
                            }
                            .padding(.horizontal, 16)
                            .padding(.vertical, 12)

                            Rectangle().fill(Theme.border).frame(height: 1)
                                .padding(.leading, 56)
                        }
                    }
                }
            }
        }
        .navigationTitle("Pinned Messages")
        .navigationBarTitleDisplayMode(.inline)
        .toolbarBackground(Theme.bgSecondary, for: .navigationBar)
        .toolbarBackground(.visible, for: .navigationBar)
        .task { await fetchPins() }
    }

    private func fetchPins() async {
        loading = true
        defer { loading = false }

        let encoded = channelName.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed) ?? channelName
        guard let url = URL(string: "https://irc.freeq.at/api/v1/channels/\(encoded)/pins") else {
            error = "Invalid channel name"
            return
        }

        do {
            let (data, response) = try await URLSession.shared.data(from: url)
            guard (response as? HTTPURLResponse)?.statusCode == 200 else {
                error = "Failed to load pins"
                return
            }
            let json = try JSONSerialization.jsonObject(with: data) as? [String: Any]
            guard let pinsArray = json?["pins"] as? [[String: Any]] else {
                error = "Invalid response"
                return
            }

            let formatter = ISO8601DateFormatter()
            pins = pinsArray.compactMap { pin -> PinnedMessage? in
                guard let msgid = pin["msgid"] as? String,
                      let from = pin["from"] as? String,
                      let text = pin["text"] as? String else { return nil }
                let timestamp: Date
                if let ts = pin["timestamp"] as? String {
                    timestamp = formatter.date(from: ts) ?? Date()
                } else {
                    timestamp = Date()
                }
                let pinnedBy = pin["pinned_by"] as? String ?? "unknown"
                return PinnedMessage(id: msgid, from: from, text: text, timestamp: timestamp, pinnedBy: pinnedBy)
            }
        } catch {
            self.error = "Network error"
        }
    }
}
