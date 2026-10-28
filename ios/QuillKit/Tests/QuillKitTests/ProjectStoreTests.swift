import Foundation
import Testing

@testable import QuillKit

/// A scratch folder unique to one test.
func scratch(_ name: String) throws -> URL {
    let dir = FileManager.default.temporaryDirectory
        .appendingPathComponent("quillkit-\(ProcessInfo.processInfo.processIdentifier)-\(name)")
    try? FileManager.default.removeItem(at: dir)
    try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
    return dir
}

/// A two-chapter novel; the first chapter carries front matter.
func writeNovel(in dir: URL) throws {
    try """
    title = "The Lighthouse"

    [[chapters]]
    title = "Arrival"
    file = "arrival.md"

    [[chapters]]
    title = "Storm"
    file = "storm.md"

    """.write(to: dir.appendingPathComponent("book.toml"), atomically: true, encoding: .utf8)
    try "---\nstatus: draft\n---\nThe boat came in at dusk.\n"
        .write(to: dir.appendingPathComponent("arrival.md"), atomically: true, encoding: .utf8)
    try "Wind. Then rain.\n"
        .write(to: dir.appendingPathComponent("storm.md"), atomically: true, encoding: .utf8)
}

@MainActor
@Suite struct ProjectStoreTests {
    @Test func opensANovelAndListsItsChapters() throws {
        let dir = try scratch("opens")
        try writeNovel(in: dir)
        let store = ProjectStore()
        store.open(dir)

        #expect(store.error == nil)
        #expect(store.info?.title == "The Lighthouse")
        #expect(store.info?.form == .novel)
        #expect(store.chapters.map(\.title) == ["Arrival", "Storm"])
        #expect(store.chapters.map(\.words) == [6, 3])
        #expect(store.chapters.first?.status == "Draft")
    }

    @Test func chapterTextHidesFrontMatter() throws {
        let dir = try scratch("text")
        try writeNovel(in: dir)
        let store = ProjectStore()
        store.open(dir)
        let text = try store.text(of: store.chapters[0])
        #expect(text == "The boat came in at dusk.\n")
    }

    @Test func aMissingProjectReportsWhatWentWrong() throws {
        let dir = try scratch("missing")
        let store = ProjectStore()
        store.open(dir.appendingPathComponent("nowhere"))
        #expect(store.info == nil)
        let error = try #require(store.error)
        #expect(error.contains("book.toml"))
        #expect(!error.contains("QuillError"))
    }
}

@Suite struct FormatTests {
    @Test func wordsAreGroupedLikeTheDesktop() {
        #expect(Format.words(1) == "1 word")
        #expect(Format.words(0) == "0 words")
        #expect(Format.words(12_345) == "12,345 words")
    }
}

@Suite struct SampleNovelTests {
    @Test func installsACopyOnce() throws {
        let root = try scratch("sample")
        let bundled = root.appendingPathComponent("Sample")
        try FileManager.default.createDirectory(at: bundled, withIntermediateDirectories: true)
        try writeNovel(in: bundled)
        let projects = root.appendingPathComponent("Projects")

        let copy = try SampleNovel.install(from: bundled, into: projects)
        #expect(FileManager.default.fileExists(atPath: copy.appendingPathComponent("storm.md").path))
        // An author's edit to the copy survives a second launch.
        try "Edited.\n".write(to: copy.appendingPathComponent("storm.md"), atomically: true, encoding: .utf8)
        _ = try SampleNovel.install(from: bundled, into: projects)
        #expect(try String(contentsOf: copy.appendingPathComponent("storm.md"), encoding: .utf8) == "Edited.\n")
    }
}
