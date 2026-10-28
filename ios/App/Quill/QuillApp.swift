import QuillKit
import SwiftUI

/// The Quill iPhone app: Quill's Rust core under a touch-first SwiftUI shell.
@main
struct QuillApp: App {
    @State private var app = AppState()

    var body: some Scene {
        WindowGroup {
            RootView()
                .environment(app)
                .environment(app.store)
                .environment(app.library)
                .task { app.launch() }
        }
    }
}

/// The library, or the open project.
struct RootView: View {
    @Environment(AppState.self) private var app
    @Environment(ProjectStore.self) private var store

    var body: some View {
        if store.url == nil {
            LibraryView()
        } else {
            ProjectView()
        }
    }
}
