import SwiftUI
import PhotosUI

/// Photo picker + upload flow for AT-authenticated users.
struct PhotoPickerButton: View {
    @EnvironmentObject var appState: AppState
    @State private var selectedItem: PhotosPickerItem? = nil
    @State private var uploading = false

    let channel: String

    var body: some View {
        if appState.authenticatedDID != nil {
            PhotosPicker(selection: $selectedItem, matching: .images) {
                Image(systemName: "photo.on.rectangle.angled")
                    .font(.system(size: 20))
                    .foregroundColor(uploading ? Theme.textMuted : Theme.accent)
            }
            .disabled(uploading)
            .onChange(of: selectedItem) {
                if let item = selectedItem {
                    uploadPhoto(item)
                    selectedItem = nil
                }
            }
            .overlay {
                if uploading {
                    ProgressView()
                        .scaleEffect(0.6)
                        .tint(Theme.accent)
                }
            }
        }
    }

    private func uploadPhoto(_ item: PhotosPickerItem) {
        uploading = true
        Task {
            guard let data = try? await item.loadTransferable(type: Data.self) else {
                await MainActor.run {
                    uploading = false
                    appState.errorMessage = "Failed to load photo data"
                }
                return
            }
            print("[Upload] Photo data loaded: \(data.count) bytes")

            let did = appState.authenticatedDID ?? ""
            let serverBase = appState.serverAddress.contains("freeq.at")
                ? "https://irc.freeq.at"
                : "http://127.0.0.1:8080"

            let boundary = UUID().uuidString
            var body = Data()

            // DID field
            body.append("--\(boundary)\r\n".data(using: .utf8)!)
            body.append("Content-Disposition: form-data; name=\"did\"\r\n\r\n".data(using: .utf8)!)
            body.append("\(did)\r\n".data(using: .utf8)!)

            // Channel field
            body.append("--\(boundary)\r\n".data(using: .utf8)!)
            body.append("Content-Disposition: form-data; name=\"channel\"\r\n\r\n".data(using: .utf8)!)
            body.append("\(channel)\r\n".data(using: .utf8)!)

            // File field
            body.append("--\(boundary)\r\n".data(using: .utf8)!)
            body.append("Content-Disposition: form-data; name=\"file\"; filename=\"photo.jpg\"\r\n".data(using: .utf8)!)
            body.append("Content-Type: image/jpeg\r\n\r\n".data(using: .utf8)!)
            body.append(data)
            body.append("\r\n".data(using: .utf8)!)
            body.append("--\(boundary)--\r\n".data(using: .utf8)!)

            var request = URLRequest(url: URL(string: "\(serverBase)/api/v1/upload")!)
            request.httpMethod = "POST"
            request.setValue("multipart/form-data; boundary=\(boundary)", forHTTPHeaderField: "Content-Type")
            request.httpBody = body

            do {
                print("[Upload] Sending to \(serverBase)/api/v1/upload, DID=\(did), channel=\(channel)")
                let (responseData, response) = try await URLSession.shared.data(for: request)
                let statusCode = (response as? HTTPURLResponse)?.statusCode ?? 0
                let responseText = String(data: responseData, encoding: .utf8) ?? ""
                print("[Upload] Response: HTTP \(statusCode), body=\(responseText.prefix(200))")

                if statusCode == 200 {
                    let json = try JSONSerialization.jsonObject(with: responseData) as? [String: Any]
                    if let url = json?["url"] as? String {
                        await MainActor.run {
                            appState.sendMessage(target: channel, text: url)
                        }
                    }
                } else {
                    await MainActor.run {
                        appState.errorMessage = "Upload failed (HTTP \(statusCode)): \(responseText.prefix(100))"
                    }
                }
            } catch {
                print("[Upload] Error: \(error)")
                await MainActor.run {
                    appState.errorMessage = "Upload failed: \(error.localizedDescription)"
                }
            }

            await MainActor.run { uploading = false }
        }
    }
}
