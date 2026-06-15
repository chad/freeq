import SwiftUI

/// Horizontal pinned messages bar — shows latest pin, click to expand.
struct PinnedMessagesBar: View {
    @Environment(AppState.self) private var appState
    let pins: [ChatMessage]
    @State private var expanded = false

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            Button {
                withAnimation { expanded.toggle() }
            } label: {
                HStack(spacing: 6) {
                    Image(systemName: "pin.fill")
                        .font(.caption)
                        .foregroundStyle(Theme.warning)
                        .rotationEffect(.degrees(-45))

                    if let latest = pins.last {
                        Text("\(latest.from):")
                            .font(.caption.weight(.semibold))
                            .foregroundStyle(Theme.nickColor(for: latest.from))
                        Text(latest.text)
                            .font(.caption)
                            .foregroundStyle(Theme.textPrimary)
                            .lineLimit(1)
                    }

                    Spacer()

                    if pins.count > 1 {
                        Text("\(pins.count) pins")
                            .font(.caption2)
                            .foregroundStyle(Theme.textSecondary)
                    }

                    Image(systemName: expanded ? "chevron.up" : "chevron.down")
                        .font(.caption2)
                        .foregroundStyle(Theme.textTertiary)
                }
                .padding(.horizontal, 16)
                .padding(.vertical, 6)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)

            if expanded {
                Divider().overlay(Theme.borderSoft)
                ScrollView {
                    VStack(alignment: .leading, spacing: 0) {
                        ForEach(pins) { pin in
                            Button {
                                appState.scrollToMessageId = pin.id
                                expanded = false
                            } label: {
                                HStack(spacing: 6) {
                                    Image(systemName: "pin.fill")
                                        .font(.caption2)
                                        .foregroundStyle(Theme.warning)
                                    Text(pin.from)
                                        .font(.caption.weight(.semibold))
                                        .foregroundStyle(Theme.nickColor(for: pin.from))
                                    Text(pin.text)
                                        .font(.caption)
                                        .foregroundStyle(Theme.textPrimary)
                                        .lineLimit(2)
                                    Spacer()
                                    Text(formatTime(pin.timestamp))
                                        .font(.caption2)
                                        .foregroundStyle(Theme.textTertiary)
                                }
                                .padding(.horizontal, 16)
                                .padding(.vertical, 4)
                                .contentShape(Rectangle())
                            }
                            .buttonStyle(.plain)
                            if pin.id != pins.last?.id {
                                Divider().overlay(Theme.borderSoft).padding(.leading, 16)
                            }
                        }
                    }
                }
                .frame(maxHeight: 150)
            }
        }
        .background(Theme.warning.opacity(0.055))
    }
}
