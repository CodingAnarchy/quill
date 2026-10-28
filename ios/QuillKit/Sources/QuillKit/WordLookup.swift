import Foundation
import Observation

/// Looking up a word for a slot the author selected (#1235): the same
/// reference-only lookup the desktop runs, through the core. It returns
/// candidates for the selection, never a rewritten sentence; the author
/// picks one and the text view places it in the selection alone.
@MainActor
@Observable
public final class WordLookup {
    public let selection: String
    public private(set) var candidates: [WordCandidate] = []
    public private(set) var error: String?
    public private(set) var searching = false

    private let line: String

    /// A lookup for `selection`, judged against `line`, the paragraph it
    /// sits in.
    public init(selection: String, line: String) {
        self.selection = selection
        self.line = line
    }

    public func run() async {
        searching = true
        defer { searching = false }
        let (selection, line) = (selection, line)
        do {
            candidates = try await Task.detached {
                try lookUpWords(selection: selection, line: line)
            }.value
            error = candidates.isEmpty ? "No candidates. Try a different word." : nil
        } catch {
            self.error = describe(error)
        }
    }
}

/// The Claude API key used for lookups, kept through the core's keychain
/// store (the same one the desktop reads).
public enum ClaudeKey {
    public static var isSet: Bool { hasClaudeApiKey() }

    public static func save(_ key: String) throws {
        try setClaudeApiKey(key: key)
    }
}
