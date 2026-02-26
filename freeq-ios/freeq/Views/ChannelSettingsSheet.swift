import SwiftUI

/// Channel settings sheet — topic editing, channel info, leave.
struct ChannelSettingsSheet: View {
    @EnvironmentObject var appState: AppState
    @Environment(\.dismiss) var dismiss
    let channel: ChannelState

    @State private var editingTopic = false
    @State private var topicDraft: String = ""

    private var isOp: Bool {
        channel.memberInfo(for: appState.nick)?.isOp ?? false
    }

    var body: some View {
        NavigationView {
            ZStack {
                Theme.bgPrimary.ignoresSafeArea()

                List {
                    // Channel info
                    Section {
                        HStack(spacing: 12) {
                            ZStack {
                                RoundedRectangle(cornerRadius: 12)
                                    .fill(Theme.accent.opacity(0.15))
                                    .frame(width: 48, height: 48)
                                Text("#")
                                    .font(.system(size: 24, weight: .bold, design: .monospaced))
                                    .foregroundColor(Theme.accent)
                            }

                            VStack(alignment: .leading, spacing: 3) {
                                Text(channel.name)
                                    .font(.system(size: 17, weight: .bold))
                                    .foregroundColor(Theme.textPrimary)

                                Text("\(channel.members.count) members")
                                    .font(.system(size: 13))
                                    .foregroundColor(Theme.textSecondary)
                            }
                        }
                        .listRowBackground(Theme.bgSecondary)
                    }

                    // Topic
                    Section {
                        if editingTopic {
                            VStack(alignment: .leading, spacing: 8) {
                                TextField("Channel topic", text: $topicDraft, axis: .vertical)
                                    .font(.system(size: 15))
                                    .foregroundColor(Theme.textPrimary)
                                    .lineLimit(1...5)
                                    .tint(Theme.accent)

                                HStack {
                                    Button("Cancel") {
                                        editingTopic = false
                                        topicDraft = channel.topic
                                    }
                                    .font(.system(size: 14, weight: .medium))
                                    .foregroundColor(Theme.textSecondary)

                                    Spacer()

                                    Button("Save") {
                                        appState.sendRaw("TOPIC \(channel.name) :\(topicDraft)")
                                        editingTopic = false
                                    }
                                    .font(.system(size: 14, weight: .bold))
                                    .foregroundColor(Theme.accent)
                                }
                            }
                            .listRowBackground(Theme.bgSecondary)
                        } else {
                            VStack(alignment: .leading, spacing: 6) {
                                HStack {
                                    Text("Topic")
                                        .font(.system(size: 13, weight: .semibold))
                                        .foregroundColor(Theme.textMuted)
                                    Spacer()
                                    if isOp {
                                        Button(action: {
                                            topicDraft = channel.topic
                                            editingTopic = true
                                        }) {
                                            Image(systemName: "pencil")
                                                .font(.system(size: 13))
                                                .foregroundColor(Theme.accent)
                                        }
                                    }
                                }

                                if channel.topic.isEmpty {
                                    Text("No topic set")
                                        .font(.system(size: 14))
                                        .foregroundColor(Theme.textMuted)
                                        .italic()
                                } else {
                                    Text(channel.topic)
                                        .font(.system(size: 14))
                                        .foregroundColor(Theme.textSecondary)
                                        .textSelection(.enabled)
                                }
                            }
                            .listRowBackground(Theme.bgSecondary)
                        }
                    } header: {
                        Text("Topic")
                            .foregroundColor(Theme.textMuted)
                    }

                    // Members preview
                    Section {
                        let ops = channel.members.filter { $0.isOp }
                        if !ops.isEmpty {
                            VStack(alignment: .leading, spacing: 6) {
                                Text("Operators")
                                    .font(.system(size: 12, weight: .bold))
                                    .foregroundColor(Theme.textMuted)
                                    .kerning(0.5)

                                ForEach(ops) { member in
                                    HStack(spacing: 8) {
                                        UserAvatar(nick: member.nick, size: 28)
                                        Text(member.nick)
                                            .font(.system(size: 14))
                                            .foregroundColor(Theme.textPrimary)
                                        if member.isVerified {
                                            VerifiedBadge(size: 12)
                                        }
                                        Spacer()
                                        Image(systemName: "shield.fill")
                                            .font(.system(size: 11))
                                            .foregroundColor(Theme.warning)
                                    }
                                }
                            }
                            .listRowBackground(Theme.bgSecondary)
                        }
                    } header: {
                        Text("Members (\(channel.members.count))")
                            .foregroundColor(Theme.textMuted)
                    }

                    // Actions
                    Section {
                        Button(action: {
                            appState.partChannel(channel.name)
                            dismiss()
                        }) {
                            HStack {
                                Spacer()
                                Text("Leave Channel")
                                    .font(.system(size: 16, weight: .medium))
                                    .foregroundColor(Theme.danger)
                                Spacer()
                            }
                        }
                        .listRowBackground(Theme.bgSecondary)
                    }
                }
                .listStyle(.insetGrouped)
                .scrollContentBackground(.hidden)
            }
            .navigationTitle("Channel Settings")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button("Done") { dismiss() }
                        .foregroundColor(Theme.accent)
                }
            }
            .toolbarBackground(Theme.bgSecondary, for: .navigationBar)
            .toolbarBackground(.visible, for: .navigationBar)
        }
        .preferredColorScheme(.dark)
    }
}
