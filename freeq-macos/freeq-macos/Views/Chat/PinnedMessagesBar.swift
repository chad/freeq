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
                        .foregroundStyle(.orange)
                        .rotationEffect(.degrees(-45))

                    if let latest = pins.last {
                        Text("\(latest.from):")
                            .font(.caption.weight(.semibold))
                            .foregroundStyle(Theme.nickColor(for: latest.from))
                        Text(latest.text)
                            .font(.caption)
                            .foregroundStyle(.primary)
                            .lineLimit(1)
                    }

                    Spacer()

                    if pins.count > 1 {
                        Text("\(pins.count) pins")
                            .font(.caption2)
                            .foregroundStyle(.secondary)
                    }

                    Image(systemName: expanded ? "chevron.up" : "chevron.down")
                        .font(.caption2)
                        .foregroundStyle(.secondary)
                }
                .padding(.horizontal, 16)
                .padding(.vertical, 6)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)

            if expanded {
                Divider()
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
                                        .foregroundStyle(.orange)
                                    Text(pin.from)
                                        .font(.caption.weight(.semibold))
                                        .foregroundStyle(Theme.nickColor(for: pin.from))
                                    Text(pin.text)
                                        .font(.caption)
                                        .lineLimit(2)
                                    Spacer()
                                    Text(formatTime(pin.timestamp))
                                        .font(.caption2)
                                        .foregroundStyle(.tertiary)
                                }
                                .padding(.horizontal, 16)
                                .padding(.vertical, 4)
                                .contentShape(Rectangle())
                            }
                            .buttonStyle(.plain)
                            if pin.id != pins.last?.id { Divider().padding(.leading, 16) }
                        }
                    }
                }
                .frame(maxHeight: 150)
            }
        }
        .background(.orange.opacity(0.03))
    }
}
