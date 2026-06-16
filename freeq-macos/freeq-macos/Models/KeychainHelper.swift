import Foundation
import LocalAuthentication
import Security

/// Simple Keychain wrapper for storing sensitive strings.
enum KeychainHelper {
    static let service = "at.freeq.macos"

    static func baseQuery(key: String) -> [String: Any] {
        [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: key,
            // Use the modern data-protection keychain instead of the
            // legacy macOS keychain ACL path. The legacy path prompts
            // repeatedly for rebuilt/dev-signed apps.
            kSecUseDataProtectionKeychain as String: true,
        ]
    }

    static func noninteractiveContext() -> LAContext {
        let context = LAContext()
        context.interactionNotAllowed = true
        return context
    }

    static func loadQuery(key: String) -> [String: Any] {
        var query = baseQuery(key: key)
        query[kSecReturnData as String] = true
        query[kSecMatchLimit as String] = kSecMatchLimitOne
        query[kSecUseAuthenticationContext as String] = noninteractiveContext()
        return query
    }

    /// Persist `value` for `key`. Returns true on success. Callers
    /// MUST check the return — silent failure leaves the user with an
    /// unauthed restart loop (e.g. locked keychain, quota, sandbox
    /// permission denial), which the prior implementation hid.
    @discardableResult
    static func save(key: String, value: String) -> Bool {
        guard let data = value.data(using: .utf8) else { return false }
        let query = baseQuery(key: key)
        let attributes: [String: Any] = [
            kSecValueData as String: data,
            kSecAttrAccessible as String: kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly,
        ]

        let updateStatus = SecItemUpdate(query as CFDictionary, attributes as CFDictionary)
        if updateStatus == errSecSuccess { return true }
        guard updateStatus == errSecItemNotFound else {
            Log.auth.error("KeychainHelper.update failed key=\(key, privacy: .public) status=\(updateStatus, privacy: .public)")
            return false
        }

        var add = query
        for (attributeKey, value) in attributes {
            add[attributeKey] = value
        }
        let status = SecItemAdd(add as CFDictionary, nil)
        if status != errSecSuccess {
            Log.auth.error("KeychainHelper.save failed key=\(key, privacy: .public) status=\(status, privacy: .public)")
            return false
        }
        return true
    }

    static func load(key: String) -> String? {
        let query = loadQuery(key: key)
        var result: AnyObject?
        let status = SecItemCopyMatching(query as CFDictionary, &result)
        guard status == errSecSuccess, let data = result as? Data else { return nil }
        return String(data: data, encoding: .utf8)
    }

    static func delete(key: String) {
        let query = baseQuery(key: key)
        SecItemDelete(query as CFDictionary)
    }
}
