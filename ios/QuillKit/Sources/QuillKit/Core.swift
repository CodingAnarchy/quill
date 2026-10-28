import Foundation
@_exported import QuillCore

/// App-wide setup of the Rust core.
public enum Core {
    /// Point the core's config and data directories into the app's container.
    /// Call once at launch, before opening a project.
    public static func configure(supportDirectory: URL) {
        try? FileManager.default.createDirectory(
            at: supportDirectory, withIntermediateDirectories: true)
        QuillCore.configure(supportDir: supportDirectory.path)
    }

    /// The Rust core's version, for the settings screen and bug reports.
    public static var version: String { coreVersion() }
}

extension QuillError {
    /// The message written for the author (the generated `errorDescription`
    /// is a debug dump of the enum case).
    public var message: String {
        switch self {
        case let .NotFound(message), let .Invalid(message), let .Io(message), let .Remote(message),
            let .Lookup(message):
            return message
        }
    }
}

/// A short, author-facing description of any error the app surfaces.
public func describe(_ error: Error) -> String {
    (error as? QuillError)?.message ?? error.localizedDescription
}
