import Foundation
import Security

/// Simple Keychain wrapper for storing secrets.
/// Stores UTF-8 strings under the app's default access group.
enum KeychainHelper {
    private static let service = "at.freeq.app"

    static func save(key: String, value: String) {
        guard let data = value.data(using: .utf8) else { return }
        // Delete any existing item first
        let deleteQuery: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: key,
        ]
        SecItemDelete(deleteQuery as CFDictionary)

        let addQuery: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: key,
            kSecValueData as String: data,
            kSecAttrAccessible as String: kSecAttrAccessibleAfterFirstUnlock,
        ]
        SecItemAdd(addQuery as CFDictionary, nil)
    }

    static func load(key: String) -> String? {
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: key,
            kSecReturnData as String: true,
            kSecMatchLimit as String: kSecMatchLimitOne,
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

    // MARK: - Migration from UserDefaults

    /// Migrate a value from UserDefaults to Keychain (one-time).
    /// Removes the UserDefaults entry after successful migration.
    static func migrateFromUserDefaults(userDefaultsKey: String, keychainKey: String) {
        if let value = UserDefaults.standard.string(forKey: userDefaultsKey) {
            // Only migrate if not already in Keychain
            if load(key: keychainKey) == nil {
                save(key: keychainKey, value: value)
            }
            UserDefaults.standard.removeObject(forKey: userDefaultsKey)
        }
    }
}
