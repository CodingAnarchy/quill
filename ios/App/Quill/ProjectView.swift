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
            if let prose = app.prose {
                ChapterEditorView(host: prose)
            } else if let error = app.chapterError {
                ContentUnavailableView(
                    "Couldn't open the chapter", systemImage: "exclamationmark.triangle",
                    description: Text(error))
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
                VStack(alignment: .leading, spacing: 2) {
                    if let info = store.info {
                        Text(Format.words(info.words))
                    }
                    if let status = app.syncStatus {
                        Text(status)
                    }
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
                        Button("Sync", systemImage: "arrow.triangle.2.circlepath") {
                            Task { await app.sync(.manual) }
                        }
                    }
                }
            }
        }
        .refreshable {
            await app.sync(.manual)
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

/// The open chapter: its prose, editable, with a live word count.
struct ChapterEditorView: View {
    let host: ProseHost

    var body: some View {
        ProseTextView(host: host)
            .ignoresSafeArea(.container, edges: .bottom)
            .navigationTitle(host.editor.chapter.title)
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .status) {
                    WordCountLabel(editor: host.editor)
                }
            }
            .toolbarTitleMenu {
                if let error = host.editor.saveError {
                    Label(error, systemImage: "exclamationmark.triangle")
                }
            }
    }
}

/// "1,204 words", and a warning when the last save failed.
struct WordCountLabel: View {
    let editor: ChapterEditor

    var body: some View {
        HStack(spacing: 4) {
            if editor.saveError != nil {
                Image(systemName: "exclamationmark.triangle.fill")
                    .foregroundStyle(.red)
                    .accessibilityLabel("Not saved")
            }
            Text(Format.words(editor.wordCount))
                .monospacedDigit()
        }
        .font(.footnote)
        .foregroundStyle(.secondary)
    }
}
