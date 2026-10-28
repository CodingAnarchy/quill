import QuillKit
import SwiftUI

/// The candidates for a looked-up word (#1235). Tapping one puts it in place
/// of the selection and nowhere else; the author picks, Quill places.
struct LookupSheet: View {
    let lookup: WordLookup
    let choose: (String) -> Void
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        NavigationStack {
            List {
                if lookup.searching {
                    HStack {
                        ProgressView()
                        Text("Looking up “\(lookup.selection)”…")
                            .foregroundStyle(.secondary)
                    }
                }
                if let error = lookup.error {
                    Label(error, systemImage: "exclamationmark.triangle")
                        .foregroundStyle(.secondary)
                }
                ForEach(lookup.candidates, id: \.text) { candidate in
                    Button {
                        choose(candidate.text)
                        dismiss()
                    } label: {
                        CandidateRow(candidate: candidate)
                    }
                    .tint(.primary)
                }
            }
            .navigationTitle(lookup.selection)
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") { dismiss() }
                }
            }
            .task { await lookup.run() }
        }
        .presentationDetents([.medium, .large])
    }
}

private struct CandidateRow: View {
    let candidate: WordCandidate

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(candidate.text)
                .font(.system(.body, design: .serif).weight(.semibold))
            Text(candidate.definition)
                .font(.subheadline)
                .foregroundStyle(.secondary)
            let tags = [
                candidate.syllables.map { "\($0) syllable\($0 == 1 ? "" : "s")" },
                candidate.origin, candidate.rarity,
            ].compactMap { $0 }
            if !tags.isEmpty {
                Text(tags.joined(separator: " · "))
                    .font(.caption)
                    .foregroundStyle(.tertiary)
            }
        }
    }
}
