import QuillKit
import SwiftUI
import UIKit

/// A chapter's earlier versions (#1299): every snapshot, sync, and commit that
/// changed it, newest first. Opening one shows the chapter as it was, to read,
/// copy from, compare with the chapter now, or restore.
struct ChapterHistoryView: View {
    @Environment(AppState.self) private var app
    @Environment(\.dismiss) private var dismiss
    let host: ProseHost
    @State private var versions: [VersionInfo]?
    @State private var error: String?

    var body: some View {
        NavigationStack {
            Group {
                if let error {
                    ContentUnavailableView(
                        "Couldn't read the history", systemImage: "exclamationmark.triangle",
                        description: Text(error))
                } else if let versions, versions.isEmpty {
                    ContentUnavailableView(
                        "No earlier versions yet", systemImage: "clock",
                        description: Text("Quill saves a version each time you leave the app."))
                } else if let versions {
                    List(versions, id: \.rev) { version in
                        NavigationLink {
                            VersionView(host: host, version: version) { dismiss() }
                        } label: {
                            VersionRow(version: version)
                        }
                    }
                } else {
                    ProgressView()
                }
            }
            .navigationTitle("History")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button("Done") { dismiss() }
                }
            }
        }
        .task { await load() }
    }

    private func load() async {
        // The versions are the saved ones: save first, so the chapter as it
        // is now compares against what's on disk.
        host.editor.save()
        do {
            versions = try await app.store.versions(of: host.editor.chapter)
        } catch {
            self.error = describe(error)
        }
    }
}

/// When a version was saved, and where it came from.
struct VersionRow: View {
    let version: VersionInfo

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(version.date.formatted(date: .abbreviated, time: .shortened))
            Text(version.origin)
                .font(.footnote)
                .foregroundStyle(.secondary)
        }
    }
}

extension VersionInfo {
    var date: Date { Date(timeIntervalSince1970: TimeInterval(seconds)) }

    /// Where the version came from, in a few words: the commit message, less
    /// the date a snapshot's message ends with ("Snapshot, 2 Nov 2026 at
    /// 14:05" reads "Snapshot"), since the row already shows when.
    var origin: String {
        guard summary.hasPrefix("Snapshot"), let comma = summary.firstIndex(of: ",") else {
            return summary
        }
        return String(summary[..<comma])
    }
}

/// One earlier version of the chapter, read-only: the text as it was, or
/// compared with the chapter now, and a way to restore it.
struct VersionView: View {
    @Environment(AppState.self) private var app
    let host: ProseHost
    let version: VersionInfo
    /// Close the history once the chapter is restored.
    let onRestore: () -> Void
    @State private var text: String?
    @State private var comparing = false
    @State private var confirmingRestore = false
    @State private var error: String?

    var body: some View {
        Group {
            if let error {
                ContentUnavailableView(
                    "Couldn't read this version", systemImage: "exclamationmark.triangle",
                    description: Text(error))
            } else if let text {
                ReadOnlyText(
                    text: comparing
                        ? Self.comparison(of: text, with: host.textView.text)
                        : NSAttributedString(string: text, attributes: ProseStyle.base))
            } else {
                ProgressView()
            }
        }
        .navigationTitle(version.date.formatted(date: .abbreviated, time: .shortened))
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            ToolbarItem(placement: .principal) {
                Picker("Show", selection: $comparing) {
                    Text("Version").tag(false)
                    Text("Compare").tag(true)
                }
                .pickerStyle(.segmented)
                .accessibilityIdentifier("VersionMode")
            }
            ToolbarItem(placement: .bottomBar) {
                Button("Restore This Version") { confirmingRestore = true }
                    .disabled(text == nil)
            }
        }
        .confirmationDialog(
            "Restore this version?", isPresented: $confirmingRestore, titleVisibility: .visible
        ) {
            Button("Restore") {
                if let text {
                    app.restore(host, to: text)
                    onRestore()
                }
            }
        } message: {
            Text("The chapter goes back to this version. The chapter as it is now stays in the history, and Undo brings it back.")
        }
        .task {
            do {
                text = try await app.store.text(of: host.editor.chapter, at: version)
            } catch {
                self.error = describe(error)
            }
        }
    }

    /// The earlier version marked against the chapter now: words cut since
    /// struck through in red, words added since highlighted in green.
    static func comparison(of old: String, with new: String) -> NSAttributedString {
        let out = NSMutableAttributedString()
        for run in compareVersions(old: old, new: new) {
            var attributes = ProseStyle.base
            switch run.kind {
            case .same:
                break
            case .added:
                attributes[.backgroundColor] = UIColor.systemGreen.withAlphaComponent(0.25)
                attributes[.accessibilityTextCustom] = ["added"]
            case .removed:
                attributes[.foregroundColor] = UIColor.systemRed
                attributes[.strikethroughStyle] = NSUnderlineStyle.single.rawValue
                attributes[.accessibilityTextCustom] = ["removed"]
            }
            out.append(NSAttributedString(string: run.text, attributes: attributes))
        }
        return out
    }
}

/// Text to read and copy from, but not edit: selecting a passage and copying
/// it out of an old version works as anywhere in iOS.
struct ReadOnlyText: UIViewRepresentable {
    let text: NSAttributedString

    func makeUIView(context: Context) -> UITextView {
        let view = UITextView()
        view.isEditable = false
        view.isSelectable = true
        view.alwaysBounceVertical = true
        view.adjustsFontForContentSizeCategory = true
        view.textContainerInset = UIEdgeInsets(top: 16, left: 12, bottom: 16, right: 12)
        view.accessibilityIdentifier = "VersionText"
        return view
    }

    func updateUIView(_ view: UITextView, context: Context) {
        if view.attributedText != text {
            view.attributedText = text
        }
    }
}
