import CoreGraphics
import Testing

@testable import QuillKit

@Suite struct PostureTests {
    /// The unfolded Duo's inner screen, upright: wider than tall.
    let upright = CGRect(x: 0, y: 0, width: 951, height: 669)

    @Test func flatOrFoldedIsAnOrdinaryScreen() {
        #expect(Posture(partiallyOpen: false, screen: upright) == .flat)
        #expect(Posture(partiallyOpen: false, screen: CGRect(x: 0, y: 0, width: 669, height: 951)) == .flat)
    }

    @Test func halfOpenAndUprightIsABookWithTheCreaseDownTheMiddle() {
        #expect(Posture(partiallyOpen: true, screen: upright) == .book(crease: 475.5))
    }

    @Test func halfOpenAndSidewaysIsALaptopWithTheCreaseAcross() {
        let sideways = CGRect(x: 0, y: 0, width: 669, height: 951)
        #expect(Posture(partiallyOpen: true, screen: sideways) == .laptop(crease: 475.5))
    }

    /// The crease is the screen's middle, not the view's: a view that starts
    /// below the status bar sees it higher up.
    @Test func theCreaseIsInTheViewsCoordinates() {
        let screen = CGRect(x: 0, y: -50, width: 669, height: 951)
        #expect(Posture(partiallyOpen: true, screen: screen) == .laptop(crease: 425.5))
    }

    @Test func noScreenIsFlat() {
        #expect(Posture(partiallyOpen: true, screen: .zero) == .flat)
    }
}
