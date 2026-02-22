import SwiftUI
import PhotosUI

/// Wrapper so we can use sheet(item:) — passes image data directly.
struct StagedPhoto: Identifiable {
    let id = UUID()
    let image: UIImage
    let data: Data
}

/// Photo picker button — opens picker, stages image for preview before send.
struct PhotoPickerButton: View {
    @EnvironmentObject var appState: AppState
    @State private var selectedItem: PhotosPickerItem? = nil
    @State private var stagedPhoto: StagedPhoto? = nil

    let channel: String

    var body: some View {
        PhotosPicker(selection: $selectedItem, matching: .images) {
            Image(systemName: "photo.on.rectangle.angled")
                .font(.system(size: 20))
                .foregroundColor(Theme.accent)
        }
        .onChange(of: selectedItem) {
            if let item = selectedItem {
                loadPhoto(item)
                selectedItem = nil
            }
        }
        .sheet(item: $stagedPhoto) { photo in
            ImagePreviewSheet(
                image: photo.image,
                imageData: photo.data,
                channel: channel
            )
        }
    }

    private func loadPhoto(_ item: PhotosPickerItem) {
        Task {
            guard let data = try? await item.loadTransferable(type: Data.self) else { return }
            guard let uiImage = UIImage(data: data) else { return }
            await MainActor.run {
                stagedPhoto = StagedPhoto(image: uiImage, data: data)
            }
        }
    }
}

/// Preview sheet — shows the image with optional caption, upload progress, and send button.
struct ImagePreviewSheet: View {
    @EnvironmentObject var appState: AppState
    @Environment(\.dismiss) var dismiss
    let image: UIImage
    let imageData: Data
    let channel: String

    @State private var caption: String = ""
    @State private var crossPost = false
    @State private var uploading = false
    @State private var uploadProgress: String = ""
    @FocusState private var captionFocused: Bool

    var body: some View {
        NavigationView {
            ZStack {
                Theme.bgPrimary.ignoresSafeArea()

                VStack(spacing: 0) {
                    // Image preview
                    Image(uiImage: image)
                        .resizable()
                        .scaledToFit()
                        .frame(maxHeight: 400)
                        .clipShape(RoundedRectangle(cornerRadius: 12))
                        .padding(16)

                    // Cross-post toggle
                    if appState.authenticatedDID != nil {
                        Toggle(isOn: $crossPost) {
                            HStack(spacing: 8) {
                                Image(systemName: "arrow.up.right.circle.fill")
                                    .font(.system(size: 16))
                                    .foregroundColor(Color(hex: "0085ff"))
                                Text("Also post to Bluesky")
                                    .font(.system(size: 14, weight: .medium))
                                    .foregroundColor(Theme.textPrimary)
                            }
                        }
                        .tint(Color(hex: "0085ff"))
                        .padding(.horizontal, 20)
                        .padding(.vertical, 8)
                    }

                    Spacer()

                    // Caption input + send
                    VStack(spacing: 0) {
                        Rectangle().fill(Theme.border).frame(height: 1)

                        if uploading {
                            HStack(spacing: 12) {
                                ProgressView()
                                    .tint(Theme.accent)
                                Text(uploadProgress.isEmpty ? "Uploading..." : uploadProgress)
                                    .font(.system(size: 14))
                                    .foregroundColor(Theme.textSecondary)
                            }
                            .frame(maxWidth: .infinity)
                            .padding(.vertical, 16)
                            .background(Theme.bgSecondary)
                        } else {
                            HStack(alignment: .bottom, spacing: 10) {
                                TextField("Add a caption...", text: $caption, axis: .vertical)
                                    .foregroundColor(Theme.textPrimary)
                                    .font(.system(size: 16))
                                    .lineLimit(1...4)
                                    .focused($captionFocused)
                                    .tint(Theme.accent)

                                Button(action: upload) {
                                    ZStack {
                                        Circle()
                                            .fill(Theme.accent)
                                            .frame(width: 36, height: 36)
                                        Image(systemName: "arrow.up")
                                            .font(.system(size: 15, weight: .bold))
                                            .foregroundColor(.white)
                                    }
                                }
                            }
                            .padding(.horizontal, 16)
                            .padding(.vertical, 12)
                            .background(Theme.bgSecondary)
                        }
                    }
                }
            }
            .navigationTitle("Send Photo")
            .navigationBarTitleDisplayMode(.inline)
            .toolbarBackground(Theme.bgSecondary, for: .navigationBar)
            .toolbarBackground(.visible, for: .navigationBar)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") { dismiss() }
                        .foregroundColor(Theme.accent)
                        .disabled(uploading)
                }
            }
        }
        .preferredColorScheme(.dark)
        .interactiveDismissDisabled(uploading)
    }

    private func upload() {
        uploading = true
        uploadProgress = "Uploading \(ByteCountFormatter.string(fromByteCount: Int64(imageData.count), countStyle: .file))..."

        Task {
            let did = appState.authenticatedDID ?? ""
            let serverBase = "https://irc.freeq.at"

            let boundary = UUID().uuidString
            var body = Data()

            body.append("--\(boundary)\r\n".data(using: .utf8)!)
            body.append("Content-Disposition: form-data; name=\"did\"\r\n\r\n\(did)\r\n".data(using: .utf8)!)

            body.append("--\(boundary)\r\n".data(using: .utf8)!)
            body.append("Content-Disposition: form-data; name=\"channel\"\r\n\r\n\(channel)\r\n".data(using: .utf8)!)

            if crossPost {
                body.append("--\(boundary)\r\n".data(using: .utf8)!)
                body.append("Content-Disposition: form-data; name=\"cross_post\"\r\n\r\ntrue\r\n".data(using: .utf8)!)
            }

            body.append("--\(boundary)\r\n".data(using: .utf8)!)
            body.append("Content-Disposition: form-data; name=\"file\"; filename=\"photo.jpg\"\r\nContent-Type: image/jpeg\r\n\r\n".data(using: .utf8)!)
            body.append(imageData)
            body.append("\r\n--\(boundary)--\r\n".data(using: .utf8)!)

            var request = URLRequest(url: URL(string: "\(serverBase)/api/v1/upload")!)
            request.httpMethod = "POST"
            request.setValue("multipart/form-data; boundary=\(boundary)", forHTTPHeaderField: "Content-Type")
            request.httpBody = body

            do {
                let (responseData, response) = try await URLSession.shared.data(for: request)
                let statusCode = (response as? HTTPURLResponse)?.statusCode ?? 0

                if statusCode == 200 {
                    let json = try JSONSerialization.jsonObject(with: responseData) as? [String: Any]
                    if let url = json?["url"] as? String {
                        await MainActor.run {
                            // Send image URL, then caption if provided
                            let text = caption.trimmingCharacters(in: .whitespacesAndNewlines)
                            if text.isEmpty {
                                appState.sendMessage(target: channel, text: url)
                            } else {
                                appState.sendMessage(target: channel, text: "\(url) \(text)")
                            }
                            UIImpactFeedbackGenerator(style: .medium).impactOccurred()
                            dismiss()
                        }
                    }
                } else {
                    let responseText = String(data: responseData, encoding: .utf8) ?? ""
                    await MainActor.run {
                        uploading = false
                        uploadProgress = "Failed: \(responseText.prefix(80))"
                    }
                }
            } catch {
                await MainActor.run {
                    uploading = false
                    uploadProgress = "Error: \(error.localizedDescription)"
                }
            }
        }
    }
}
