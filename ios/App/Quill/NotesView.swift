import QuillKit
import SwiftUI

/// The story bible and the analysis findings (#1236), read-only: both come
/// from the desktop through sync, so reading them costs nothing and changing
/// them stays a desktop job.
struct NotesView: View {
    @Environment(AppState.self) private var app
    @Environment(ProjectStore.self) private var store
    @Environment(\.dismiss) private var dismiss
    @State private var tab = Tab.bible

    enum Tab: String, CaseIterable { case bible = "Story Bible", findings = "Findings" }

    var body: some View {
        NavigationStack {
            Group {
                switch tab {
                case .bible: BibleList()
                case .findings: FindingsList { finding in
                    dismiss()
                    app.open(finding)
                }
                }
            }
            .toolbar {
                ToolbarItem(placement: .principal) {
                    Picker("Notes", selection: $tab) {
                        ForEach(Tab.allCases, id: \.self) { Text($0.rawValue).tag($0) }
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
}

private struct BibleList: View {
    @Environment(ProjectStore.self) private var store

    var body: some View {
        let pages = store.biblePages()
        let sections = Dictionary(grouping: pages, by: \.section)
        List {
            ForEach(["Story", "Characters"], id: \.self) { section in
                if let pages = sections[section] {
                    Section(section) {
                        ForEach(pages, id: \.path) { page in
                            NavigationLink(page.title) { BiblePageView(page: page) }
                        }
                    }
                }
            }
        }
        .overlay {
            if pages.isEmpty {
                ContentUnavailableView(
                    "No story bible yet", systemImage: "books.vertical",
                    description: Text("The desktop's bible passes write it into bible/, and sync brings it here."))
            }
        }
    }
}

private struct BiblePageView: View {
    @Environment(ProjectStore.self) private var store
    let page: BiblePage
    @State private var text: AttributedString = ""

    var body: some View {
        ScrollView {
            Text(text)
                .frame(maxWidth: 640, alignment: .leading)
                .frame(maxWidth: .infinity)
                .padding()
                .textSelection(.enabled)
        }
        .navigationTitle(page.title)
        .task {
            let raw = (try? store.biblePage(page)) ?? ""
            text =
                (try? AttributedString(
                    markdown: raw,
                    options: .init(interpretedSyntax: .inlineOnlyPreservingWhitespace)))
                ?? AttributedString(raw)
        }
    }
}

private struct FindingsList: View {
    @Environment(ProjectStore.self) private var store
    let open: (FindingInfo) -> Void

    var body: some View {
        let findings = store.findings()
        let groups = Dictionary(grouping: findings) { $0.chapters.first ?? "Not tied to a chapter" }
        let order = findings.map { $0.chapters.first ?? "Not tied to a chapter" }
            .reduce(into: [String]()) { if !$0.contains($1) { $0.append($1) } }
        List {
            ForEach(order, id: \.self) { chapter in
                Section(chapter) {
                    ForEach(groups[chapter] ?? [], id: \.summary) { finding in
                        Button { open(finding) } label: {
                            VStack(alignment: .leading, spacing: 2) {
                                Text(finding.summary)
                                Text([finding.kind, finding.agreement].compactMap { $0 }.joined(separator: " · "))
                                    .font(.caption)
                                    .foregroundStyle(.secondary)
                            }
                        }
                        .tint(.primary)
                        .disabled(finding.chapterIndex == nil)
                    }
                }
            }
        }
        .overlay {
            if findings.isEmpty {
                ContentUnavailableView(
                    "No open findings", systemImage: "checkmark.seal",
                    description: Text("Findings from the desktop's analysis passes show up here after a sync."))
            }
        }
    }
}
