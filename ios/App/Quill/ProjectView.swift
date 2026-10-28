import QuillKit
import SwiftUI

/// The open project: its chapter list beside the selected chapter. On a
/// compact screen the split view collapses to one column at a time.
struct ProjectView: View {
    @Environment(AppState.self) private var app

    var body: some View {
        @Bindable var app = app
        NavigationSplitView {
            ChapterSidebar(selection: $app.selectedChapter)
        } detail: {
            if let index = app.selectedChapter,
                let chapter = app.store.chapters.first(where: { $0.index == index })
            {
                ChapterReader(chapter: chapter)
                    .id(chapter)
            } else {
                ContentUnavailableView("Choose a chapter", systemImage: "book")
            }
        }
    }
}

/// The chapter list, with a book picker for a series and a Pull button.
struct ChapterSidebar: View {
    @Environment(AppState.self) private var app
    @Environment(ProjectStore.self) private var store
    @Binding var selection: UInt32?
    @State private var pullMessage: String?

    var body: some View {
        List(selection: $selection) {
            if let info = store.info, info.books.count > 1 {
                Section {
                    Picker("Book", selection: bookBinding(info)) {
                        ForEach(info.books, id: \.index) { book in
                            Text(book.title).tag(book.index)
                        }
                    }
                }
            }
            Section {
                ForEach(store.chapters, id: \.index) { chapter in
                    ChapterRow(chapter: chapter)
                        .tag(chapter.index)
                }
            } footer: {
                if let info = store.info {
                    Text(Format.words(info.words))
                }
            }
        }
        .navigationTitle(store.info?.title ?? "")
        .toolbar {
            ToolbarItem(placement: .topBarLeading) {
                Button("Projects", systemImage: "books.vertical") { app.closeProject() }
            }
            if store.remote != nil {
                ToolbarItem(placement: .topBarTrailing) {
                    if store.pulling {
                        ProgressView()
                    } else {
                        Button("Pull", systemImage: "arrow.down.circle") {
                            Task { pullMessage = await app.pull() }
                        }
                    }
                }
            }
        }
        .alert(
            pullMessage ?? "", isPresented: Binding(get: { pullMessage != nil }, set: { if !$0 { pullMessage = nil } })
        ) {
            Button("OK", role: .cancel) {}
        }
        .refreshable {
            if store.remote != nil {
                pullMessage = await app.pull()
            }
        }
    }

    private func bookBinding(_ info: ProjectInfo) -> Binding<UInt32> {
        Binding(
            get: { info.activeBook },
            set: { book in
                selection = nil
                store.selectBook(book)
            })
    }
}

struct ChapterRow: View {
    let chapter: ChapterInfo

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(chapter.title)
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

/// A chapter's prose, read-only (#1231); editing arrives with #1232.
struct ChapterReader: View {
    @Environment(ProjectStore.self) private var store
    let chapter: ChapterInfo
    @State private var text = ""
    @State private var error: String?

    var body: some View {
        ScrollView {
            Text(error ?? text)
                .font(.system(.body, design: .serif))
                .frame(maxWidth: 640, alignment: .leading)
                .frame(maxWidth: .infinity)
                .padding()
                .textSelection(.enabled)
        }
        .navigationTitle(chapter.title)
        .navigationBarTitleDisplayMode(.inline)
        .task(id: chapter) {
            do {
                text = try store.text(of: chapter)
            } catch {
                self.error = describe(error)
            }
        }
    }
}
