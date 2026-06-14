import SwiftUI

/// Channel settings: topic, modes, ops.
struct ChannelSettingsSheet: View {
    @Environment(AppState.self) private var appState
    @Environment(\.dismiss) private var dismiss
    let channel: ChannelState
    @State private var newTopic: String = ""
    @State private var policyRules: String = "Be respectful. No harassment, spam, or hate speech."
    @State private var verifierType: String = "github_repo"
    @State private var verifierParam: String = ""
    @State private var verifierLabel: String = "GitHub"
    @State private var roleName: String = "voice"
    @State private var roleCredentialType: String = "github_repo"
    @State private var policyStatus: String?

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

                    // Policy / join gates
                    GroupBox("Policy & Join Gates") {
                        VStack(alignment: .leading, spacing: 12) {
                            Text("Set rules, add credential verifiers, accept gates, and assign roles using the server's POLICY protocol.")
                                .font(.caption)
                                .foregroundStyle(.secondary)

                            VStack(alignment: .leading, spacing: 6) {
                                Text("Rules")
                                    .font(.caption.weight(.semibold))
                                TextEditor(text: $policyRules)
                                    .font(.body)
                                    .frame(minHeight: 70)
                                    .overlay(
                                        RoundedRectangle(cornerRadius: 6)
                                            .strokeBorder(Color(nsColor: .separatorColor), lineWidth: 0.5)
                                    )
                                HStack {
                                    Button {
                                        sendPolicy("SET \(policyRules.trimmingCharacters(in: .whitespacesAndNewlines))")
                                    } label: {
                                        Label("Set Rules", systemImage: "doc.badge.gearshape")
                                    }
                                    .disabled(policyRules.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)

                                    Button {
                                        sendPolicy("INFO")
                                    } label: {
                                        Label("Info", systemImage: "info.circle")
                                    }

                                    Button {
                                        sendPolicy("ACCEPT")
                                    } label: {
                                        Label("Accept Gate", systemImage: "checkmark.seal")
                                    }
                                }
                            }

                            Divider()

                            VStack(alignment: .leading, spacing: 6) {
                                Text("Credential Verifier")
                                    .font(.caption.weight(.semibold))
                                Picker("Type", selection: $verifierType) {
                                    Text("GitHub repo").tag("github_repo")
                                    Text("GitHub org").tag("github_membership")
                                    Text("Bluesky follower").tag("bluesky_follower")
                                    Text("Moderator").tag("channel_moderator")
                                }
                                .pickerStyle(.segmented)

                                HStack {
                                    TextField(verifierPlaceholder, text: $verifierParam)
                                        .textFieldStyle(.roundedBorder)
                                    TextField("Label", text: $verifierLabel)
                                        .textFieldStyle(.roundedBorder)
                                        .frame(width: 110)
                                    Button {
                                        addVerifier()
                                    } label: {
                                        Label("Require", systemImage: "person.badge.shield.checkmark")
                                    }
                                    .disabled(verifierParam.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty && verifierType != "channel_moderator")
                                }
                            }

                            Divider()

                            VStack(alignment: .leading, spacing: 6) {
                                Text("Role by Credential")
                                    .font(.caption.weight(.semibold))
                                HStack {
                                    Picker("Role", selection: $roleName) {
                                        Text("Op").tag("op")
                                        Text("Half-op").tag("halfop")
                                        Text("Voice").tag("voice")
                                    }
                                    .frame(width: 120)
                                    TextField("Credential type", text: $roleCredentialType)
                                        .textFieldStyle(.roundedBorder)
                                    Button {
                                        setRolePolicy()
                                    } label: {
                                        Label("Set Role", systemImage: "person.crop.circle.badge.checkmark")
                                    }
                                    .disabled(roleCredentialType.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                                }
                            }

                            HStack {
                                Button(role: .destructive) {
                                    sendPolicy("CLEAR")
                                } label: {
                                    Label("Clear Policy", systemImage: "trash")
                                }
                                Spacer()
                                Button {
                                    if verifierType == "github_repo", !verifierParam.isEmpty {
                                        sendPolicy("VERIFY github \(verifierParam.trimmingCharacters(in: .whitespacesAndNewlines))")
                                    }
                                } label: {
                                    Label("Verify GitHub", systemImage: "checkmark.shield")
                                }
                                .disabled(verifierType != "github_repo" || verifierParam.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                            }

                            if let policyStatus {
                                Text(policyStatus)
                                    .font(.caption)
                                    .foregroundStyle(.secondary)
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
        .frame(width: 540, height: 700)
        .onAppear { newTopic = "" }
    }

    private var verifierPlaceholder: String {
        switch verifierType {
        case "github_repo": return "owner/repo"
        case "github_membership": return "org-name"
        case "bluesky_follower": return "handle.bsky.social"
        default: return "optional"
        }
    }

    private func sendPolicy(_ command: String) {
        appState.sendRaw("POLICY \(channel.name) \(command)")
        policyStatus = "Sent: POLICY \(channel.name) \(command)"
    }

    private func addVerifier() {
        let param = verifierParam.trimmingCharacters(in: .whitespacesAndNewlines)
        let url: String
        switch verifierType {
        case "github_repo":
            url = "/verify/github/start?repo=\(param.addingPercentEncoding(withAllowedCharacters: .urlQueryAllowed) ?? param)"
        case "github_membership":
            url = "/verify/github/start?org=\(param.addingPercentEncoding(withAllowedCharacters: .urlQueryAllowed) ?? param)"
        case "bluesky_follower":
            url = "/verify/bluesky/start?target=\(param.addingPercentEncoding(withAllowedCharacters: .urlQueryAllowed) ?? param)"
        default:
            url = "/verify/mod/start"
        }
        let label = verifierLabel
            .trimmingCharacters(in: .whitespacesAndNewlines)
            .replacingOccurrences(of: " ", with: "_")
        sendPolicy("REQUIRE \(verifierType) issuer=\(policyIssuer) url=\(url) label=\(label.isEmpty ? verifierType : label)")
    }

    private func setRolePolicy() {
        let type = roleCredentialType.trimmingCharacters(in: .whitespacesAndNewlines)
        let json = #"{"type":"PRESENT","credential_type":"\#(type)","issuer":"\#(policyIssuer)"}"#
        sendPolicy("SET-ROLE \(roleName) \(json)")
    }

    private var policyIssuer: String {
        "did:web:irc.freeq.at:verify"
    }
}
