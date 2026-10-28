import QuillKit
import SwiftUI

/// The projects on this phone, and cloning another from git (#1231).
struct LibraryView: View {
    @Environment(AppState.self) private var app
    @Environment(Library.self) private var library
    @State private var showingClone = false

    var body: some View {
        NavigationStack {
            List {
                Section("On this iPhone") {
                    ForEach(library.projects) { entry in
                        Button(entry.name) { app.open(entry.url) }
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
            .sheet(isPresented: $showingClone) {
                CloneView()
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
