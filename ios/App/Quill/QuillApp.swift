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
    @Environment(\.scenePhase) private var scenePhase

    var body: some View {
        Group {
            if store.url == nil {
                LibraryView()
            } else {
                ProjectView()
            }
        }
        // Switching apps, locking the phone, or swiping to the app switcher all
        // pass through `.inactive`: save there, before iOS can suspend or kill
        // the app, so no typed word is lost (#1232).
        .onChange(of: scenePhase) { _, phase in
            if phase != .active { app.saveForBackground() }
        }
    }
}
