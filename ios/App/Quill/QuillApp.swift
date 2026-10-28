import QuillKit
import SwiftUI

/// The Quill iPhone app (#1230): opens a novel through Quill's Rust core.
@main
struct QuillApp: App {
    @State private var store = ProjectStore()

    var body: some Scene {
        WindowGroup {
            ProjectView()
                .environment(store)
                .task { openSample() }
        }
    }

    /// Point the core at the app's container and open the bundled sample.
    /// Later milestones replace the sample with the author's cloned projects.
    @MainActor
    private func openSample() {
        guard store.info == nil else { return }
        let support = URL.applicationSupportDirectory.appending(path: "Quill", directoryHint: .isDirectory)
        Core.configure(supportDirectory: support)
        guard let bundled = Bundle.main.url(forResource: "SampleNovel", withExtension: nil) else {
            return
        }
        do {
            let sample = try SampleNovel.install(
                from: bundled, into: URL.documentsDirectory.appending(path: "Projects"))
            store.open(sample)
        } catch {
            store.open(bundled)  // surfaces the reason in the view
        }
    }
}
