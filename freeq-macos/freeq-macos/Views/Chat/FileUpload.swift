import SwiftUI
import UniformTypeIdentifiers

/// Handles image/file uploads to the freeq server.
enum FileUploader {
    /// Upload a file to the server, returns the blob URL.
    static func upload(
        data: Data,
        filename: String,
        contentType: String,
        did: String,
        channel: String?
    ) async throws -> String {
        let boundary = UUID().uuidString
        var body = Data()

        // file field
        body.append("--\(boundary)\r\n".data(using: .utf8)!)
        body.append("Content-Disposition: form-data; name=\"file\"; filename=\"\(filename)\"\r\n".data(using: .utf8)!)
        body.append("Content-Type: \(contentType)\r\n\r\n".data(using: .utf8)!)
        body.append(data)
        body.append("\r\n".data(using: .utf8)!)

        // did field
        body.append("--\(boundary)\r\n".data(using: .utf8)!)
        body.append("Content-Disposition: form-data; name=\"did\"\r\n\r\n".data(using: .utf8)!)
        body.append(did.data(using: .utf8)!)
        body.append("\r\n".data(using: .utf8)!)

        // channel field
        if let channel {
            body.append("--\(boundary)\r\n".data(using: .utf8)!)
            body.append("Content-Disposition: form-data; name=\"channel\"\r\n\r\n".data(using: .utf8)!)
            body.append(channel.data(using: .utf8)!)
            body.append("\r\n".data(using: .utf8)!)
        }

        body.append("--\(boundary)--\r\n".data(using: .utf8)!)

        var request = URLRequest(url: URL(string: "https://irc.freeq.at/api/v1/upload")!)
        request.httpMethod = "POST"
        request.setValue("multipart/form-data; boundary=\(boundary)", forHTTPHeaderField: "Content-Type")
        request.httpBody = body

        let (responseData, response) = try await URLSession.shared.data(for: request)
        let httpResponse = response as! HTTPURLResponse

        guard httpResponse.statusCode == 200 else {
            let errorText = String(data: responseData, encoding: .utf8) ?? "Upload failed"
            throw UploadError.serverError(errorText)
        }

        let json = try JSONSerialization.jsonObject(with: responseData) as? [String: Any] ?? [:]
        guard let url = json["url"] as? String else {
            throw UploadError.serverError("No URL in response")
        }
        return url
    }

    enum UploadError: Error, LocalizedError {
        case serverError(String)
        var errorDescription: String? {
            switch self {
            case .serverError(let msg): return msg
            }
        }
    }
}

/// Pending upload state for the compose bar.
struct PendingUpload: Identifiable {
    let id = UUID()
    let data: Data
    let filename: String
    let contentType: String
    let preview: NSImage?
    var uploading: Bool = false
    var error: String?
}
