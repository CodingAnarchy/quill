import XCTest

/// The chapter list and the chapter on each iPhone size, upright and turned
/// sideways (#1289).
///
/// iOS gives the widest phones (the Pro Max, Plus, and Air) a regular width in
/// landscape, and the rest a compact one. So upright, every phone shows one
/// column at a time: the list, then the chapter, with a way back. Sideways,
/// only the widest keep the list beside the chapter. `ios/test-iphones.sh`
/// runs these on the newest base iPhone, e, Pro, Pro Max, and Air, and fails a
/// run where any of them skips. The iPhone Duo's layouts are checked by
/// `ios/test-duo.sh`.
final class PhoneLayoutUITests: XCTestCase {
    private var app: XCUIApplication!
    private var phone = Phone(name: "", wide: false)

    override func setUpWithError() throws {
        continueAfterFailure = false
        phone = try Phone.current()
        XCUIDevice.shared.orientation = .portrait
        app = XCUIApplication()
        app.launch()
    }

    override func tearDown() {
        XCUIDevice.shared.orientation = .portrait
        super.tearDown()
    }

    func testUprightShowsOneColumnAtATime() {
        checkLayout(.portrait)
    }

    func testSidewaysShowsTheLayoutForThePhonesWidth() {
        checkLayout(.landscapeLeft)
    }

    /// Typing sideways is where the keyboard takes the most room; the words
    /// are saved all the same.
    func testTypedWordsSurviveAKillSideways() {
        turn(.landscapeLeft)
        app.checkTypedWordsSurviveAKill()
    }

    // MARK: - Helpers

    /// Turn the phone to `orientation`, and check it turned.
    private func turn(_ orientation: UIDeviceOrientation) {
        XCUIDevice.shared.orientation = orientation
        let window = app.windows.firstMatch
        XCTAssertTrue(window.waitForExistence(timeout: 5))
        XCTAssertEqual(
            window.frame.width > window.frame.height, orientation.isLandscape,
            "the phone should be \(orientation.isLandscape ? "sideways" : "upright"), not \(window.frame.size)")
    }

    private func checkLayout(_ orientation: UIDeviceOrientation) {
        turn(orientation)
        app.openSample()
        let lampRoom = app.staticTexts["The Lamp Room"]
        let lowTide = app.staticTexts["Low Tide"]
        XCTAssertTrue(lampRoom.waitForHittable(timeout: 10), "the chapter list should show first")
        lampRoom.tap()
        XCTAssertTrue(app.textViews.firstMatch.waitForHittable(timeout: 10), "the chapter should open")
        attach(app, "\(phone) \(orientation.isLandscape ? "sideways" : "upright")")

        if phone.split(orientation) {
            XCTAssertTrue(app.shows(lowTide), "\(phone) sideways should keep the list beside the chapter")
            // Choosing another chapter keeps the split.
            lowTide.tap()
            XCTAssertTrue(app.textViews.firstMatch.waitForHittable(timeout: 10))
            XCTAssertTrue(app.shows(lampRoom), "the list should stay after choosing a chapter")
        } else {
            XCTAssertFalse(app.shows(lowTide), "\(phone) should show the chapter alone, not beside the list")
            // Back to the list.
            app.navigationBars.buttons.element(boundBy: 0).tap()
            XCTAssertTrue(lowTide.waitForHittable(timeout: 5), "back should return to the chapter list")
            XCTAssertFalse(app.shows(app.textViews.firstMatch), "the list should be alone again")
        }
    }
}

/// Which layout a phone should show, by its size.
private struct Phone: CustomStringConvertible {
    let name: String
    /// Wide enough for a regular width sideways.
    let wide: Bool

    /// The phone this test runs on, from the simulator's name. The Duo, and
    /// models without a known layout, skip: their checks live elsewhere or
    /// don't exist yet.
    static func current() throws -> Phone {
        let name = ProcessInfo.processInfo.environment["SIMULATOR_DEVICE_NAME"] ?? ""
        if name.contains("Duo") {
            throw XCTSkip("the iPhone Duo's layouts are checked by ios/test-duo.sh")
        }
        // Measured in the simulator: the Air's 6.5-inch screen splits sideways
        // like a Pro Max's; the e's doesn't.
        if name.hasSuffix(" Pro Max") || name.hasSuffix(" Plus") || name.hasPrefix("iPhone Air") {
            return Phone(name: name, wide: true)
        }
        if name.range(of: #"^iPhone \d+(e| Pro)?$"#, options: .regularExpression) != nil {
            return Phone(name: name, wide: false)
        }
        throw XCTSkip("no expected layout for \"\(name)\"")
    }

    /// Whether the list stays beside the chapter in `orientation`.
    func split(_ orientation: UIDeviceOrientation) -> Bool {
        wide && orientation.isLandscape
    }

    var description: String { name }
}
