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
        info = project.info()
        chapters = project.chapters()
    }

    /// Switch a series to book `index`.
    public func selectBook(_ index: UInt32) {
        project?.setActiveBook(index: index)
        refresh()
    }

    /// A chapter's prose, with its front matter hidden.
    public func text(of chapter: ChapterInfo) throws -> String {
        guard let project else { return "" }
        return try project.chapterText(index: chapter.index)
    }
}
