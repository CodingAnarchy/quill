import CoreGraphics

/// How the iPhone Duo is being held, from its hinge and its screen.
///
/// Half open, the inner screen has a crease across the middle of its long
/// side. Held upright like a book, the screen is wider than tall and the crease
/// runs top to bottom: Quill shows the chapter as a two-page spread. Turned
/// sideways like a laptop, the screen is taller than wide and the crease runs
/// across: Quill keeps the chapter on the upper half, above the keyboard on
/// the lower one. Open flat, or folded, it's an ordinary screen.
public enum Posture: Equatable, Sendable {
    case flat
    /// Half open and upright, with the crease at `crease` points across.
    case book(crease: CGFloat)
    /// Half open and sideways, with the crease at `crease` points down.
    case laptop(crease: CGFloat)

    /// The posture for a hinge that is (or isn't) partially open, given the
    /// screen's bounds in the coordinates the crease should come back in.
    /// The crease is at the middle of the screen's long side, whatever part
    /// of the screen those coordinates start from.
    public init(partiallyOpen: Bool, screen: CGRect) {
        guard partiallyOpen, !screen.isEmpty else {
            self = .flat
            return
        }
        self = screen.width > screen.height ? .book(crease: screen.midX) : .laptop(crease: screen.midY)
    }
}
