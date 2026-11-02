import Foundation

/// Watches an open chapter's file in iCloud Drive (#1298): iOS tells it when
/// another device's change arrives, and the chapter's saves go through its file
/// coordinator, so iCloud sees each write and syncs it.
final class ChapterFilePresenter: NSObject, NSFilePresenter, @unchecked Sendable {
    let presentedItemURL: URL?
    let presentedItemOperationQueue = OperationQueue()
    private let onChange: @MainActor () -> Void

    init(url: URL, onChange: @escaping @MainActor () -> Void) {
        presentedItemURL = url
        self.onChange = onChange
        super.init()
        presentedItemOperationQueue.maxConcurrentOperationCount = 1
        NSFileCoordinator.addFilePresenter(self)
    }

    deinit {
        NSFileCoordinator.removeFilePresenter(self)
    }

    /// Another device (or app) changed the file.
    func presentedItemDidChange() {
        let onChange = onChange
        Task { @MainActor in onChange() }
    }

    /// A conflicting version arrived from another device.
    func presentedItemDidGain(_ version: NSFileVersion) {
        presentedItemDidChange()
    }

    /// Run `write` (a save of this chapter) as a coordinated write, so iCloud
    /// knows the file changed. This presenter isn't told of its own write.
    func coordinateWrite(_ write: () -> Void) {
        guard let url = presentedItemURL else { return write() }
        var error: NSError?
        NSFileCoordinator(filePresenter: self).coordinate(
            writingItemAt: url, options: .forReplacing, error: &error
        ) { _ in write() }
        if error != nil { write() }
    }
}
