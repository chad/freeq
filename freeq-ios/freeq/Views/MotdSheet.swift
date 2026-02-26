import SwiftUI

/// Modal sheet displaying the server's Message of the Day.
struct MotdSheet: View {
    @EnvironmentObject var appState: AppState
    @Environment(\.dismiss) var dismiss

    var body: some View {
        NavigationView {
            ZStack {
                Theme.bgPrimary.ignoresSafeArea()

                ScrollView {
                    VStack(alignment: .leading, spacing: 0) {
                        // Header
                        HStack(spacing: 12) {
                            FreeqLogo(size: 48)

                            VStack(alignment: .leading, spacing: 4) {
                                Text("Welcome to ")
                                    .font(.system(size: 20, weight: .bold))
                                    .foregroundColor(Theme.textPrimary)
                                + Text("freeq")
                                    .font(.system(size: 20, weight: .bold))
                                    .foregroundColor(Theme.accent)

                                Text("Message of the Day")
                                    .font(.system(size: 13))
                                    .foregroundColor(Theme.textMuted)
                            }
                        }
                        .padding(.horizontal, 20)
                        .padding(.top, 20)
                        .padding(.bottom, 16)

                        // MOTD body
                        VStack(alignment: .leading, spacing: 2) {
                            ForEach(Array(appState.motdLines.enumerated()), id: \.offset) { _, line in
                                if line.trimmingCharacters(in: .whitespaces).isEmpty {
                                    Spacer().frame(height: 8)
                                } else {
                                    motdLine(line)
                                }
                            }
                        }
                        .padding(.horizontal, 20)
                        .padding(.bottom, 24)

                        // Action button
                        Button(action: { dismiss() }) {
                            Text("Let's go")
                                .font(.system(size: 16, weight: .bold))
                                .foregroundColor(.white)
                                .frame(maxWidth: .infinity)
                                .padding(.vertical, 14)
                                .background(
                                    LinearGradient(
                                        colors: [Theme.accent, Theme.accentLight],
                                        startPoint: .leading,
                                        endPoint: .trailing
                                    )
                                )
                                .cornerRadius(12)
                        }
                        .padding(.horizontal, 20)
                        .padding(.bottom, 20)
                    }
                }
            }
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button(action: { dismiss() }) {
                        Image(systemName: "xmark")
                            .font(.system(size: 14, weight: .medium))
                            .foregroundColor(Theme.textMuted)
                    }
                }
            }
            .toolbarBackground(Theme.bgPrimary, for: .navigationBar)
            .toolbarBackground(.visible, for: .navigationBar)
        }
        .preferredColorScheme(.dark)
    }

    @ViewBuilder
    private func motdLine(_ text: String) -> some View {
        let trimmed = text.trimmingCharacters(in: .whitespaces)

        if trimmed.hasPrefix("#") {
            // Channel name — highlight in accent
            HStack(spacing: 0) {
                ForEach(Array(splitMotdLine(text).enumerated()), id: \.offset) { _, part in
                    if part.hasPrefix("#") && part.count > 1 {
                        Text(part)
                            .font(.system(size: 15, weight: .semibold, design: .monospaced))
                            .foregroundColor(Theme.accent)
                    } else if part.hasPrefix("http") {
                        Link(part, destination: URL(string: part) ?? URL(string: "https://freeq.at")!)
                            .font(.system(size: 15, design: .monospaced))
                            .foregroundColor(Theme.accent)
                    } else {
                        Text(part)
                            .font(.system(size: 15, design: .monospaced))
                            .foregroundColor(Theme.textSecondary)
                    }
                }
            }
            .padding(.vertical, 1)
        } else {
            // Regular line — detect channels and URLs
            HStack(spacing: 0) {
                ForEach(Array(splitMotdLine(text).enumerated()), id: \.offset) { _, part in
                    if part.hasPrefix("#") && part.count > 1 {
                        Text(part)
                            .font(.system(size: 15, weight: .semibold, design: .monospaced))
                            .foregroundColor(Theme.accent)
                    } else if part.hasPrefix("http") {
                        Link(part, destination: URL(string: part) ?? URL(string: "https://freeq.at")!)
                            .font(.system(size: 15, design: .monospaced))
                            .foregroundColor(Theme.accent)
                    } else {
                        Text(part)
                            .font(.system(size: 15, design: .monospaced))
                            .foregroundColor(Theme.textSecondary)
                    }
                }
            }
            .padding(.vertical, 1)
        }
    }

    /// Split a MOTD line into parts: channels (#foo), URLs (https://...), and plain text.
    private func splitMotdLine(_ text: String) -> [String] {
        let pattern = #"(#\S+|https?://\S+)"#
        guard let regex = try? NSRegularExpression(pattern: pattern) else { return [text] }
        let range = NSRange(text.startIndex..., in: text)
        let matches = regex.matches(in: text, range: range)

        var parts: [String] = []
        var lastEnd = text.startIndex

        for match in matches {
            guard let matchRange = Range(match.range, in: text) else { continue }
            if lastEnd < matchRange.lowerBound {
                parts.append(String(text[lastEnd..<matchRange.lowerBound]))
            }
            parts.append(String(text[matchRange]))
            lastEnd = matchRange.upperBound
        }
        if lastEnd < text.endIndex {
            parts.append(String(text[lastEnd...]))
        }
        return parts
    }
}
