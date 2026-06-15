import Foundation
import LocalAuthentication
import Security

/// Simple Keychain wrapper for storing sensitive strings.
enum KeychainHelper {
    private static let service = "at.freeq.macos"

    /// Persist `value` for `key`. Returns true on success. Callers
    /// MUST check the return — silent failure leaves the user with an
    /// unauthed restart loop (e.g. locked keychain, quota, sandbox
    /// permission denial), which the prior implementation hid.
    @discardableResult
    static func save(key: String, value: String) -> Bool {
        guard let data = value.data(using: .utf8) else { return false }
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: key,
        ]
        SecItemDelete(query as CFDictionary)
        var add = query
        add[kSecValueData as String] = data
        let status = SecItemAdd(add as CFDictionary, nil)
        if status != errSecSuccess {
            Log.auth.error("KeychainHelper.save failed key=\(key, privacy: .public) status=\(status, privacy: .public)")
            return false
        }
        return true
    }

    static func load(key: String) -> String? {
        let context = LAContext()
        context.interactionNotAllowed = true

        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: key,
            kSecReturnData as String: true,
            kSecMatchLimit as String: kSecMatchLimitOne,
            kSecUseAuthenticationContext as String: context,
        ]
        var result: AnyObject?
        let status = SecItemCopyMatching(query as CFDictionary, &result)
        guard status == errSecSuccess, let data = result as? Data else { return nil }
        return String(data: data, encoding: .utf8)
    }

    static func delete(key: String) {
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: key,
        ]
        SecItemDelete(query as CFDictionary)
    }
}
