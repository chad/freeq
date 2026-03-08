import Foundation

/// Manages E2EE session via FreeqE2ee FFI.
@Observable
class E2eeManager {
    static let shared = E2eeManager()

    private var e2ee: FreeqE2ee?
    var isInitialized: Bool { e2ee != nil }
    var publicKey: String?  // Our identity key (base64)
    var sessions: Set<String> = []  // DIDs we have sessions with

    /// Initialize E2EE — generate or restore keys.
    func initialize() throws {
        let e = FreeqE2ee()

        // Try to restore saved keys
        if let ikSecret = KeychainHelper.load(key: "e2ee.ik"),
           let spkSecret = KeychainHelper.load(key: "e2ee.spk") {
            let bundle = try e.restoreKeys(ikSecretB64: ikSecret, spkSecretB64: spkSecret)
            publicKey = bundle.identityKey
            Log.auth.info("E2EE: Restored keys")
        } else {
            // Generate new keypair
            let bundle = try e.generateKeys()
            publicKey = bundle.identityKey
            // Save secrets to keychain
            let keys = try e.exportKeys()
            if keys.count >= 2 {
                KeychainHelper.save(key: "e2ee.ik", value: keys[0])
                KeychainHelper.save(key: "e2ee.spk", value: keys[1])
            }
            Log.auth.info("E2EE: Generated new keys")
        }

        self.e2ee = e
    }

    /// Establish a session with a remote DID.
    func establishSession(remoteDid: String, theirIk: String, theirSpk: String) throws {
        guard let e2ee else { throw FreeqError.InvalidArgument(message: "E2EE not initialized") }
        try e2ee.establishSession(remoteDid: remoteDid, theirIkB64: theirIk, theirSpkB64: theirSpk)
        sessions.insert(remoteDid)
        Log.auth.info("E2EE: Session established with \(remoteDid)")
    }

    /// Check if we have a session with a DID.
    func hasSession(remoteDid: String) -> Bool {
        e2ee?.hasSession(remoteDid: remoteDid) ?? false
    }

    /// Encrypt a message for a DID.
    func encrypt(remoteDid: String, plaintext: String) throws -> String {
        guard let e2ee else { throw FreeqError.InvalidArgument(message: "E2EE not initialized") }
        return try e2ee.encryptMessage(remoteDid: remoteDid, plaintext: plaintext)
    }

    /// Decrypt a message from a DID.
    func decrypt(remoteDid: String, ciphertext: String) throws -> String {
        guard let e2ee else { throw FreeqError.InvalidArgument(message: "E2EE not initialized") }
        return try e2ee.decryptMessage(remoteDid: remoteDid, wire: ciphertext)
    }

    /// Check if a message is encrypted.
    func isEncrypted(_ text: String) -> Bool {
        e2ee?.isEncrypted(text: text) ?? false
    }

    /// Get safety number for a session.
    func safetyNumber(remoteDid: String) -> String? {
        try? e2ee?.getSafetyNumber(remoteDid: remoteDid).number
    }
}
