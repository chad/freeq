import SwiftUI

/// Channel settings: topic, modes, ops.
struct ChannelSettingsSheet: View {
    @Environment(AppState.self) private var appState
    @Environment(\.dismiss) private var dismiss
    let channel: ChannelState
    @State private var newTopic: String = ""

    var body: some View {
        VStack(spacing: 0) {
            // Header
            HStack {
                Text("Channel Settings")
                    .font(.headline)
                Spacer()
                Button("Done") { dismiss() }
                    .keyboardShortcut(.cancelAction)
            }
            .padding(16)

            Divider()

            ScrollView {
                VStack(alignment: .leading, spacing: 16) {
                    // Topic
                    GroupBox("Topic") {
                        VStack(alignment: .leading, spacing: 8) {
                            if !channel.topic.isEmpty {
                                Text(channel.topic)
                                    .font(.body)
                                if let setBy = channel.topicSetBy {
                                    Text("Set by \(setBy)")
                                        .font(.caption)
                                        .foregroundStyle(.tertiary)
                                }
                            }
                            HStack {
                                TextField("New topic…", text: $newTopic)
                                    .textFieldStyle(.roundedBorder)
                                Button("Set") {
                                    appState.sendRaw("TOPIC \(channel.name) :\(newTopic)")
                                    newTopic = ""
                                }
                                .disabled(newTopic.isEmpty)
                            }
                        }
                        .padding(4)
                    }

                    // Members
                    GroupBox("Members (\(channel.members.count))") {
                        VStack(alignment: .leading, spacing: 4) {
                            let ops = channel.members.filter(\.isOp)
                            let voiced = channel.members.filter { $0.isVoiced && !$0.isOp }

                            if !ops.isEmpty {
                                Text("Operators: \(ops.map(\.nick).joined(separator: ", "))")
                                    .font(.caption)
                            }
                            if !voiced.isEmpty {
                                Text("Voiced: \(voiced.map(\.nick).joined(separator: ", "))")
                                    .font(.caption)
                            }
                        }
                        .padding(4)
                    }

                    // Actions
                    GroupBox("Actions") {
                        VStack(alignment: .leading, spacing: 8) {
                            Button("Request PINS") {
                                appState.sendRaw("PINS \(channel.name)")
                            }
                            Button("Leave Channel", role: .destructive) {
                                appState.partChannel(channel.name)
                                dismiss()
                            }
                        }
                        .padding(4)
                    }
                }
                .padding(16)
            }
        }
        .frame(width: 400, height: 450)
        .onAppear { newTopic = "" }
    }
}
