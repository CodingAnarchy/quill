import Foundation
import Observation

/// The projects on this phone (#1231): the folders under `Projects/` that hold
/// a `book.toml`, starting a new one (#1297), and cloning one from the
/// author's git remote.
@MainActor
@Observable
public final class Library {
    public struct Entry: Identifiable, Hashable, Sendable {
        public let url: URL
        /// Whether the book is in iCloud Drive (#1298).
        public var inCloud = false
        public var id: URL { url }
        public var name: String { url.lastPathComponent }
    }

    public private(set) var projects: [Entry] = []
    /// True while a clone is running.
    public private(set) var cloning = false

    /// The books on this phone only.
    public let folder: URL
    /// Quill's iCloud Drive folder, when iCloud is on and the app may use it
    /// (#1298). New books go here, so they sync with the Mac.
    public private(set) var cloudFolder: URL?
    private let tokens: TokenStore

    public init(folder: URL, tokens: TokenStore) {
        self.folder = folder
        self.tokens = tokens
        refresh()
    }

    /// Use `folder` (Quill's iCloud Drive folder, or `nil` when iCloud is off)
    /// for new books, and list the books in it alongside the phone's own.
    public func useCloud(_ folder: URL?) {
        cloudFolder = folder
        refresh()
    }

    /// Re-scan the projects folders.
    public func refresh() {
        let fm = FileManager.default
        func books(in folder: URL?, inCloud: Bool) -> [Entry] {
            guard let folder else { return [] }
            let contents =
                (try? fm.contentsOfDirectory(
                    at: folder, includingPropertiesForKeys: nil, options: [.skipsHiddenFiles])) ?? []
            return
                contents
                .filter { fm.fileExists(atPath: $0.appendingPathComponent("book.toml").path) }
                .map { Entry(url: $0, inCloud: inCloud) }
        }
        projects = (books(in: cloudFolder, inCloud: true) + books(in: folder, inCloud: false))
            .sorted { $0.name.localizedStandardCompare($1.name) == .orderedAscending }
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

    /// Create a new book called `title` (#1297), a novel or a short story, with
    /// version history from the start, and return its folder. Throws with a
    /// message that says what to fix (no title, or a book of that name already
    /// here).
    public func create(title: String, form: ProjectForm) async throws -> URL {
        // In iCloud Drive when it's on, so the book syncs with the Mac (#1298).
        let target = cloudFolder ?? folder
        let projects = target.path
        try FileManager.default.createDirectory(at: target, withIntermediateDirectories: true)
        let path = try await Task.detached {
            try createProject(title: title, form: form, projectsDir: projects)
        }.value
        refresh()
        return URL(fileURLWithPath: path, isDirectory: true)
    }

    /// Delete the book in `entry` from this phone: its folder, chapters,
    /// and history. A copy elsewhere (a Mac, a git remote) is untouched.
    public func delete(_ entry: Entry) throws {
        try FileManager.default.removeItem(at: entry.url)
        refresh()
    }

    /// Hand the core the saved token for `remote`'s host before a pull or sync.
    public func useToken(for remote: String?) {
        let token = remote.flatMap(tokenHost(for:)).flatMap(tokens.token(forHost:))
        setGitToken(token: token)
    }
}
