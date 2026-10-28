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
        // the app, so no typed word is lost (#1232). Sync when the app comes
        // back and when it goes to the background (#1233).
        .onChange(of: scenePhase) { old, phase in
            switch phase {
            case .inactive: app.saveForBackground()
            case .background: app.saveAndSyncForBackground()
            case .active where old == .background: Task { await app.sync(.foreground) }
            default: break
            }
        }
        .alert(
            app.notice ?? "",
            isPresented: Binding(get: { app.notice != nil }, set: { if !$0 { app.notice = nil } })
        ) {
            Button("OK", role: .cancel) {}
        }
    }
}
