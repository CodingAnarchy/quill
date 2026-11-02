import QuillKit
import SwiftUI
import UIKit

/// A finished export and what the author should know about it (#1300).
struct ExportedFile: Identifiable {
    let url: URL
    let warnings: [String]
    var id: URL { url }
}

/// Export the book (#1300): a Word manuscript for an agent or editor, an EPUB,
/// or a PDF, handed to the share sheet to mail, AirDrop, save to Files, or open
/// in another app. While it runs, the button shows progress.
struct ExportMenu: View {
    @Environment(AppState.self) private var app
    @Environment(ProjectStore.self) private var store

    var body: some View {
        if store.exporting {
            ProgressView()
                .accessibilityLabel("Exporting")
        } else {
            Menu("Export", systemImage: "square.and.arrow.up") {
                Button("Manuscript (Word)", systemImage: "doc.text") { run(.manuscript) }
                Button("EPUB", systemImage: "book") { run(.epub) }
                Button("PDF", systemImage: "doc.richtext") { run(.pdf) }
                Button("Print PDF", systemImage: "printer") { run(.printPdf) }
            }
            .sheet(item: Bindable(app).exported) { file in
                ExportShareView(file: file)
            }
        }
    }

    private func run(_ kind: ExportKind) {
        Task { await app.export(kind) }
    }
}

/// The exported file in the share sheet, with any warnings about it above.
struct ExportShareView: View {
    let file: ExportedFile

    var body: some View {
        VStack(spacing: 0) {
            if !file.warnings.isEmpty {
                VStack(alignment: .leading, spacing: 4) {
                    ForEach(file.warnings, id: \.self) { warning in
                        Label(warning, systemImage: "exclamationmark.triangle")
                            .font(.footnote)
                    }
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding()
            }
            ShareSheet(items: [file.url])
        }
        .presentationDetents([.medium, .large])
    }
}

/// UIKit's share sheet, for a file.
struct ShareSheet: UIViewControllerRepresentable {
    let items: [Any]

    func makeUIViewController(context: Context) -> UIActivityViewController {
        UIActivityViewController(activityItems: items, applicationActivities: nil)
    }

    func updateUIViewController(_ controller: UIActivityViewController, context: Context) {}
}
