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
    /// True while a pull or sync is running.
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

    /// Sync with the remote (#1233): commit this phone's saved edits with
    /// `commitMessage`, merge the remote's, and push, or park conflicting work
    /// on a `phone/<date>` draft. Returns `nil` when a sync is already running.
    /// The caller saves the open chapter and supplies the token first.
    public func sync(commitMessage: String) async throws -> SyncReport? {
        guard let project, !pulling else { return nil }
        pulling = true
        defer { pulling = false }
        let report = try await Task.detached { try project.sync(commitMessage: commitMessage) }.value
        refresh()
        return report
    }

    /// Open `chapter` for editing. Each save refreshes the chapter list, so
    /// its word counts stay current.
    public func editor(for chapter: ChapterInfo) throws -> ChapterEditor {
        guard let project else {
            throw QuillError.Io(message: "No project is open.")
        }
        return try ChapterEditor(project: project, chapter: chapter) { [weak self] in
            self?.refresh()
        }
    }

    /// The story bible's pages (#1236), read-only.
    public func biblePages() -> [BiblePage] {
        (try? project?.biblePages()) ?? []
    }

    /// A bible page's text, without front matter.
    public func biblePage(_ page: BiblePage) throws -> String {
        guard let project else { return "" }
        return try project.biblePage(path: page.path)
    }

    /// The open findings the desktop's analysis saved, in manuscript order.
    public func findings() -> [FindingInfo] {
        (try? project?.findings()) ?? []
    }

    /// Where in its chapter a finding points (UTF-16), if it quotes a phrase
    /// the chapter contains.
    public func passage(for finding: FindingInfo) -> NSRange? {
        guard let project, let index = finding.chapterIndex,
            let span = try? project.findingPassage(index: index, summary: finding.summary)
        else { return nil }
        return NSRange(location: Int(span.start), length: Int(span.length))
    }

    /// A chapter's prose, with its front matter hidden.
    public func text(of chapter: ChapterInfo) throws -> String {
        guard let project else { return "" }
        return try project.chapterText(index: chapter.index)
    }
}
