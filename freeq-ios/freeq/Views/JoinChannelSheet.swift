import SwiftUI

struct JoinChannelSheet: View {
    @EnvironmentObject var appState: AppState
    @Environment(\.dismiss) var dismiss
    @State private var channelName: String = ""

    var body: some View {
        NavigationView {
            VStack(spacing: 20) {
                TextField("#channel", text: $channelName)
                    .textFieldStyle(.roundedBorder)
                    .font(.body)
                    .autocapitalization(.none)
                    .disableAutocorrection(true)
                    .padding(.horizontal)
                    .padding(.top, 20)

                Button(action: {
                    appState.joinChannel(channelName)
                    dismiss()
                }) {
                    Text("Join Channel")
                        .fontWeight(.semibold)
                        .frame(maxWidth: .infinity)
                        .padding(.vertical, 12)
                        .background(channelName.isEmpty ? Color.gray : Color.accentColor)
                        .foregroundColor(.white)
                        .cornerRadius(10)
                }
                .disabled(channelName.isEmpty)
                .padding(.horizontal)

                Spacer()
            }
            .navigationTitle("Join Channel")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") { dismiss() }
                }
            }
        }
    }
}
