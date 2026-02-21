import SwiftUI

struct JoinChannelSheet: View {
    @EnvironmentObject var appState: AppState
    @Environment(\.dismiss) var dismiss
    @State private var channelName: String = ""
    @FocusState private var focused: Bool

    var body: some View {
        NavigationView {
            ZStack {
                Theme.bgPrimary.ignoresSafeArea()

                VStack(spacing: 24) {
                    // Input
                    VStack(alignment: .leading, spacing: 8) {
                        Text("CHANNEL NAME")
                            .font(.system(size: 11, weight: .bold))
                            .foregroundColor(Theme.textMuted)
                            .kerning(1)

                        HStack(spacing: 8) {
                            Text("#")
                                .font(.system(size: 20, weight: .medium, design: .monospaced))
                                .foregroundColor(Theme.textMuted)

                            TextField("", text: $channelName, prompt: Text("general").foregroundColor(Theme.textMuted))
                                .foregroundColor(Theme.textPrimary)
                                .font(.system(size: 17))
                                .autocapitalization(.none)
                                .disableAutocorrection(true)
                                .focused($focused)
                                .onSubmit { join() }
                        }
                        .padding(.horizontal, 14)
                        .padding(.vertical, 12)
                        .background(Theme.bgTertiary)
                        .cornerRadius(10)
                        .overlay(
                            RoundedRectangle(cornerRadius: 10)
                                .stroke(focused ? Theme.accent : Theme.border, lineWidth: 1)
                        )
                    }

                    // Quick suggestions
                    VStack(alignment: .leading, spacing: 8) {
                        Text("POPULAR CHANNELS")
                            .font(.system(size: 11, weight: .bold))
                            .foregroundColor(Theme.textMuted)
                            .kerning(1)

                        LazyVGrid(columns: [GridItem(.adaptive(minimum: 120))], spacing: 8) {
                            ForEach(["general", "random", "music", "tech", "gaming"], id: \.self) { name in
                                Button(action: {
                                    channelName = name
                                    join()
                                }) {
                                    HStack(spacing: 4) {
                                        Text("#")
                                            .font(.system(size: 13, design: .monospaced))
                                            .foregroundColor(Theme.textMuted)
                                        Text(name)
                                            .font(.system(size: 14, weight: .medium))
                                            .foregroundColor(Theme.textPrimary)
                                    }
                                    .frame(maxWidth: .infinity)
                                    .padding(.vertical, 10)
                                    .background(Theme.bgTertiary)
                                    .cornerRadius(8)
                                    .overlay(
                                        RoundedRectangle(cornerRadius: 8)
                                            .stroke(Theme.border, lineWidth: 1)
                                    )
                                }
                                .buttonStyle(.plain)
                            }
                        }
                    }

                    // Join button
                    Button(action: join) {
                        Text("Join Channel")
                            .font(.system(size: 16, weight: .semibold))
                            .frame(maxWidth: .infinity)
                            .padding(.vertical, 14)
                            .background(
                                channelName.isEmpty
                                    ? AnyShapeStyle(Theme.textMuted.opacity(0.3))
                                    : AnyShapeStyle(Theme.accent)
                            )
                            .foregroundColor(.white)
                            .cornerRadius(10)
                    }
                    .disabled(channelName.isEmpty)

                    Spacer()
                }
                .padding(20)
            }
            .navigationTitle("Join Channel")
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

    private func join() {
        guard !channelName.isEmpty else { return }
        appState.joinChannel(channelName)
        dismiss()
    }
}
