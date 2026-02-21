import SwiftUI

struct MemberListView: View {
    @ObservedObject var channel: ChannelState

    private var ops: [MemberInfo] {
        channel.members.filter { $0.isOp }.sorted { $0.nick.lowercased() < $1.nick.lowercased() }
    }

    private var voiced: [MemberInfo] {
        channel.members.filter { $0.isVoiced && !$0.isOp }.sorted { $0.nick.lowercased() < $1.nick.lowercased() }
    }

    private var regular: [MemberInfo] {
        channel.members.filter { !$0.isOp && !$0.isVoiced }.sorted { $0.nick.lowercased() < $1.nick.lowercased() }
    }

    var body: some View {
        VStack(spacing: 0) {
            // Header
            HStack {
                Text("Members")
                    .font(.system(size: 13, weight: .bold))
                    .foregroundColor(Theme.textSecondary)
                Spacer()
                Text("\(channel.members.count)")
                    .font(.system(size: 13, weight: .medium))
                    .foregroundColor(Theme.textMuted)
            }
            .padding(.horizontal, 16)
            .padding(.vertical, 12)

            Rectangle()
                .fill(Theme.border)
                .frame(height: 1)

            ScrollView {
                VStack(alignment: .leading, spacing: 0) {
                    if !ops.isEmpty {
                        memberSection("OPERATORS — \(ops.count)", members: ops)
                    }
                    if !voiced.isEmpty {
                        memberSection("VOICED — \(voiced.count)", members: voiced)
                    }
                    if !regular.isEmpty {
                        memberSection("MEMBERS — \(regular.count)", members: regular)
                    }
                }
                .padding(.vertical, 4)
            }
        }
        .background(Theme.bgSecondary)
        .overlay(
            Rectangle()
                .fill(Theme.border)
                .frame(width: 1),
            alignment: .leading
        )
    }

    private func memberSection(_ title: String, members: [MemberInfo]) -> some View {
        VStack(alignment: .leading, spacing: 0) {
            Text(title)
                .font(.system(size: 10, weight: .bold))
                .foregroundColor(Theme.textMuted)
                .kerning(0.5)
                .padding(.horizontal, 16)
                .padding(.top, 12)
                .padding(.bottom, 6)

            ForEach(members) { member in
                memberRow(member)
            }
        }
    }

    private func memberRow(_ member: MemberInfo) -> some View {
        HStack(spacing: 10) {
            // Avatar
            ZStack {
                Circle()
                    .fill(Theme.nickColor(for: member.nick).opacity(0.2))
                    .frame(width: 32, height: 32)
                Text(String(member.nick.prefix(1)).uppercased())
                    .font(.system(size: 12, weight: .bold))
                    .foregroundColor(Theme.nickColor(for: member.nick))
            }
            .overlay(
                // Presence dot
                Circle()
                    .fill(Theme.success)
                    .frame(width: 10, height: 10)
                    .overlay(
                        Circle()
                            .stroke(Theme.bgSecondary, lineWidth: 2)
                    )
                    .offset(x: 2, y: 2),
                alignment: .bottomTrailing
            )

            VStack(alignment: .leading, spacing: 1) {
                HStack(spacing: 4) {
                    if member.isOp {
                        Image(systemName: "shield.fill")
                            .font(.system(size: 9))
                            .foregroundColor(Theme.warning)
                    }
                    Text(member.nick)
                        .font(.system(size: 14, weight: .medium))
                        .foregroundColor(Theme.textPrimary)
                        .lineLimit(1)
                }
            }

            Spacer()
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 5)
    }
}
