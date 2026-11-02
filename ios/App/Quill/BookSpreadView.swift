import QuillKit
import SwiftUI
import UIKit

/// The open chapter as a two-page spread, for reading with the iPhone Duo half
/// open and upright like a book (#1284).
///
/// One page sits on each side of the crease, so no line crosses the fold, and
/// a swipe turns the spread. The pages are for reading: selecting a passage
/// offers "Add TODO", which drops the project's marker into the chapter for
/// the next writing session, and nothing else changes the prose here.
/// Leaving the posture puts the editor back at the spread that was showing.
struct BookSpreadView: UIViewRepresentable {
    let host: ProseHost
    /// The book's title, for the running head over the right-hand pages.
    let bookTitle: String

    func makeUIView(context: Context) -> BookSpread {
        BookSpread(host: host, bookTitle: bookTitle)
    }

    func updateUIView(_ view: BookSpread, context: Context) {}

    static func dismantleUIView(_ view: BookSpread, coordinator: ()) {
        view.returnToEditor()
    }
}

/// The spread's pages: a copy of the chapter's styled text, measured into
/// page-sized pieces, each piece laid out on its own page.
final class BookSpread: UIView, UIScrollViewDelegate, UITextViewDelegate {
    private let host: ProseHost
    private let bookTitle: String
    private let scroll = UIScrollView()
    private var pages: [Page] = []
    private var laidOutSize = CGSize.zero
    /// Which spread is showing: 0 is pages 1 and 2.
    private var spread = 0

    /// Each page's half of the screen keeps this far from the crease, and
    /// from the screen's edge.
    private static let gutter: CGFloat = 28
    private static let margin: CGFloat = 32
    /// Room above the pages for the running head, and below for the number.
    private static let head: CGFloat = 40
    private static let foot: CGFloat = 36

    private struct Page {
        /// The page's own copy of its text, which owns the page's layout.
        let storage: NSTextStorage
        let text: UITextView
        let views: [UIView]
        /// The chapter's text on this page, as a UTF-16 range.
        let characters: NSRange
    }

    init(host: ProseHost, bookTitle: String) {
        self.host = host
        self.bookTitle = bookTitle
        super.init(frame: .zero)
        backgroundColor = .systemBackground
        scroll.isPagingEnabled = true
        scroll.showsHorizontalScrollIndicator = false
        scroll.showsVerticalScrollIndicator = false
        scroll.alwaysBounceVertical = false
        scroll.contentInsetAdjustmentBehavior = .never
        scroll.delegate = self
        scroll.accessibilityIdentifier = "BookSpread"
        addSubview(scroll)
        // The keyboard has nothing to type into here.
        host.textView.resignFirstResponder()
    }

    required init?(coder: NSCoder) { fatalError("not used from a storyboard") }

    override func layoutSubviews() {
        super.layoutSubviews()
        scroll.frame = bounds
        guard bounds.size != laidOutSize, window != nil else { return }
        laidOutSize = bounds.size
        paginate(showing: pages.isEmpty ? host.textView.selectedRange.location : firstShowing)
    }

    /// The first character on the left-hand page that's showing.
    private var firstShowing: Int {
        pages.indices.contains(spread * 2) ? pages[spread * 2].characters.location : 0
    }

    // MARK: - Layout

    /// Where the pages go: the same size on both sides, each hugging the
    /// crease, which is the middle of the screen (not of this view).
    private func pageFrames() -> (left: CGRect, right: CGRect) {
        let crease =
            window?.windowScene.map { convert($0.screen.bounds, from: $0.screen.coordinateSpace).midX } ?? bounds.midX
        let insets = safeAreaInsets
        let room = min(
            crease - Self.gutter - insets.left - Self.margin,
            bounds.width - insets.right - Self.margin - crease - Self.gutter)
        let width = min(max(room, 120), ProseUITextView.maxLineWidth)
        let top = insets.top + Self.head
        let height = max(bounds.height - top - insets.bottom - Self.foot, 120)
        return (
            CGRect(x: crease - Self.gutter - width, y: top, width: width, height: height),
            CGRect(x: crease + Self.gutter, y: top, width: width, height: height)
        )
    }

    /// Lay the chapter out as pages, turned to the spread holding `location`.
    private func paginate(showing location: Int) {
        pages.flatMap(\.views).forEach { $0.removeFromSuperview() }
        pages = []
        let chapter = NSAttributedString(attributedString: host.textView.textStorage)
        let (left, right) = pageFrames()
        for (index, characters) in Self.pageBreaks(chapter, size: left.size).enumerated() {
            var frame = index.isMultiple(of: 2) ? left : right
            frame.origin.x += CGFloat(index / 2) * bounds.width
            pages.append(
                makePage(
                    index, of: chapter.attributedSubstring(from: characters), frame: frame, characters: characters))
        }
        for (index, page) in pages.enumerated() {
            page.views.last?.accessibilityLabel = "Page \(index + 1) of \(pages.count)"
        }
        let spreads = (pages.count + 1) / 2
        scroll.contentSize = CGSize(width: CGFloat(spreads) * bounds.width, height: bounds.height)
        let page = pages.firstIndex { NSLocationInRange(location, $0.characters) } ?? pages.count - 1
        spread = max(page, 0) / 2
        scroll.contentOffset = CGPoint(x: CGFloat(spread) * bounds.width, y: 0)
    }

    /// Where the chapter's pages break: the characters each `size` page
    /// holds, flowing the text through one container per page. Always at
    /// least one page, even for an empty chapter.
    static func pageBreaks(_ chapter: NSAttributedString, size: CGSize) -> [NSRange] {
        // The storage owns the layout (not the other way round), so it's
        // held here for as long as the layout is used.
        let storage = NSTextStorage(attributedString: chapter)
        let layout = NSLayoutManager()
        storage.addLayoutManager(layout)
        var pages: [NSRange] = []
        repeat {
            layout.addTextContainer(container(size))
            let glyphs = layout.glyphRange(for: layout.textContainers.last!)
            pages.append(layout.characterRange(forGlyphRange: glyphs, actualGlyphRange: nil))
            // A page too small for even one line would loop forever.
            if glyphs.length == 0 { break }
        } while NSMaxRange(layout.glyphRange(for: layout.textContainers.last!)) < layout.numberOfGlyphs
        return pages
    }

    private static func container(_ size: CGSize) -> NSTextContainer {
        let container = NSTextContainer(size: size)
        container.lineFragmentPadding = 0
        return container
    }

    /// One page: its own text, laid out the way `pageBreaks` measured it.
    /// Each page has its own layout because UIKit only selects text in the
    /// first of several text views sharing one.
    private func makePage(_ index: Int, of content: NSAttributedString, frame: CGRect, characters: NSRange) -> Page {
        let storage = NSTextStorage(attributedString: content)
        let layout = NSLayoutManager()
        storage.addLayoutManager(layout)
        let container = Self.container(frame.size)
        layout.addTextContainer(container)
        let text = UITextView(frame: frame, textContainer: container)
        text.isEditable = false
        text.isSelectable = true
        text.isScrollEnabled = false
        text.showsVerticalScrollIndicator = false
        text.showsHorizontalScrollIndicator = false
        text.textContainerInset = .zero
        text.backgroundColor = .clear
        text.delegate = self
        text.accessibilityIdentifier = "BookPage"

        // The chapter's title over the left-hand page, the book's over the
        // right, and the page number under each: a book's small furniture,
        // however large the type is.
        let head = UILabel()
        head.text = index.isMultiple(of: 2) ? host.editor.chapter.title : bookTitle
        head.font = .systemFont(ofSize: 12)
        head.textColor = .secondaryLabel
        head.textAlignment = .center
        head.frame = CGRect(x: frame.minX, y: frame.minY - Self.head + 8, width: frame.width, height: Self.head - 16)
        let number = UILabel()
        number.text = "\(index + 1)"
        number.font = .monospacedDigitSystemFont(ofSize: 12, weight: .regular)
        number.textColor = .secondaryLabel
        number.textAlignment = .center
        number.accessibilityIdentifier = "PageNumber"
        number.frame = CGRect(x: frame.minX, y: frame.maxY + 8, width: frame.width, height: Self.foot - 8)

        let views: [UIView] = [text, head, number]
        views.forEach(scroll.addSubview)
        return Page(storage: storage, text: text, views: views, characters: characters)
    }

    func scrollViewDidEndDecelerating(_ scrollView: UIScrollView) {
        guard bounds.width > 0 else { return }
        spread = Int((scrollView.contentOffset.x / bounds.width).rounded())
    }

    // MARK: - Marking

    /// "Add TODO" on any selection. The pages aren't editable, so the other
    /// actions are the reading ones: copy, look up, share.
    func textView(
        _ textView: UITextView, editMenuForTextIn range: NSRange, suggestedActions: [UIMenuElement]
    ) -> UIMenu? {
        // The range is in the page's own text; the page says where it starts.
        guard let page = pages.first(where: { $0.text === textView }) else { return nil }
        let location = page.characters.location + range.location
        let add = UIAction(title: "Add TODO", image: UIImage(systemName: "flag")) { [weak self] _ in
            self?.addMarker(at: location)
        }
        return UIMenu(children: [add] + suggestedActions)
    }

    /// Mark the chapter at `location`, then lay the pages out again around
    /// the marker, which is now part of the text.
    private func addMarker(at location: Int) {
        host.insertMarker(at: location)
        paginate(showing: location)
    }

    /// Hand the reading position back to the editor as the spread goes away.
    func returnToEditor() {
        // A spread that never laid out its pages has no reading position.
        guard !pages.isEmpty else { return }
        host.textView.revealOnShow(at: firstShowing)
    }
}
