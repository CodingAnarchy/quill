import Foundation
import Security

/// Where the author's git access tokens are kept (#1231). The app's real store
/// is the Keychain; tests use an in-memory one.
public protocol TokenStore: Sendable {
    func token(forHost host: String) -> String?
    func save(_ token: String, forHost host: String) throws
    func remove(forHost host: String)
}

/// Personal access tokens in the iOS Keychain, one per git host. Kept with
/// `AfterFirstUnlockThisDeviceOnly`, so a sync started while the phone is
/// locked can still read it, it survives app updates, and it never leaves the
/// device in a backup. The token is never written to a file or logged.
public struct KeychainTokenStore: TokenStore {
    private let service: String

    public init(service: String = "dev.quill.git-token") {
        self.service = service
    }

    public func token(forHost host: String) -> String? {
        var query = base(host)
        query[kSecReturnData as String] = true
        query[kSecMatchLimit as String] = kSecMatchLimitOne
        var item: CFTypeRef?
        guard SecItemCopyMatching(query as CFDictionary, &item) == errSecSuccess,
            let data = item as? Data
        else { return nil }
        return String(data: data, encoding: .utf8)
    }

    public func save(_ token: String, forHost host: String) throws {
        let data = Data(token.utf8)
        let update: [String: Any] = [
            kSecValueData as String: data,
            kSecAttrAccessible as String: kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly,
        ]
        var status = SecItemUpdate(base(host) as CFDictionary, update as CFDictionary)
        if status == errSecItemNotFound {
            var add = base(host)
            add.merge(update) { $1 }
            status = SecItemAdd(add as CFDictionary, nil)
        }
        guard status == errSecSuccess else {
            throw TokenStoreError(status: status)
        }
    }

    public func remove(forHost host: String) {
        SecItemDelete(base(host) as CFDictionary)
    }

    private func base(_ host: String) -> [String: Any] {
        [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: host,
        ]
    }
}

/// The Keychain refused to store a token.
public struct TokenStoreError: LocalizedError {
    public let status: OSStatus
    public var errorDescription: String? {
        "Couldn't save the token in the Keychain (error \(status))."
    }
}

/// The host a remote URL's token belongs to ("github.com").
public func tokenHost(for remote: String) -> String? {
    URL(string: remote.trimmingCharacters(in: .whitespaces))?.host()?.lowercased()
}

/// Tokens in memory, for tests and previews.
public final class MemoryTokenStore: TokenStore, @unchecked Sendable {
    private var tokens: [String: String] = [:]
    private let lock = NSLock()

    public init() {}

    public func token(forHost host: String) -> String? {
        lock.withLock { tokens[host] }
    }

    public func save(_ token: String, forHost host: String) throws {
        lock.withLock { tokens[host] = token }
    }

    public func remove(forHost host: String) {
        _ = lock.withLock { tokens.removeValue(forKey: host) }
    }
}
