import QuillKit
import SwiftUI
import UIKit

/// App-wide state: the library of projects on the phone, the open project, and
/// which chapter is selected.
@MainActor
@Observable
final class AppState {
    let store = ProjectStore()
    let library: Library
    /// The selected chapter's index in the active book. Changing it saves
    /// the chapter being left and opens the new one for editing.
    var selectedChapter: UInt32? {
        didSet {
            if selectedChapter != oldValue { openSelectedChapter() }
        }
    }
    /// The open chapter's editor and text view.
    private(set) var prose: ProseHost?
    /// Why the selected chapter couldn't be opened, if it couldn't.
    private(set) var chapterError: String?

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
        prose = nil
        store.open(folder)
        if store.error == nil {
            UserDefaults.standard.set(folder.lastPathComponent, forKey: Self.lastProjectKey)
        }
    }

    /// Back to the library.
    func closeProject() {
        selectedChapter = nil
        prose = nil
        store.close()
        UserDefaults.standard.removeObject(forKey: Self.lastProjectKey)
        library.refresh()
    }

    /// Pull the remote's changes into the open project, using the token saved
    /// for its host. Returns the message to show. The open chapter is saved
    /// first and reopened after, so it shows what the pull brought in.
    func pull() async -> String {
        save()
        library.useToken(for: store.remote)
        defer { reopenChapter() }
        do {
            return try await store.pull()
        } catch {
            return describe(error)
        }
    }

    /// Save the open chapter now.
    func save() {
        prose?.editor.save()
    }

    /// Save the open chapter as the app leaves the foreground, inside a
    /// background task so iOS lets the write finish (#1232).
    func saveForBackground() {
        let task = UIApplication.shared.beginBackgroundTask(withName: "Save chapter")
        save()
        UIApplication.shared.endBackgroundTask(task)
    }

    private func openSelectedChapter() {
        save()
        prose = nil
        chapterError = nil
        guard let index = selectedChapter,
            let chapter = store.chapters.first(where: { $0.index == index })
        else { return }
        do {
            prose = ProseHost(editor: try store.editor(for: chapter))
        } catch {
            chapterError = describe(error)
        }
    }

    /// Reopen the selected chapter from disk (after a pull rewrote it).
    private func reopenChapter() {
        guard selectedChapter != nil else { return }
        openSelectedChapter()
    }
}
