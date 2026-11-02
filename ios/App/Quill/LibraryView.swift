import QuillKit
import SwiftUI

/// The projects on this phone, and cloning another from git (#1231).
struct LibraryView: View {
    @Environment(AppState.self) private var app
    @Environment(Library.self) private var library
    @State private var showingClone = false
    @State private var showingNewBook = false
    @State private var showingSettings = false

    var body: some View {
        NavigationStack {
            List {
                // Three ways in (#1297): start a book, open the sample, or
                // (for a power user) clone one from git.
                Section {
                    Button("New Book…", systemImage: "square.and.pencil") {
                        showingNewBook = true
                    }
                } footer: {
                    Text("A novel or a short story, kept on this iPhone with its own version history.")
                }
                if !library.projects.isEmpty {
                    Section {
                        ForEach(library.projects) { entry in
                            Button(entry.name) { app.open(entry.url) }
                        }
                    } header: {
                        Text("On this iPhone")
                    } footer: {
                        if library.projects.contains(where: { $0.name == SampleNovel.folderName }) {
                            Text("\(SampleNovel.folderName) is a short sample book to try Quill with.")
                        }
                    }
                }
                Section {
                    Button("Clone from Git…", systemImage: "arrow.down.doc") {
                        showingClone = true
                    }
                } footer: {
                    Text("Bring a novel the desktop pushes to GitHub (or any HTTPS git host) onto this phone.")
                }
            }
            .navigationTitle("Quill")
            .toolbar {
                ToolbarItem(placement: .topBarTrailing) {
                    Button("Settings", systemImage: "gearshape") { showingSettings = true }
                }
            }
            .sheet(isPresented: $showingClone) {
                CloneView()
            }
            .sheet(isPresented: $showingNewBook) {
                NewBookView()
            }
            .sheet(isPresented: $showingSettings) {
                SettingsView()
            }
            .overlay {
                if let error = app.store.error {
                    ContentUnavailableView(
                        "Couldn't open the project", systemImage: "exclamationmark.triangle",
                        description: Text(error))
                }
            }
        }
    }
}

/// Start a new book (#1297): its title, and whether it's a novel or a short
/// story. It opens ready to type.
struct NewBookView: View {
    @Environment(AppState.self) private var app
    @Environment(\.dismiss) private var dismiss
    @State private var title = ""
    @State private var form = ProjectForm.novel
    @State private var creating = false
    @State private var error: String?

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    TextField("Title", text: $title)
                        .textInputAutocapitalization(.words)
                        .submitLabel(.done)
                        .onSubmit { Task { await create() } }
                        .accessibilityIdentifier("NewBookTitle")
                }
                Section {
                    Picker("Kind", selection: $form) {
                        Text("Novel").tag(ProjectForm.novel)
                        Text("Short story").tag(ProjectForm.shortStory)
                    }
                    .pickerStyle(.segmented)
                } footer: {
                    Text(
                        form == .novel
                            ? "A novel starts with one chapter; add more as you go."
                            : "A short story is one piece of prose, with scenes."
                    )
                }
                if let error {
                    Section {
                        Label(error, systemImage: "exclamationmark.triangle")
                            .foregroundStyle(.red)
                    }
                }
            }
            .navigationTitle("New Book")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") { dismiss() }
                        .disabled(creating)
                }
                ToolbarItem(placement: .confirmationAction) {
                    if creating {
                        ProgressView()
                    } else {
                        Button("Create") { Task { await create() } }
                            .disabled(title.trimmingCharacters(in: .whitespaces).isEmpty)
                    }
                }
            }
            .interactiveDismissDisabled(creating)
        }
    }

    private func create() async {
        guard !creating, !title.trimmingCharacters(in: .whitespaces).isEmpty else { return }
        error = nil
        creating = true
        defer { creating = false }
        do {
            try await app.create(title: title, form: form)
            dismiss()
        } catch {
            self.error = describe(error)
        }
    }
}

/// The clone form: the repository's HTTPS address and a personal access token.
struct CloneView: View {
    @Environment(AppState.self) private var app
    @Environment(Library.self) private var library
    @Environment(\.dismiss) private var dismiss
    @State private var url = ""
    @State private var token = ""
    @State private var error: String?

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    TextField("https://github.com/you/novel.git", text: $url)
                        .textContentType(.URL)
                        .keyboardType(.URL)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                } header: {
                    Text("Repository")
                }
                Section {
                    SecureField("Personal access token", text: $token)
                        .textContentType(.password)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                } header: {
                    Text("Access token")
                } footer: {
                    Text(
                        "A token that can read and write this repository (on GitHub: a fine-grained token with Contents access). It's kept in the iPhone's Keychain and used only for this host."
                    )
                }
                if let error {
                    Section {
                        Label(error, systemImage: "exclamationmark.triangle")
                            .foregroundStyle(.red)
                    }
                }
            }
            .navigationTitle("Clone a Novel")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") { dismiss() }
                        .disabled(library.cloning)
                }
                ToolbarItem(placement: .confirmationAction) {
                    if library.cloning {
                        ProgressView()
                    } else {
                        Button("Clone") { Task { await clone() } }
                            .disabled(url.trimmingCharacters(in: .whitespaces).isEmpty)
                    }
                }
            }
            .interactiveDismissDisabled(library.cloning)
        }
    }

    private func clone() async {
        error = nil
        do {
            let folder = try await library.clone(url: url, token: token)
            dismiss()
            app.open(folder)
        } catch {
            self.error = describe(error)
        }
    }
}
