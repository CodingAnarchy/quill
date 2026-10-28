import QuillKit
import SwiftUI
import UIKit

/// The chapter's prose in a `UITextView` (#1232), bridged to SwiftUI.
///
/// The text view owns the on-screen text, so selection, copy and paste,
/// dictation, autocorrect, and undo are all standard iOS behavior. Its text
/// storage reports every change (typing, paste, dictation, undo) as the exact
/// UTF-16 range and replacement, which goes straight to the Rust core. The
/// core answers with styled spans for the edited paragraph, which are applied
/// as attributes, so emphasis and headings look the way the desktop draws
/// them.
///
/// The view itself lives in a ``ProseHost`` owned by the app state, not in
/// SwiftUI's view tree: when the split view rearranges (folding or unfolding
/// the phone), SwiftUI places the same text view again, with its text,
/// selection, undo history, and keyboard intact (#1234).
struct ProseTextView: UIViewRepresentable {
    let host: ProseHost

    func makeUIView(context: Context) -> ProseUITextView { host.textView }

    func updateUIView(_ view: ProseUITextView, context: Context) {}
}

/// One open chapter's text view and the bridge from its edits to the core.
@MainActor
final class ProseHost: NSObject, NSTextStorageDelegate {
    let editor: ChapterEditor
    let textView: ProseUITextView
    /// True while restyling, so attribute changes aren't taken for edits.
    private var styling = false

    init(editor: ChapterEditor) {
        self.editor = editor
        textView = ProseUITextView(usingTextLayoutManager: true)
        super.init()
        configure()
    }

    private func configure() {
        let view = textView
        view.backgroundColor = .systemBackground
        view.alwaysBounceVertical = true
        view.keyboardDismissMode = .interactive
        view.autocorrectionType = .default
        view.spellCheckingType = .yes
        view.smartQuotesType = .yes
        view.smartDashesType = .yes
        view.adjustsFontForContentSizeCategory = true
        view.typingAttributes = ProseStyle.base
        view.textStorage.setAttributedString(
            NSAttributedString(string: editor.initialText, attributes: ProseStyle.base))
        restyle(view.textStorage, NSRange(location: 0, length: view.textStorage.length))
        view.textStorage.delegate = self
    }

    nonisolated func textStorage(
        _ storage: NSTextStorage, didProcessEditing mask: NSTextStorage.EditActions,
        range edited: NSRange, changeInLength delta: Int
    ) {
        guard mask.contains(.editedCharacters) else { return }
        MainActor.assumeIsolated {
            guard !styling else { return }
            // `edited` is the changed range in the new text; the range it
            // replaced in the old text is shorter (or longer) by `delta`.
            let old = NSRange(location: edited.location, length: edited.length - delta)
            let replacement = (storage.string as NSString).substring(with: edited)
            editor.apply(range: old, replacement: replacement)
            // Restyle the paragraphs the edit touched. Changing attributes is
            // allowed here and doesn't come back as a character edit.
            restyle(storage, (storage.string as NSString).paragraphRange(for: edited))
        }
    }

    private func restyle(_ storage: NSTextStorage, _ range: NSRange) {
        guard range.length > 0 else { return }
        styling = true
        defer { styling = false }
        storage.setAttributes(ProseStyle.base, range: range)
        for span in editor.styles(in: range) {
            let r = NSIntersectionRange(
                NSRange(location: Int(span.start), length: Int(span.length)), range)
            guard r.length > 0, NSMaxRange(r) <= storage.length else { continue }
            storage.addAttributes(ProseStyle.attributes(for: span.kind), range: r)
            if span.kind == .strong || span.kind == .emphasis {
                ProseStyle.addTrait(span.kind == .strong ? .traitBold : .traitItalic, to: storage, in: r)
            }
        }
    }

    /// Restyle the whole chapter (after a style-affecting change elsewhere).
    func restyleAll() {
        restyle(textView.textStorage, NSRange(location: 0, length: textView.textStorage.length))
    }
}

/// A text view that keeps a comfortable line length on a wide screen (the
/// unfolded iPhone Duo, landscape) by widening its side insets rather than
/// letting lines run edge to edge.
final class ProseUITextView: UITextView {
    /// The widest the text column gets, in points (about 70 characters).
    static let maxLineWidth: CGFloat = 620

    /// Whether the author was typing when the view left its window. Folding
    /// or unfolding the phone moves this view to a new place in the layout,
    /// which takes it out of the window for a moment and drops the keyboard;
    /// it comes straight back with the cursor where it was (#1234).
    private var wasEditing = false

    override func willMove(toWindow newWindow: UIWindow?) {
        super.willMove(toWindow: newWindow)
        if newWindow == nil {
            wasEditing = isFirstResponder
        }
    }

    override func didMoveToWindow() {
        super.didMoveToWindow()
        guard window != nil, wasEditing else { return }
        wasEditing = false
        let selection = selectedRange
        becomeFirstResponder()
        selectedRange = selection
        scrollRangeToVisible(selection)
    }

    override func layoutSubviews() {
        super.layoutSubviews()
        let side = max(16, (bounds.width - Self.maxLineWidth) / 2)
        let inset = UIEdgeInsets(top: 20, left: side, bottom: 40, right: side)
        if textContainerInset != inset {
            textContainerInset = inset
        }
    }

    /// Insert `text` at the cursor (replacing any selection) as if typed, so it
    /// goes through undo and the core like any other edit.
    func insertAtCursor(_ text: String) {
        if let range = selectedTextRange {
            replace(range, withText: text)
        }
    }

    /// Select the UTF-16 range `range` and scroll it into view.
    func reveal(_ range: NSRange) {
        let clamped = NSIntersectionRange(range, NSRange(location: 0, length: textStorage.length))
        selectedRange = clamped
        scrollRangeToVisible(clamped)
    }
}

/// How the prose looks: a serif body, with the desktop's emphasis, heading,
/// and marker styling.
enum ProseStyle {
    static var bodyFont: UIFont {
        let base = UIFont.preferredFont(forTextStyle: .body)
        let serif = base.fontDescriptor.withDesign(.serif) ?? base.fontDescriptor
        return UIFont(descriptor: serif, size: base.pointSize + 1)
    }

    static var base: [NSAttributedString.Key: Any] {
        let paragraph = NSMutableParagraphStyle()
        paragraph.lineSpacing = 4
        paragraph.paragraphSpacing = 6
        return [.font: bodyFont, .foregroundColor: UIColor.label, .paragraphStyle: paragraph]
    }

    static func attributes(for kind: StyleKind) -> [NSAttributedString.Key: Any] {
        switch kind {
        case .heading:
            let font = UIFont.preferredFont(forTextStyle: .title2)
            let bold = font.fontDescriptor.withSymbolicTraits(.traitBold) ?? font.fontDescriptor
            return [.font: UIFont(descriptor: bold, size: font.pointSize), .foregroundColor: UIColor.systemTeal]
        case .code:
            return [.font: UIFont.monospacedSystemFont(ofSize: bodyFont.pointSize - 2, weight: .regular)]
        case .sceneBreak:
            let centered = NSMutableParagraphStyle()
            centered.alignment = .center
            return [.foregroundColor: UIColor.secondaryLabel, .paragraphStyle: centered]
        case .marker:
            return [.foregroundColor: UIColor.tertiaryLabel]
        case .strong, .emphasis:
            return [:]  // a font trait, added by `addTrait` so bold italic combines
        }
    }

    /// Add a bold or italic trait to the fonts already in `range`, so nested
    /// emphasis combines (bold inside italic stays both).
    static func addTrait(_ trait: UIFontDescriptor.SymbolicTraits, to storage: NSTextStorage, in range: NSRange) {
        storage.enumerateAttribute(.font, in: range) { value, sub, _ in
            guard let font = value as? UIFont else { return }
            let traits = font.fontDescriptor.symbolicTraits.union(trait)
            if let descriptor = font.fontDescriptor.withSymbolicTraits(traits) {
                storage.addAttribute(.font, value: UIFont(descriptor: descriptor, size: font.pointSize), range: sub)
            }
        }
    }
}
