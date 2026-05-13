import Foundation
import os.log
import Security

/// Simple Keychain wrapper for storing secrets.
/// Items use `kSecAttrAccessibleAfterFirstUnlock` so they survive reboots
/// and are readable in the background after the first unlock — matching
/// the buffer cache's `.completeFileProtection`. iCloud Keychain sync is
/// explicitly disabled: credentials stay on this device.
enum KeychainHelper {
    private static let service = "at.freeq.app"
    private static let log = Logger(subsystem: "at.freeq.ios", category: "keychain")

    @discardableResult
    static func save(key: String, value: String) -> Bool {
        guard let data = value.data(using: .utf8) else { return false }
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
            // Explicit defense in depth: never let Keychain items sync via
            // iCloud Keychain (different lifetime semantics, can vanish when
            // the user disables sync on another device).
            kSecAttrSynchronizable as String: false,
        ]
        let status = SecItemAdd(addQuery as CFDictionary, nil)
        if status != errSecSuccess {
            log.error("SecItemAdd failed for key=\(key, privacy: .public) status=\(status, privacy: .public)")
            return false
        }
        return true
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
    /// Removes the UserDefaults source ONLY after we've confirmed the
    /// Keychain copy is readable — a previous version of this function
    /// removed it unconditionally, which could lose the only copy of
    /// `brokerToken` on a botched upgrade.
    static func migrateFromUserDefaults(userDefaultsKey: String, keychainKey: String) {
        guard let value = UserDefaults.standard.string(forKey: userDefaultsKey) else { return }
        // Already migrated? Nothing to do — but DO clear the stale source.
        if load(key: keychainKey) != nil {
            UserDefaults.standard.removeObject(forKey: userDefaultsKey)
            return
        }
        guard save(key: keychainKey, value: value) else {
            log.error("Keychain migration save failed for key=\(keychainKey, privacy: .public) — leaving UserDefaults source in place")
            return
        }
        // Confirm round-trip before deleting the source. If the load fails
        // (data protection class refusal, simulator quirks, etc.), keep the
        // UserDefaults copy so the next launch can try again.
        guard load(key: keychainKey) == value else {
            log.error("Keychain migration round-trip mismatch for key=\(keychainKey, privacy: .public) — leaving UserDefaults source in place")
            return
        }
        UserDefaults.standard.removeObject(forKey: userDefaultsKey)
    }
}
