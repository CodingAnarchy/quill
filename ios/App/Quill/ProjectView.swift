import QuillKit
import SwiftUI

/// The open project's chapter list, each with its word count and status.
struct ProjectView: View {
    @Environment(ProjectStore.self) private var store

    var body: some View {
        NavigationStack {
            Group {
                if let error = store.error {
                    ContentUnavailableView(
                        "Couldn't open the project",
                        systemImage: "exclamationmark.triangle",
                        description: Text(error))
                } else if let info = store.info {
                    ChapterList(info: info)
                } else {
                    ProgressView()
                }
            }
            .navigationDestination(for: ChapterInfo.self) { chapter in
                ChapterReader(chapter: chapter)
            }
        }
    }
}

private struct ChapterList: View {
    @Environment(ProjectStore.self) private var store
    let info: ProjectInfo

    var body: some View {
        List {
            Section {
                ForEach(store.chapters, id: \.index) { chapter in
                    NavigationLink(value: chapter) {
                        ChapterRow(chapter: chapter)
                    }
                }
            } footer: {
                Text(Format.words(info.words))
            }
        }
        .navigationTitle(info.title)
    }
}

struct ChapterRow: View {
    let chapter: ChapterInfo

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(chapter.title)
                .font(.body)
            HStack(spacing: 6) {
                Text(Format.words(chapter.words))
                if let status = chapter.status {
                    Text("·")
                    Text(status)
                }
            }
            .font(.caption)
            .foregroundStyle(.secondary)
        }
    }
}

/// A chapter's prose, read-only for now (#1232 adds editing).
private struct ChapterReader: View {
    @Environment(ProjectStore.self) private var store
    let chapter: ChapterInfo
    @State private var text = ""
    @State private var error: String?

    var body: some View {
        ScrollView {
            Text(error ?? text)
                .font(.system(.body, design: .serif))
                .frame(maxWidth: 640, alignment: .leading)
                .padding()
                .textSelection(.enabled)
        }
        .navigationTitle(chapter.title)
        .navigationBarTitleDisplayMode(.inline)
        .task {
            do {
                text = try store.text(of: chapter)
            } catch {
                self.error = describe(error)
            }
        }
    }
}
