import SwiftUI

struct JoinChannelSheet: View {
    @Environment(AppState.self) private var appState
    @Environment(\.dismiss) private var dismiss
    @State private var channelName: String = "#"

    var body: some View {
        VStack(spacing: 16) {
            Text("Join Channel")
                .font(.headline)

            TextField("Channel name", text: $channelName)
                .textFieldStyle(.roundedBorder)
                .frame(width: 250)
                .onSubmit { join() }

            HStack {
                Button("Cancel") { dismiss() }
                    .keyboardShortcut(.cancelAction)
                Button("Join") { join() }
                    .keyboardShortcut(.defaultAction)
                    .disabled(channelName.count < 2)
            }
        }
        .padding(24)
    }

    private func join() {
        var name = channelName.trimmingCharacters(in: .whitespaces)
        if !name.hasPrefix("#") { name = "#" + name }
        appState.joinChannel(name)
        appState.activeChannel = name
        dismiss()
    }
}
