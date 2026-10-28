import Foundation

/// The sample novel the app ships with (#1230), so a fresh install opens a
/// real project before any git remote is set up.
public enum SampleNovel {
    /// Copy the bundled sample at `bundled` into `projects` (once) and return
    /// the copy. The bundle is read-only and opening a project writes to it
    /// (its lock file, caches), so the app always opens the copy.
    public static func install(from bundled: URL, into projects: URL) throws -> URL {
        let fm = FileManager.default
        let target = projects.appendingPathComponent(bundled.lastPathComponent, isDirectory: true)
        if !fm.fileExists(atPath: target.appendingPathComponent("book.toml").path) {
            try fm.createDirectory(at: projects, withIntermediateDirectories: true)
            try? fm.removeItem(at: target)
            try fm.copyItem(at: bundled, to: target)
        }
        return target
    }
}
