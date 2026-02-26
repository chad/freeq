import SwiftUI

struct SearchSheet: View {
    @EnvironmentObject var appState: AppState
    @Environment(\.dismiss) var dismiss
    @State private var query: String = ""
    @FocusState private var focused: Bool

    private var results: [SearchResult] {
        guard query.count >= 2 else { return [] }
        let q = query.lowercased()
        var matches: [SearchResult] = []

        for channel in appState.channels + appState.dmBuffers {
            for msg in channel.messages where !msg.from.isEmpty && !msg.isDeleted {
                if msg.text.lowercased().contains(q) || msg.from.lowercased().contains(q) {
                    matches.append(SearchResult(channel: channel.name, message: msg))
                }
            }
        }

        return Array(matches.suffix(50).reversed())
    }

    var body: some View {
        NavigationView {
            ZStack {
                Theme.bgPrimary.ignoresSafeArea()

                VStack(spacing: 0) {
                    // Search input
                    HStack(spacing: 10) {
                        Image(systemName: "magnifyingglass")
                            .foregroundColor(Theme.textMuted)
                        TextField("", text: $query, prompt: Text("Search messages...").foregroundColor(Theme.textMuted))
                            .foregroundColor(Theme.textPrimary)
                            .font(.system(size: 16))
                            .focused($focused)
                            .autocapitalization(.none)
                            .disableAutocorrection(true)
                    }
                    .padding(.horizontal, 14)
                    .padding(.vertical, 10)
                    .background(Theme.bgTertiary)
                    .cornerRadius(10)
                    .padding(.horizontal, 16)
                    .padding(.top, 8)

                    if results.isEmpty && query.count >= 2 {
                        Spacer()
                        VStack(spacing: 8) {
                            Image(systemName: "magnifyingglass")
                                .font(.system(size: 32))
                                .foregroundColor(Theme.textMuted)
                            Text("No results found")
                                .font(.system(size: 15))
                                .foregroundColor(Theme.textSecondary)
                        }
                        Spacer()
                    } else if query.count < 2 {
                        Spacer()
                        Text("Type at least 2 characters to search")
                            .font(.system(size: 14))
                            .foregroundColor(Theme.textMuted)
                        Spacer()
                    } else {
                        // Results list
                        ScrollView {
                            LazyVStack(alignment: .leading, spacing: 0) {
                                ForEach(results) { result in
                                    Button(action: {
                                        appState.activeChannel = result.channel
                                        dismiss()
                                    }) {
                                        searchResultRow(result)
                                    }
                                    .buttonStyle(.plain)
                                }
                            }
                            .padding(.top, 8)
                        }
                    }
                }
            }
            .navigationTitle("Search")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") { dismiss() }
                        .foregroundColor(Theme.accent)
                }
            }
            .toolbarBackground(Theme.bgSecondary, for: .navigationBar)
            .toolbarBackground(.visible, for: .navigationBar)
        }
        .onAppear { focused = true }
        .preferredColorScheme(.dark)
    }

    private func searchResultRow(_ result: SearchResult) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack(spacing: 8) {
                Text(result.channel)
                    .font(.system(size: 12, weight: .medium))
                    .foregroundColor(Theme.accent)

                Spacer()

                Text(formatTime(result.message.timestamp))
                    .font(.system(size: 11))
                    .foregroundColor(Theme.textMuted)
            }

            HStack(spacing: 8) {
                Text(result.message.from)
                    .font(.system(size: 13, weight: .bold))
                    .foregroundColor(Theme.nickColor(for: result.message.from))

                Text(highlightedText(result.message.text, query: query))
                    .font(.system(size: 13))
                    .foregroundColor(Theme.textSecondary)
                    .lineLimit(2)
            }
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 10)
        .background(Theme.bgPrimary)
        .overlay(
            Rectangle().fill(Theme.border).frame(height: 0.5),
            alignment: .bottom
        )
    }

    private func highlightedText(_ text: String, query: String) -> AttributedString {
        var result = AttributedString(text)
        if let range = text.lowercased().range(of: query.lowercased()),
           let attrRange = Range(NSRange(range, in: text), in: result) {
            result[attrRange].foregroundColor = Theme.textPrimary
            result[attrRange].font = .system(size: 13, weight: .semibold)
        }
        return result
    }

    private static let todayFormatter: DateFormatter = {
        let f = DateFormatter()
        f.dateFormat = "h:mm a"
        return f
    }()
    private static let dateTimeFormatter: DateFormatter = {
        let f = DateFormatter()
        f.dateFormat = "MMM d, h:mm a"
        return f
    }()

    private func formatTime(_ date: Date) -> String {
        Calendar.current.isDateInToday(date)
            ? Self.todayFormatter.string(from: date)
            : Self.dateTimeFormatter.string(from: date)
    }
}

struct SearchResult: Identifiable {
    let channel: String
    let message: ChatMessage

    var id: String { "\(channel)-\(message.id)" }
}
