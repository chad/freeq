import SwiftUI

/// Rich user profile sheet: avatar, Bluesky info, shared channels, DID.
struct UserProfileSheet: View {
    @Environment(AppState.self) private var appState
    @Environment(\.dismiss) private var dismiss
    let nick: String

    private var profile: ProfileCache.Profile? {
        ProfileCache.shared.profile(for: nick)
    }
    private var did: String? {
        ProfileCache.shared.did(for: nick)
    }

    var body: some View {
        VStack(spacing: 0) {
            // Header
            HStack {
                Spacer()
                Button { dismiss() } label: {
                    Image(systemName: "xmark.circle.fill")
                        .font(.title3)
                        .foregroundStyle(.secondary)
                }
                .buttonStyle(.plain)
            }
            .padding(.trailing, 12)
            .padding(.top, 8)

            // Avatar
            AvatarView(nick: nick, size: 72)
                .padding(.top, 4)

            // Name
            if let displayName = profile?.displayName, !displayName.isEmpty {
                Text(displayName)
                    .font(.title2.weight(.bold))
                    .padding(.top, 8)
                Text("@\(profile?.handle ?? nick)")
                    .font(.subheadline)
                    .foregroundStyle(.secondary)
            } else {
                Text(nick)
                    .font(.title2.weight(.bold))
                    .padding(.top, 8)
            }

            // DID
            if let did {
                HStack(spacing: 4) {
                    Image(systemName: "checkmark.seal.fill")
                        .font(.caption)
                        .foregroundStyle(.blue)
                    Text(did)
                        .font(.caption2.monospaced())
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                        .truncationMode(.middle)
                }
                .padding(.top, 4)
                .textSelection(.enabled)
            }

            // Bio
            if let bio = profile?.description, !bio.isEmpty {
                Text(bio)
                    .font(.body)
                    .foregroundStyle(.secondary)
                    .multilineTextAlignment(.center)
                    .padding(.horizontal, 24)
                    .padding(.top, 8)
            }

            // Stats
            if let p = profile {
                HStack(spacing: 24) {
                    StatItem(label: "Posts", value: p.postsCount ?? 0)
                    StatItem(label: "Followers", value: p.followersCount ?? 0)
                    StatItem(label: "Following", value: p.followsCount ?? 0)
                }
                .padding(.top, 12)
            }

            Divider().padding(.vertical, 12)

            // Shared channels
            let sharedChannels = appState.channels.filter { ch in
                ch.members.contains(where: { $0.nick.lowercased() == nick.lowercased() })
            }
            if !sharedChannels.isEmpty {
                VStack(alignment: .leading, spacing: 4) {
                    Text("Shared Channels")
                        .font(.caption.weight(.semibold))
                        .foregroundStyle(.secondary)
                        .padding(.horizontal, 24)
                    ForEach(sharedChannels) { ch in
                        Button {
                            appState.activeChannel = ch.name
                            dismiss()
                        } label: {
                            HStack {
                                Image(systemName: "number")
                                    .font(.caption)
                                    .foregroundStyle(.secondary)
                                Text(ch.name.replacingOccurrences(of: "#", with: ""))
                                    .font(.body)
                                Spacer()
                                Text("\(ch.members.count)")
                                    .font(.caption)
                                    .foregroundStyle(.tertiary)
                            }
                            .padding(.horizontal, 24)
                            .padding(.vertical, 4)
                            .contentShape(Rectangle())
                        }
                        .buttonStyle(.plain)
                    }
                }
            }

            Spacer()

            // Actions
            HStack(spacing: 12) {
                Button("Send Message") {
                    let dm = appState.getOrCreateDM(nick)
                    appState.activeChannel = dm.name
                    dismiss()
                }
                .buttonStyle(.borderedProminent)
                .controlSize(.regular)

                if let handle = profile?.handle ?? did.map({ _ in nick }) {
                    Link("View on Bluesky", destination: URL(string: "https://bsky.app/profile/\(handle)")!)
                        .font(.body)
                }
            }
            .padding(.bottom, 16)
        }
        .frame(width: 340, height: 480)
        .onAppear {
            // Trigger WHOIS if we don't have DID
            if did == nil { appState.sendWhois(nick) }
            // Fetch profile if we have DID
            if let did, profile == nil {
                ProfileCache.shared.fetchProfile(nick: nick, did: did)
            }
        }
    }
}

struct StatItem: View {
    let label: String
    let value: Int

    var body: some View {
        VStack(spacing: 2) {
            Text("\(value)")
                .font(.headline)
            Text(label)
                .font(.caption2)
                .foregroundStyle(.secondary)
        }
    }
}
