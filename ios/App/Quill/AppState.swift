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
    /// How the last sync went, for the chapter list's footer.
    private(set) var syncStatus: String?
    /// Whether the TODO markers list is showing (#1237).
    var showingMarkers = false
    /// The word lookup in progress (#1235), and the range it replaces.
    var lookup: (lookup: WordLookup, range: NSRange)?
    /// A message the author must see (work parked on a draft, a failed manual
    /// sync), shown as an alert.
    var notice: String?

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
        // A short story opens straight into its prose (#1247).
        if let story = store.info?.storyChapter {
            selectedChapter = story
        }
    }

    /// Move the cursor to the start of a story's scene (#1247).
    func reveal(_ scene: SceneInfo) {
        prose?.textView.reveal(NSRange(location: Int(scene.start), length: 0))
    }

    /// Back to the library.
    func closeProject() {
        selectedChapter = nil
        prose = nil
        store.close()
        UserDefaults.standard.removeObject(forKey: Self.lastProjectKey)
        library.refresh()
    }

    /// Why a sync is running: the author asked, or the app came to or left
    /// the foreground. Only an explicit sync reports failures with an alert;
    /// the automatic ones note them in the status line, so a phone without
    /// signal isn't nagged on every unlock.
    enum SyncTrigger { case manual, foreground, background }

    /// Sync the open project with its remote (#1233). The open chapter is
    /// saved first and reopened after, so it shows what the sync brought in.
    func sync(_ trigger: SyncTrigger) async {
        guard store.remote != nil else { return }
        save()
        library.useToken(for: store.remote)
        do {
            guard let report = try await store.sync(commitMessage: "Edits on iPhone") else { return }
            if report.reload { reopenChapter() }
            syncStatus = "Synced at \(Date.now.formatted(date: .omitted, time: .shortened))"
            if report.draft != nil || trigger == .manual {
                notice = report.message
            }
        } catch {
            syncStatus = "Not synced: \(describe(error))"
            if trigger == .manual { notice = describe(error) }
        }
    }

    /// Save and sync as the app leaves the foreground, inside a background
    /// task so iOS gives the work time to finish. If time runs out first,
    /// the edits are already committed here and go out on the next sync.
    func saveAndSyncForBackground() {
        let app = UIApplication.shared
        var task = UIBackgroundTaskIdentifier.invalid
        task = app.beginBackgroundTask(withName: "Save and sync") {
            app.endBackgroundTask(task)
        }
        save()
        Task {
            await sync(.background)
            app.endBackgroundTask(task)
        }
    }

    /// Open a finding's chapter at the passage it cites (#1236), selected and
    /// ready to edit; at the top when it quotes nothing that's there.
    func open(_ finding: FindingInfo) {
        guard let index = finding.chapterIndex else { return }
        let passage = store.passage(for: finding)
        if selectedChapter != index { selectedChapter = index }
        if let passage { prose?.textView.reveal(passage) }
    }

    /// Open a TODO marker's chapter with the marker selected (#1237).
    func open(_ marker: MarkerInfo) {
        if selectedChapter != marker.chapterIndex { selectedChapter = marker.chapterIndex }
        prose?.textView.reveal(NSRange(location: Int(marker.start), length: Int(marker.length)))
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
            let host = ProseHost(editor: try store.editor(for: chapter))
            host.onLookUp = { [weak self] selection, range, line in
                self?.lookup = (WordLookup(selection: selection, line: line), range)
            }
            host.onShowMarkers = { [weak self] in self?.showingMarkers = true }
            host.todoMarker = store.todoMarker
            prose = host
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
