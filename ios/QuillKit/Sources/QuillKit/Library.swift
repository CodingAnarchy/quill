import Foundation
import Observation

/// The projects on this phone (#1231): the folders under `Projects/` that hold
/// a `book.toml`, and cloning a new one from the author's git remote.
@MainActor
@Observable
public final class Library {
    public struct Entry: Identifiable, Hashable, Sendable {
        public let url: URL
        public var id: URL { url }
        public var name: String { url.lastPathComponent }
    }

    public private(set) var projects: [Entry] = []
    /// True while a clone is running.
    public private(set) var cloning = false

    public let folder: URL
    private let tokens: TokenStore

    public init(folder: URL, tokens: TokenStore) {
        self.folder = folder
        self.tokens = tokens
        refresh()
    }

    /// Re-scan the projects folder.
    public func refresh() {
        let fm = FileManager.default
        let contents =
            (try? fm.contentsOfDirectory(
                at: folder, includingPropertiesForKeys: nil, options: [.skipsHiddenFiles])) ?? []
        projects =
            contents
            .filter { fm.fileExists(atPath: $0.appendingPathComponent("book.toml").path) }
            .sorted { $0.lastPathComponent.localizedStandardCompare($1.lastPathComponent) == .orderedAscending }
            .map { Entry(url: $0) }
    }

    /// Clone the project at `url`, keeping `token` in the token store for its
    /// host. Returns the new project's folder, or throws with a message that
    /// says what to fix; a failed clone leaves nothing behind.
    public func clone(url: String, token: String) async throws -> URL {
        guard let host = tokenHost(for: url) else {
            throw QuillError.Remote(message: "Enter the repository's HTTPS address (it starts with https://).")
        }
        let token = token.trimmingCharacters(in: .whitespacesAndNewlines)
        cloning = true
        defer { cloning = false }
        let projects = folder.path
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        let path = try await Task.detached {
            setGitToken(token: token.isEmpty ? nil : token)
            return try cloneProject(url: url, projectsDir: projects)
        }.value
        // Keep the token only once it has worked.
        if !token.isEmpty {
            try tokens.save(token, forHost: host)
        }
        refresh()
        return URL(fileURLWithPath: path, isDirectory: true)
    }

    /// Hand the core the saved token for `remote`'s host before a pull or sync.
    public func useToken(for remote: String?) {
        let token = remote.flatMap(tokenHost(for:)).flatMap(tokens.token(forHost:))
        setGitToken(token: token)
    }
}
