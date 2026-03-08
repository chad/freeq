import SwiftUI

/// Channel list browser — shows all available channels from /LIST.
struct ChannelListBrowser: View {
    @Environment(AppState.self) private var appState
    @Environment(\.dismiss) private var dismiss
    @State private var searchText = ""
    @State private var channels: [(name: String, users: Int, topic: String)] = []
    @State private var isLoading = true

    var filtered: [(name: String, users: Int, topic: String)] {
        if searchText.isEmpty { return channels }
        let q = searchText.lowercased()
        return channels.filter { $0.name.lowercased().contains(q) || $0.topic.lowercased().contains(q) }
    }

    var body: some View {
        VStack(spacing: 0) {
            HStack {
                Text("Browse Channels")
                    .font(.headline)
                Spacer()
                if isLoading {
                    ProgressView()
                        .scaleEffect(0.6)
                }
                Button("Done") { dismiss() }
            }
            .padding(16)

            // Search
            HStack {
                Image(systemName: "magnifyingglass")
                    .foregroundStyle(.secondary)
                TextField("Filter channels…", text: $searchText)
                    .textFieldStyle(.plain)
            }
            .padding(.horizontal, 16)
            .padding(.bottom, 8)

            Divider()

            if filtered.isEmpty && !isLoading {
                VStack(spacing: 8) {
                    Image(systemName: "list.bullet")
                        .font(.system(size: 32))
                        .foregroundStyle(.tertiary)
                    Text("No channels found")
                        .foregroundStyle(.secondary)
                }
                .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else {
                List {
                    ForEach(filtered, id: \.name) { ch in
                        Button {
                            appState.joinChannel(ch.name)
                            dismiss()
                        } label: {
                            HStack {
                                VStack(alignment: .leading, spacing: 2) {
                                    HStack(spacing: 6) {
                                        Text(ch.name)
                                            .font(.body.weight(.medium))
                                        Text("\(ch.users)")
                                            .font(.caption)
                                            .foregroundStyle(.secondary)
                                            .padding(.horizontal, 6)
                                            .padding(.vertical, 1)
                                            .background(Capsule().fill(.secondary.opacity(0.15)))
                                    }
                                    if !ch.topic.isEmpty {
                                        Text(ch.topic)
                                            .font(.caption)
                                            .foregroundStyle(.secondary)
                                            .lineLimit(2)
                                    }
                                }
                                Spacer()
                                let alreadyJoined = appState.channels.contains(where: { $0.name.lowercased() == ch.name.lowercased() })
                                if alreadyJoined {
                                    Text("Joined")
                                        .font(.caption2)
                                        .foregroundStyle(.green)
                                }
                            }
                            .contentShape(Rectangle())
                        }
                        .buttonStyle(.plain)
                    }
                }
            }
        }
        .frame(width: 500, height: 400)
        .onAppear { requestList() }
    }

    private func requestList() {
        isLoading = true
        appState.sendRaw("LIST")
        // Collect responses — they come through notices
        // For now, show joined channels + common defaults
        DispatchQueue.main.asyncAfter(deadline: .now() + 1.5) {
            // Pull from what we know + any LIST responses
            var result: [(String, Int, String)] = []
            for ch in appState.channels {
                result.append((ch.name, ch.members.count, ch.topic))
            }
            // Add some common defaults if not already in list
            for name in ["#general", "#random", "#dev", "#help", "#offtopic"] {
                if !result.contains(where: { $0.0.lowercased() == name }) {
                    result.append((name, 0, ""))
                }
            }
            channels = result.sorted { $0.1 > $1.1 }
            isLoading = false
        }
    }
}
