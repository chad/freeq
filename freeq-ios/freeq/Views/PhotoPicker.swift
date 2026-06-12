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
    // Private to the channel/DM by default. Two opt-in toggles are the only
    // way bytes leave freeq. Posting to the feed implies the PDS copy.
    @State private var sharePds = false
    @State private var shareBluesky = false
    @State private var uploading = false
    @State private var uploadError: String? = nil
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

                    // Sharing toggles — private to the channel by default.
                    if appState.authenticatedDID != nil {
                        VStack(alignment: .leading, spacing: 6) {
                            HStack(spacing: 6) {
                                Image(systemName: "lock.fill")
                                    .font(.system(size: 11))
                                    .foregroundColor(Theme.textMuted)
                                Text("Private to \(channel) by default")
                                    .font(.system(size: 12))
                                    .foregroundColor(Theme.textMuted)
                            }
                            Toggle(isOn: Binding(
                                get: { sharePds || shareBluesky },
                                set: { sharePds = $0 }
                            )) {
                                HStack(spacing: 8) {
                                    Image(systemName: "icloud.and.arrow.up.fill")
                                        .font(.system(size: 16))
                                        .foregroundColor(Color(hex: "0085ff"))
                                    Text("Save a public copy to my PDS")
                                        .font(.system(size: 14, weight: .medium))
                                        .foregroundColor(Theme.textPrimary)
                                }
                            }
                            .tint(Color(hex: "0085ff"))
                            .disabled(shareBluesky)
                            Toggle(isOn: $shareBluesky) {
                                HStack(spacing: 8) {
                                    Image(systemName: "arrow.up.right.circle.fill")
                                        .font(.system(size: 16))
                                        .foregroundColor(Color(hex: "0085ff"))
                                    Text("Also post to Bluesky feed")
                                        .font(.system(size: 14, weight: .medium))
                                        .foregroundColor(Theme.textPrimary)
                                }
                            }
                            .tint(Color(hex: "0085ff"))
                        }
                        .padding(.horizontal, 20)
                        .padding(.vertical, 8)
                    }

                    Spacer()

                    // Caption input + send
                    VStack(spacing: 0) {
                        Rectangle().fill(Theme.border).frame(height: 1)

                        // Error banner
                        if let error = uploadError {
                            HStack(spacing: 8) {
                                Image(systemName: "exclamationmark.triangle.fill")
                                    .font(.system(size: 12))
                                    .foregroundColor(Theme.danger)
                                Text(error)
                                    .font(.system(size: 13))
                                    .foregroundColor(Theme.danger)
                                    .lineLimit(2)
                                Spacer()
                                Button("Retry") {
                                    uploadError = nil
                                    upload()
                                }
                                .font(.system(size: 13, weight: .medium))
                                .foregroundColor(Theme.accent)
                            }
                            .padding(.horizontal, 16)
                            .padding(.vertical, 10)
                            .background(Theme.danger.opacity(0.1))
                        }

                        if uploading {
                            HStack(spacing: 12) {
                                ProgressView()
                                    .tint(Theme.accent)
                                Text("Uploading \(ByteCountFormatter.string(fromByteCount: Int64(imageData.count), countStyle: .file))...")
                                    .font(.system(size: 14))
                                    .foregroundColor(Theme.textSecondary)
                                Spacer()
                                Button("Cancel") {
                                    uploading = false
                                }
                                .font(.system(size: 13, weight: .medium))
                                .foregroundColor(Theme.textMuted)
                            }
                            .padding(.horizontal, 16)
                            .padding(.vertical, 12)
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
        uploadError = nil

        Task {
            let did = appState.authenticatedDID ?? ""
            let serverBase = ServerConfig.apiBaseUrl

            // Ensure server has a fresh web session for this DID by calling broker
            if let brokerToken = appState.brokerToken {
                do {
                    let brokerBase = appState.authBrokerBase
                    let sessionURL = URL(string: "\(brokerBase)/session")!
                    var sessionReq = URLRequest(url: sessionURL)
                    sessionReq.httpMethod = "POST"
                    sessionReq.setValue("application/json", forHTTPHeaderField: "Content-Type")
                    sessionReq.httpBody = try JSONSerialization.data(withJSONObject: ["broker_token": brokerToken])
                    let (_, sessionResp) = try await URLSession.shared.data(for: sessionReq)
                    let sessionStatus = (sessionResp as? HTTPURLResponse)?.statusCode ?? 0
                    if sessionStatus != 200 {
                        print("Broker session refresh returned \(sessionStatus)")
                    }
                } catch {
                    print("Broker session refresh failed: \(error)")
                }
            }

            let boundary = UUID().uuidString
            var body = Data()

            body.append("--\(boundary)\r\n".data(using: .utf8)!)
            body.append("Content-Disposition: form-data; name=\"did\"\r\n\r\n\(did)\r\n".data(using: .utf8)!)

            body.append("--\(boundary)\r\n".data(using: .utf8)!)
            body.append("Content-Disposition: form-data; name=\"channel\"\r\n\r\n\(channel)\r\n".data(using: .utf8)!)

            // share_bluesky implies share_pds (the feed embed references the blob).
            if sharePds || shareBluesky {
                body.append("--\(boundary)\r\n".data(using: .utf8)!)
                body.append("Content-Disposition: form-data; name=\"share_pds\"\r\n\r\ntrue\r\n".data(using: .utf8)!)
            }
            if shareBluesky {
                body.append("--\(boundary)\r\n".data(using: .utf8)!)
                body.append("Content-Disposition: form-data; name=\"share_bluesky\"\r\n\r\ntrue\r\n".data(using: .utf8)!)
            }

            body.append("--\(boundary)\r\n".data(using: .utf8)!)
            body.append("Content-Disposition: form-data; name=\"file\"; filename=\"photo.jpg\"\r\nContent-Type: image/jpeg\r\n\r\n".data(using: .utf8)!)
            body.append(imageData)
            body.append("\r\n--\(boundary)--\r\n".data(using: .utf8)!)

            var request = URLRequest(url: URL(string: "\(serverBase)/api/v1/upload")!)
            request.httpMethod = "POST"
            request.timeoutInterval = 30
            request.setValue("multipart/form-data; boundary=\(boundary)", forHTTPHeaderField: "Content-Type")
            request.httpBody = body

            // Step-up if the server says blob_upload needs a fresh PDS-grant.
            let result = await StepUpAuth.uploadWithStepUp(
                request: request, did: did, appState: appState
            )
            guard let (responseData, response) = result else {
                await MainActor.run {
                    uploading = false
                    uploadError = "Upload failed"
                }
                return
            }
            let statusCode = (response as? HTTPURLResponse)?.statusCode ?? 0

            if statusCode == 200 {
                let json = try? JSONSerialization.jsonObject(with: responseData) as? [String: Any]
                if let url = json?["url"] as? String {
                    await MainActor.run {
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
            } else if StepUpAuth.detectStepUpRequired(status: statusCode, body: responseData) != nil {
                // Step-up was offered and the user declined / cancelled —
                // surface that state instead of a generic upload error.
                await MainActor.run {
                    uploading = false
                    uploadError = "Image upload needs Bluesky permission. Tap Retry to try again."
                }
            } else {
                let responseText = String(data: responseData, encoding: .utf8) ?? ""
                await MainActor.run {
                    uploading = false
                    uploadError = "Upload failed: \(responseText.prefix(80))"
                }
            }
        }
    }
}
