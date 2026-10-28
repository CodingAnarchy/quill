import Foundation
import Testing

@testable import QuillKit

@MainActor
@Suite struct BibleTests {
    @Test func readsTheBibleAndFindsTheCitedPassage() throws {
        let dir = try scratch("bible")
        try writeNovel(in: dir)
        let bible = dir.appendingPathComponent("bible")
        try FileManager.default.createDirectory(
            at: bible.appendingPathComponent("characters"), withIntermediateDirectories: true)
        try "Grey eyes.\n".write(
            to: bible.appendingPathComponent("characters/mara.md"), atomically: true, encoding: .utf8)
        try """
        [[findings]]
        summary = "weather: \\"Then rain\\" contradicts the clear sky"
        kind = "continuity"
        chapters = ["Storm"]

        """.write(to: bible.appendingPathComponent("findings.toml"), atomically: true, encoding: .utf8)

        let store = ProjectStore()
        store.open(dir)
        let pages = store.biblePages()
        #expect(pages.map(\.title) == ["Mara"])
        #expect(try store.biblePage(pages[0]) == "Grey eyes.\n")

        let finding = try #require(store.findings().first)
        #expect(finding.chapterIndex == 1)
        let range = try #require(store.passage(for: finding))
        #expect(range == ("Wind. Then rain.\n" as NSString).range(of: "Then rain"))
    }
}
