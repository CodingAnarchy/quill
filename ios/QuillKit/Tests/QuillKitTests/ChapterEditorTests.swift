import Foundation
import Testing

@testable import QuillKit

@MainActor
@Suite struct ChapterEditorTests {
    @Test func editsSaveWithTheFrontMatterIntact() throws {
        let dir = try scratch("editor-save")
        try writeNovel(in: dir)
        let store = ProjectStore()
        store.open(dir)
        let editor = try store.editor(for: store.chapters[0])
        #expect(editor.initialText == "The boat came in at dusk.\n")

        let dusk = (editor.initialText as NSString).range(of: "dusk")
        editor.apply(range: dusk, replacement: "nightfall")
        #expect(editor.dirty)
        editor.save()
        #expect(!editor.dirty)

        let file = try String(contentsOf: dir.appendingPathComponent("arrival.md"), encoding: .utf8)
        #expect(file == "---\nstatus: draft\n---\nThe boat came in at nightfall.\n")
        // The chapter list picked up the save.
        #expect(store.chapters[0].words == 6)
    }

    @Test func utf16RangesAfterEmojiAndCombiningMarks() throws {
        let dir = try scratch("editor-utf16")
        try writeNovel(in: dir)
        try "🙂 cafe\u{301} soon\n".write(
            to: dir.appendingPathComponent("storm.md"), atomically: true, encoding: .utf8)
        let store = ProjectStore()
        store.open(dir)
        let editor = try store.editor(for: store.chapters[1])
        let text = editor.initialText as NSString
        #expect(editor.utf16Length == text.length)

        let soon = text.range(of: "soon")
        editor.apply(range: soon, replacement: "now 🌊")
        editor.save()
        let saved = try String(contentsOf: dir.appendingPathComponent("storm.md"), encoding: .utf8)
        #expect(saved == "🙂 cafe\u{301} now 🌊\n")
    }

    @Test func theWordCountIsLive() throws {
        let dir = try scratch("editor-words")
        try writeNovel(in: dir)
        let store = ProjectStore()
        store.open(dir)
        let editor = try store.editor(for: store.chapters[1])
        #expect(editor.wordCount == 3)
        editor.apply(range: NSRange(location: 16, length: 0), replacement: " And more.")
        #expect(editor.wordCount == 5)
    }

    @Test func aPauseInTypingSaves() async throws {
        let dir = try scratch("editor-autosave")
        try writeNovel(in: dir)
        let store = ProjectStore()
        store.open(dir)
        let editor = try store.editor(for: store.chapters[1])
        editor.saveDelay = .milliseconds(20)
        editor.apply(range: NSRange(location: 0, length: 4), replacement: "Gale")
        try await Task.sleep(for: .milliseconds(200))
        #expect(!editor.dirty)
        let saved = try String(contentsOf: dir.appendingPathComponent("storm.md"), encoding: .utf8)
        #expect(saved == "Gale. Then rain.\n")
    }

    @Test func stylesComeBackInUtf16() throws {
        let dir = try scratch("editor-styles")
        try writeNovel(in: dir)
        try "🙂 a *dark* sky\n".write(
            to: dir.appendingPathComponent("storm.md"), atomically: true, encoding: .utf8)
        let store = ProjectStore()
        store.open(dir)
        let editor = try store.editor(for: store.chapters[1])
        let spans = editor.styles(in: NSRange(location: 0, length: editor.utf16Length))
        let em = try #require(spans.first { $0.kind == .emphasis })
        #expect(NSRange(location: Int(em.start), length: Int(em.length))
            == (editor.initialText as NSString).range(of: "*dark*"))
    }
}
