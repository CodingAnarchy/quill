import Foundation
import Testing

@testable import QuillKit

/// Starting a book on the phone (#1297): a novel or a short story, with
/// version history from the start and snapshots after that.
@MainActor
@Suite struct NewBookTests {
    @Test func aNewNovelOpensWithOneChapterAndHistory() async throws {
        let root = try scratch("new-novel")
        let library = Library(folder: root, tokens: MemoryTokenStore())
        let folder = try await library.create(title: "The Lamp Room", form: .novel)

        #expect(library.projects.map(\.name) == ["The Lamp Room"])
        let store = ProjectStore()
        store.open(folder)
        #expect(store.info?.title == "The Lamp Room")
        #expect(store.chapters.count == 1)
        let log = try git("log", "--format=%s", in: folder)
        #expect(log == "Start of version history\n")
    }

    @Test func aNewShortStoryOpensOnTheStory() async throws {
        let root = try scratch("new-story")
        let library = Library(folder: root, tokens: MemoryTokenStore())
        let folder = try await library.create(title: "Low Tide", form: .shortStory)
        let store = ProjectStore()
        store.open(folder)
        #expect(store.info?.form == .shortStory)
        #expect(store.info?.storyChapter != nil)
    }

    @Test func aSecondBookWithTheSameTitleIsRefused() async throws {
        let root = try scratch("new-twice")
        let library = Library(folder: root, tokens: MemoryTokenStore())
        _ = try await library.create(title: "Twice", form: .novel)
        await #expect(throws: QuillError.self) {
            _ = try await library.create(title: "Twice", form: .novel)
        }
    }

    @Test func aSavedEditIsSnapshotted() async throws {
        let root = try scratch("new-snapshot")
        let library = Library(folder: root, tokens: MemoryTokenStore())
        let folder = try await library.create(title: "Tide", form: .novel)
        let store = ProjectStore()
        store.open(folder)
        let chapter = try #require(store.chapters.first)
        let editor = try store.editor(for: chapter)
        editor.apply(range: NSRange(location: 0, length: 0), replacement: "The sea came in.")
        editor.save()

        #expect(try await store.snapshot())
        #expect(try await !store.snapshot(), "nothing new the second time")
        let log = try git("log", "--format=%s", in: folder)
        #expect(log.hasPrefix("Snapshot, "))
    }
}

/// A chapter's earlier versions on the phone (#1299).
@MainActor
@Suite struct VersionTests {
    @Test func aChaptersVersionsAreListedAndReadBack() async throws {
        let root = try scratch("versions")
        let library = Library(folder: root, tokens: MemoryTokenStore())
        let folder = try await library.create(title: "Tide", form: .novel)
        let store = ProjectStore()
        store.open(folder)
        let chapter = try #require(store.chapters.first)
        let editor = try store.editor(for: chapter)
        editor.apply(range: NSRange(location: 0, length: 0), replacement: "The sea came in.")
        editor.save()
        try await store.snapshot()

        let versions = try await store.versions(of: chapter)
        #expect(versions.count == 2)
        #expect(versions.first?.summary.hasPrefix("Snapshot, ") == true)
        let first = try #require(versions.last)
        #expect(try await store.text(of: chapter, at: first) == "")
        let latest = try #require(versions.first)
        let saved = try await store.text(of: chapter, at: latest)
        #expect(saved.trimmingCharacters(in: .newlines) == "The sea came in.", "\(saved)")

        let runs = compareVersions(old: "The sea came in.", new: "The sea went out.")
        #expect(runs.map(\.kind) == [.same, .removed, .added])
    }

    @Test func aBookIsDeletedFromThePhone() async throws {
        let root = try scratch("delete")
        let library = Library(folder: root, tokens: MemoryTokenStore())
        _ = try await library.create(title: "Gone", form: .novel)
        let entry = try #require(library.projects.first)
        try library.delete(entry)
        #expect(library.projects.isEmpty)
        #expect(!FileManager.default.fileExists(atPath: entry.url.path))
    }
}

/// Exporting from the phone (#1300).
@MainActor
@Suite struct ExportTests {
    @Test func aBookExportsInEveryKind() async throws {
        let root = try scratch("export")
        let library = Library(folder: root, tokens: MemoryTokenStore())
        let folder = try await library.create(title: "Tide", form: .novel)
        let store = ProjectStore()
        store.open(folder)
        let out = root.appendingPathComponent("out")
        for kind in [ExportKind.manuscript, .epub, .pdf, .printPdf] {
            let result = try await store.export(kind, to: out)
            #expect(FileManager.default.fileExists(atPath: result.path))
            #expect(URL(fileURLWithPath: result.path).lastPathComponent.hasPrefix("Tide."))
        }
        #expect(!store.exporting)
    }
}

/// A book on more than one device (#1298), with plain folders standing in
/// for iCloud Drive.
@MainActor
@Suite struct SyncedBookTests {
    @Test func newBooksGoToTheCloudFolderAndBothFoldersAreListed() async throws {
        let root = try scratch("cloud-library")
        let local = root.appendingPathComponent("local")
        let cloud = root.appendingPathComponent("cloud")
        let library = Library(folder: local, tokens: MemoryTokenStore())
        _ = try await library.create(title: "Here", form: .novel)
        library.useCloud(cloud)
        let synced = try await library.create(title: "There", form: .novel)
        #expect(synced.deletingLastPathComponent().standardizedFileURL == cloud.standardizedFileURL)
        #expect(library.projects.map(\.name) == ["Here", "There"])
        #expect(library.projects.map(\.inCloud) == [false, true])
    }

    @Test func aChapterChangedElsewhereIsNoticedAndItsVersionKept() async throws {
        let root = try scratch("changed-elsewhere")
        let library = Library(folder: root, tokens: MemoryTokenStore())
        let folder = try await library.create(title: "Tide", form: .novel)
        let store = ProjectStore()
        store.open(folder)
        let chapter = try #require(store.chapters.first)
        let editor = try store.editor(for: chapter)
        #expect(!editor.changedOnDisk())

        try "The Mac's words.\n".write(
            to: folder.appending(path: chapter.file), atomically: true, encoding: .utf8)
        #expect(editor.changedOnDisk())
        let copy = try editor.keepDiskVersion()
        #expect(copy.hasSuffix(" 2.md"))
        #expect(store.conflictCopies().map(\.copy) == [copy])
    }

    @Test func iCloudsOtherVersionIsKeptUnderTheNextFreeName() throws {
        let root = try scratch("cloud-copy")
        let chapter = root.appendingPathComponent("ledger.md")
        try "Mine.\n".write(to: chapter, atomically: true, encoding: .utf8)
        let first = try CloudConflicts.keepCopy(of: chapter, text: "Theirs.\n")
        let second = try CloudConflicts.keepCopy(of: chapter, text: "Again.\n")
        #expect(first.lastPathComponent == "ledger 2.md")
        #expect(second.lastPathComponent == "ledger 3.md")
        #expect(try String(contentsOf: first, encoding: .utf8) == "Theirs.\n")
    }
}
