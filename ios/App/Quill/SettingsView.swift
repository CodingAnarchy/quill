import QuillKit
import SwiftUI

/// Settings: the Claude API key for word lookups (#1235), and the versions.
struct SettingsView: View {
    @Environment(\.dismiss) private var dismiss
    @State private var key = ""
    @State private var keySet = ClaudeKey.isSet
    @State private var error: String?

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    SecureField(keySet ? "Saved. Paste a new key to replace it" : "sk-ant-…", text: $key)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                    Button("Save Key") { save() }
                        .disabled(key.trimmingCharacters(in: .whitespaces).isEmpty)
                    if let error {
                        Label(error, systemImage: "exclamationmark.triangle")
                            .foregroundStyle(.red)
                    }
                } header: {
                    Text("Claude API key")
                } footer: {
                    Text(
                        "Used only when you select a word and choose Look Up Word. It's kept in the iPhone's Keychain. Quill never sends your manuscript, only the selected word and the paragraph around it."
                    )
                }
                Section("About") {
                    LabeledContent("Quill core", value: Core.version)
                }
            }
            .navigationTitle("Settings")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button("Done") { dismiss() }
                }
            }
        }
    }

    private func save() {
        do {
            try ClaudeKey.save(key)
            key = ""
            keySet = true
            error = nil
        } catch {
            self.error = describe(error)
        }
    }
}
