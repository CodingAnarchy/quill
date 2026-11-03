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
    /// How the iPhone Duo is held: open flat (or folded), or half open as a
    /// book or a laptop.
    var posture = Posture.flat
    /// The word lookup in progress (#1235), and the range it replaces.
    var lookup: (lookup: WordLookup, range: NSRange)?
    /// A finished export waiting for the share sheet (#1300).
    var exported: ExportedFile?
    /// A message the author must see (work parked on a draft, a failed manual
    /// sync), shown as an alert.
    var notice: String?

    private static let lastProjectKey = "lastProject"
    private var launched = false
    /// The background tasks keeping the app awake for a save and sync (one
    /// per trip to the background), each ended once.
    @ObservationIgnored private var backgroundSyncs: Set<UIBackgroundTaskIdentifier> = []

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
        // Quill's iCloud Drive folder, when iCloud is on and the app is signed
        // to use it (#1298). Looking it up can take a moment the first time, so
        // it's off the main thread; the library lists those books once it's in.
        Task {
            let cloud = await Task.detached {
                FileManager.default.url(forUbiquityContainerIdentifier: nil)?
                    .appending(path: "Documents", directoryHint: .isDirectory)
            }.value
            library.useCloud(cloud)
        }
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
        TouchTrace.note("open \(folder.lastPathComponent)")
        selectedChapter = nil
        prose = nil
        store.open(folder)
        if store.error == nil {
            UserDefaults.standard.set(folder.lastPathComponent, forKey: Self.lastProjectKey)
            // Every book keeps version history, as on the desktop (#1296).
            Task { await store.startHistory() }
        }
        // A short story opens straight into its prose (#1247).
        if let story = store.info?.storyChapter {
            selectedChapter = story
        }
    }

    /// Create a new book and open it on its first chapter (or the story),
    /// with the keyboard up, ready to type (#1297).
    func create(title: String, form: ProjectForm) async throws {
        let folder = try await library.create(title: title, form: form)
        open(folder)
        if selectedChapter == nil {
            selectedChapter = store.chapters.first?.index
        }
        prose?.textView.typeOnShow()
    }

    /// Export the open book as `kind` (#1300) and hand it to the share sheet.
    /// The open chapter is saved first, so the export has every word.
    func export(_ kind: ExportKind) async {
        save()
        let folder = URL.temporaryDirectory.appending(path: "Exports", directoryHint: .isDirectory)
        do {
            let result = try await store.export(kind, to: folder)
            exported = ExportedFile(
                url: URL(fileURLWithPath: result.path), warnings: result.warnings)
        } catch {
            notice = describe(error)
        }
    }

    /// Restore the open chapter to an earlier version's `text` (#1299): one
    /// edit in the editor, so Undo brings the chapter back, then saved and
    /// snapshotted, so the restore is the newest version and nothing in the
    /// history is lost.
    func restore(_ host: ProseHost, to text: String) {
        host.replaceAll(with: text)
        host.editor.save()
        Task { try? await store.snapshot() }
    }

    /// Move the cursor to the start of a story's scene (#1247).
    func reveal(_ scene: SceneInfo) {
        prose?.textView.reveal(NSRange(location: Int(scene.start), length: 0))
    }

    /// Back to the library.
    func closeProject() {
        TouchTrace.note("close the project")
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

    /// Save, snapshot, and sync as the app leaves the foreground, inside a
    /// background task so iOS gives the work time to finish. If time runs out first,
    /// the edits are already committed here and go out on the next sync.
    func saveAndSyncForBackground() {
        let task = UIApplication.shared.beginBackgroundTask(withName: "Save and sync") { [weak self] in
            // iOS calls this on the main thread when the app's time is up.
            MainActor.assumeIsolated { self?.endBackgroundSyncs() }
        }
        if task != .invalid { backgroundSyncs.insert(task) }
        save()
        Task {
            // A snapshot for the book's version history (#1297), then the
            // sync for a book with a remote.
            do {
                try await store.snapshot()
            } catch {
                syncStatus = "Version history not saved: \(describe(error))"
            }
            await sync(.background)
            endBackgroundSync(task)
        }
    }

    /// End a background save and sync's task, once: the sync finishing and
    /// iOS running out of time can both try.
    private func endBackgroundSync(_ task: UIBackgroundTaskIdentifier) {
        guard backgroundSyncs.remove(task) != nil else { return }
        UIApplication.shared.endBackgroundTask(task)
    }

    /// Time's up for the whole app, so every sync still running gives up its
    /// task. Their edits are committed; they go out on the next sync.
    private func endBackgroundSyncs() {
        backgroundSyncs.forEach(endBackgroundSync)
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
            // A chapter in iCloud saves through a file coordinator, and a file
            // presenter hears when another device changes it (#1298).
            if store.isSynced, let folder = store.url {
                let file = folder.appending(path: chapter.file)
                let presenter = ChapterFilePresenter(url: file) { [weak self] in
                    self?.checkForChangesElsewhere()
                }
                host.editor.coordinate = { write in presenter.coordinateWrite(write) }
                host.presenter = presenter
            }
            prose = host
        } catch {
            chapterError = describe(error)
        }
    }

    /// The chapter names already reported as changed on two devices (#1298).
    @ObservationIgnored private var conflictsNoted: Set<String> = []

    /// Pick up what another device changed (#1298), when the app comes back to
    /// the foreground or the open chapter's file changes under it: iCloud's
    /// conflicting versions are kept as copies beside their chapters; the open
    /// chapter shows the new version, or, with unsaved words in it, keeps the
    /// other version beside it; and the change is snapshotted into this
    /// phone's history.
    func checkForChangesElsewhere() {
        guard let folder = store.url else { return }
        if store.isSynced {
            CloudConflicts.keepCopies(in: folder, chapters: store.chapters.map(\.file))
        }
        var changed = false
        if let editor = prose?.editor, editor.changedOnDisk() {
            changed = true
            if editor.dirty {
                if let copy = try? editor.keepDiskVersion() {
                    notice =
                        "\u{201c}\(editor.chapter.title)\u{201d} also changed on another device. Your words are kept here; the other version is in \(copy)."
                }
            } else {
                reopenChapter()
                syncStatus = "Updated from another device"
            }
        }
        let copies = store.conflictCopies().filter { conflictsNoted.insert($0.copy).inserted }
        if let first = copies.first, notice == nil {
            notice =
                "\u{201c}\(first.chapterTitle)\u{201d} was changed on two devices before they synced. Both versions are kept: the other is in \(first.copy)."
        }
        store.refresh()
        if changed {
            Task { try? await store.snapshot() }
        }
    }

    /// Reopen the selected chapter from disk (after a pull rewrote it).
    private func reopenChapter() {
        guard selectedChapter != nil else { return }
        openSelectedChapter()
    }
}
