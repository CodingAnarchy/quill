import QuillKit
import SwiftUI

/// Every TODO marker in the open chapter or the whole book (#1237). Tapping
/// one opens its spot with the marker selected.
struct MarkersView: View {
    @Environment(AppState.self) private var app
    @Environment(ProjectStore.self) private var store
    @Environment(\.dismiss) private var dismiss
    /// The open chapter, whose markers are listed live (saved or not).
    let current: ChapterEditor
    @State private var scope = Scope.chapter

    enum Scope: String, CaseIterable { case chapter = "This Chapter", book = "Whole Book" }

    var body: some View {
        NavigationStack {
            let markers = scope == .chapter ? current.markers() : bookMarkers()
            List(markers, id: \.self) { marker in
                Button {
                    dismiss()
                    app.open(marker)
                } label: {
                    VStack(alignment: .leading, spacing: 2) {
                        Text(marker.line)
                        if scope == .book {
                            Text(marker.chapterTitle)
                                .font(.caption)
                                .foregroundStyle(.secondary)
                        }
                    }
                }
                .tint(.primary)
            }
            .overlay {
                if markers.isEmpty {
                    ContentUnavailableView(
                        "No markers", systemImage: "flag",
                        description: Text("Tap Marker above the keyboard to leave a “\(store.todoMarker)” note to come back to."))
                }
            }
            .toolbar {
                ToolbarItem(placement: .principal) {
                    Picker("Scope", selection: $scope) {
                        ForEach(Scope.allCases, id: \.self) { Text($0.rawValue).tag($0) }
                    }
                    .pickerStyle(.segmented)
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button("Done") { dismiss() }
                }
            }
            .navigationBarTitleDisplayMode(.inline)
        }
    }

    /// The whole book's markers. The open chapter is saved first, so the
    /// list includes a marker dropped a moment ago.
    private func bookMarkers() -> [MarkerInfo] {
        current.save()
        return store.markers()
    }
}
