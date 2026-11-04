import Foundation

/// iCloud's conflicting versions of a book's chapters (#1298). When the same
/// chapter changes on two devices before they sync, iCloud keeps one as the
/// file and the other as an unresolved `NSFileVersion`. Quill keeps both where
/// the author can see them: the other version is written beside the chapter as
/// `<name> 2.md` (the name the desktop and the core use, and look for), then
/// marked resolved. No words are lost, and the chapter list names the chapter.
public enum CloudConflicts {
    /// Keep every unresolved conflicting version of `chapters` (file paths
    /// relative to `folder`) as a copy beside its chapter. Returns the copies'
    /// file names.
    @discardableResult
    public static func keepCopies(in folder: URL, chapters: [String]) -> [String] {
        var kept: [String] = []
        for file in chapters {
            let url = folder.appending(path: file)
            guard let versions = NSFileVersion.unresolvedConflictVersionsOfItem(at: url),
                !versions.isEmpty
            else { continue }
            for version in versions {
                if let text = try? String(contentsOf: version.url, encoding: .utf8),
                    let copy = try? keepCopy(of: url, text: text)
                {
                    kept.append(copy.lastPathComponent)
                }
                version.isResolved = true
            }
            try? NSFileVersion.removeOtherVersionsOfItem(at: url)
        }
        return kept
    }

    /// Write `text` beside `chapter` as `<stem> 2.<ext>`, or the next free
    /// number, and return the copy.
    static func keepCopy(of chapter: URL, text: String) throws -> URL {
        let fm = FileManager.default
        let stem = chapter.deletingPathExtension().lastPathComponent
        let ext = chapter.pathExtension
        let dir = chapter.deletingLastPathComponent()
        let copy =
            (2..<1000).lazy
            .map { dir.appending(path: "\(stem) \($0).\(ext)") }
            .first { !fm.fileExists(atPath: $0.path) }
            ?? dir.appending(path: "\(stem) copy.\(ext)")
        try text.write(to: copy, atomically: true, encoding: .utf8)
        return copy
    }
}
