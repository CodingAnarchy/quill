import Foundation
import Testing

@testable import QuillKit

@MainActor
@Suite struct WordLookupTests {
    @Test func aLongSelectionIsRefusedBeforeAnyRequest() async {
        let lookup = WordLookup(selection: "this is far too many words to be a slot", line: "")
        await lookup.run()
        #expect(lookup.candidates.isEmpty)
        #expect(lookup.error?.contains("short phrase") == true)
    }
}
