import Foundation
import Testing

@testable import QuillKit

@MainActor
@Suite struct ShortStoryTests {
    @Test func aStoryOpensWithItsScenesAndLengthCategory() throws {
        let dir = try scratch("story")
        try """
        title = "Low Tide"
        form = "short-story"

        [[chapters]]
        title = "Low Tide"
        file = "story.md"

        """.write(to: dir.appendingPathComponent("book.toml"), atomically: true, encoding: .utf8)
        try "## Dawn\n\nLight.\n\n* * *\n\nThe boat came in.\n"
            .write(to: dir.appendingPathComponent("story.md"), atomically: true, encoding: .utf8)

        let store = ProjectStore()
        store.open(dir)
        let info = try #require(store.info)
        #expect(info.form == .shortStory)
        let index = try #require(info.storyChapter)

        let editor = try store.editor(for: store.chapters[Int(index)])
        #expect(editor.isStory)
        #expect(editor.lengthCategory == "flash fiction")
        #expect(editor.scenes().map(\.title) == ["Dawn", "The boat came in."])

        let before = editor.revision
        editor.apply(range: NSRange(location: editor.utf16Length, length: 0), replacement: "\n* * *\n\nNight.\n")
        #expect(editor.revision > before)
        #expect(editor.scenes().count == 3)
    }
}
