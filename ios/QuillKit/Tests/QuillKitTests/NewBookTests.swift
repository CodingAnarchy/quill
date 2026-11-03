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
