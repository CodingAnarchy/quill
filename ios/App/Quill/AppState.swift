import QuillKit
import SwiftUI

/// App-wide state: the library of projects on the phone, the open project, and
/// which chapter is selected.
@MainActor
@Observable
final class AppState {
    let store = ProjectStore()
    let library: Library
    /// The selected chapter's index in the active book.
    var selectedChapter: UInt32?

    private static let lastProjectKey = "lastProject"
    private var launched = false

    init() {
        let support = URL.applicationSupportDirectory.appending(path: "Quill", directoryHint: .isDirectory)
        Core.configure(supportDirectory: support)
        library = Library(
            folder: URL.documentsDirectory.appending(path: "Projects", directoryHint: .isDirectory),
            tokens: KeychainTokenStore())
    }

    /// First launch installs the sample novel; every launch reopens the last
    /// project, if it's still there.
    func launch() {
        guard !launched else { return }
        launched = true
        if let bundled = Bundle.main.url(forResource: "SampleNovel", withExtension: nil) {
            _ = try? SampleNovel.install(from: bundled, into: library.folder)
            library.refresh()
        }
        let last = UserDefaults.standard.string(forKey: Self.lastProjectKey)
        if let entry = library.projects.first(where: { $0.name == last }) {
            open(entry.url)
        }
    }

    func open(_ folder: URL) {
        selectedChapter = nil
        store.open(folder)
        if store.error == nil {
            UserDefaults.standard.set(folder.lastPathComponent, forKey: Self.lastProjectKey)
        }
    }

    /// Back to the library.
    func closeProject() {
        selectedChapter = nil
        store.close()
        UserDefaults.standard.removeObject(forKey: Self.lastProjectKey)
        library.refresh()
    }

    /// Pull the remote's changes into the open project, using the token saved
    /// for its host. Returns the message to show.
    func pull() async -> String {
        library.useToken(for: store.remote)
        do {
            return try await store.pull()
        } catch {
            return describe(error)
        }
    }
}
