import Foundation
import Observation

/// One chapter open for editing (#1232). The text view owns the on-screen
/// text and reports each change here as a UTF-16 range and its replacement;
/// the Rust core applies it to its buffer, keeps the live word count, and
/// saves the chapter, front matter and annotations included, the way the
/// desktop does.
///
/// Saving is never left to chance: ``noteEdit()`` schedules a save shortly
/// after typing pauses, and the app calls ``save()`` whenever it leaves the
/// foreground.
@MainActor
@Observable
public final class ChapterEditor {
    public let chapter: ChapterInfo
    /// The prose as loaded; the text view takes it from here.
    public let initialText: String
    public private(set) var wordCount: UInt64
    public private(set) var dirty = false
    /// Bumped on every edit, so views over the live text (the scene list)
    /// refresh as the author types.
    public private(set) var revision = 0
    /// Whether this is a short story's prose (#1247), which names its length
    /// category beside the word count.
    public let isStory: Bool
    /// The last save failure, for the view to show.
    public private(set) var saveError: String?

    /// How long typing must pause before an automatic save.
    public var saveDelay: Duration = .seconds(1)

    private let session: ChapterSession
    private let onSave: @MainActor () -> Void
    private var pendingSave: Task<Void, Never>?

    /// Open `chapter` of `project` for editing. `onSave` runs after each save
    /// (the project store refreshes its chapter list there).
    public init(
        project: QuillProject, chapter: ChapterInfo, onSave: @escaping @MainActor () -> Void = {}
    ) throws {
        session = try project.editChapter(index: chapter.index)
        self.chapter = chapter
        isStory = (try? project.info())?.storyChapter == chapter.index
        self.onSave = onSave
        initialText = session.text()
        wordCount = session.wordCount()
    }

    /// The text view replaced the UTF-16 `range` of the old text with
    /// `replacement`.
    public func apply(range: NSRange, replacement: String) {
        session.applyEdit(
            start: UInt32(range.location), end: UInt32(range.location + range.length),
            replacement: replacement)
        wordCount = session.wordCount()
        dirty = true
        revision += 1
        noteEdit()
    }

    /// The styled spans overlapping `range` (UTF-16).
    public func styles(in range: NSRange) -> [StyleSpan] {
        session.styles(start: UInt32(range.location), end: UInt32(range.location + range.length))
    }

    /// The story's scenes as the prose stands now (#1247).
    public func scenes() -> [SceneInfo] {
        session.scenes()
    }

    /// The short-fiction length category of the live count ("short story"),
    /// for a short story; `nil` for a novel's chapter.
    public var lengthCategory: String? {
        isStory ? storyLength(words: wordCount) : nil
    }

    /// The TODO markers in this chapter as it stands, saved or not (#1237).
    public func markers() -> [MarkerInfo] {
        session.markers()
    }

    /// The core's length of the text, in UTF-16 units (for consistency checks).
    public var utf16Length: Int { Int(session.utf16Length()) }

    /// Save after typing pauses.
    public func noteEdit() {
        pendingSave?.cancel()
        let delay = saveDelay
        pendingSave = Task { [weak self] in
            try? await Task.sleep(for: delay)
            guard !Task.isCancelled else { return }
            self?.save()
        }
    }

    /// Save now if anything changed. Safe to call any time.
    public func save() {
        pendingSave?.cancel()
        pendingSave = nil
        guard session.isDirty() else {
            dirty = false
            return
        }
        do {
            try session.save()
            dirty = false
            saveError = nil
            onSave()
        } catch {
            saveError = describe(error)
        }
    }
}
