import Foundation
import Testing

@testable import QuillKit

@MainActor
@Suite struct MarkerTests {
    @Test func aMarkerTypedOnThePhoneIsListedEverywhere() throws {
        let dir = try scratch("markers")
        try writeNovel(in: dir)
        let store = ProjectStore()
        store.open(dir)
        #expect(store.todoMarker == "xxx")
        #expect(store.markers().isEmpty)

        let editor = try store.editor(for: store.chapters[1])
        editor.apply(range: NSRange(location: 0, length: 0), replacement: "xxx check the wind ")
        #expect(editor.markers().count == 1)
        #expect(editor.styles(in: NSRange(location: 0, length: 3)).contains { $0.kind == .todo })
        editor.save()

        let all = store.markers()
        #expect(all.map(\.chapterIndex) == [1])
        #expect(all.first?.line.hasPrefix("xxx check the wind") == true)
    }
}
