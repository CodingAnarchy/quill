import Foundation
import Observation

/// The open project, as the views see it: its details and chapter list,
/// refreshed from the core after anything that can change them.
@MainActor
@Observable
public final class ProjectStore {
    public private(set) var info: ProjectInfo?
    public private(set) var chapters: [ChapterInfo] = []
    /// The last failure to open or read, for the view to show.
    public private(set) var error: String?
    public private(set) var url: URL?
    /// The `origin` the project pulls from, if it has one.
    public private(set) var remote: String?
    /// True while a pull is running.
    public private(set) var pulling = false

    private var project: QuillProject?

    public init() {}

    /// Open the project in `folder`, replacing any open one.
    public func open(_ folder: URL) {
        project = nil  // release the old project's lock first
        do {
            let opened = try QuillProject.open(path: folder.path)
            project = opened
            url = folder
            error = nil
            refresh()
        } catch {
            self.error = describe(error)
            info = nil
            chapters = []
            url = nil
        }
    }

    /// Re-read the project's details and chapter list from the core.
    public func refresh() {
        guard let project else { return }
        do {
            info = try project.info()
            chapters = try project.chapters()
            remote = try project.remoteUrl()
        } catch {
            self.error = describe(error)
        }
    }

    /// Close the project.
    public func close() {
        project = nil
        info = nil
        chapters = []
        url = nil
        remote = nil
        error = nil
    }

    /// Switch a series to book `index`.
    public func selectBook(_ index: UInt32) {
        do {
            try project?.setActiveBook(index: index)
        } catch {
            self.error = describe(error)
        }
        refresh()
    }

    /// Pull the remote's changes and refresh the chapter list. Returns the
    /// core's message for the author ("Pulled the latest changes…"). The
    /// caller supplies the token first (``Library/useToken(for:)``).
    public func pull() async throws -> String {
        guard let project else { return "" }
        pulling = true
        defer { pulling = false }
        let message = try await Task.detached { try project.pull() }.value
        refresh()
        return message
    }

    /// A chapter's prose, with its front matter hidden.
    public func text(of chapter: ChapterInfo) throws -> String {
        guard let project else { return "" }
        return try project.chapterText(index: chapter.index)
    }
}
