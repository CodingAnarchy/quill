import XCTest

/// End-to-end checks of the app in a simulator, against the bundled sample
/// novel and the real Rust core.
final class QuillUITests: XCTestCase {
    private var app: XCUIApplication!

    override func setUp() {
        continueAfterFailure = false
        app = XCUIApplication()
        // A half-open Duo is turned by ios/test-duo.sh, which knows which way
        // is upright; XCTest's portrait isn't, on the Duo's inner screen.
        if ProcessInfo.processInfo.environment["QUILL_DUO_POSE"] == nil {
            XCUIDevice.shared.orientation = .portrait
        }
        app.launch()  // a fresh launch: the last test may have left a chapter open
    }

    /// The sample opens with its chapters and the desktop's word counts.
    func testTheSampleNovelListsItsChapters() {
        openSample()
        XCTAssertTrue(app.staticTexts["The Lamp Room"].waitForExistence(timeout: 10))
        XCTAssertTrue(app.staticTexts["Low Tide"].exists)
        XCTAssertTrue(app.staticTexts["What the Fog Kept"].exists)
        let counts = app.staticTexts.matching(NSPredicate(format: "label ENDSWITH ' words'"))
        XCTAssertGreaterThan(counts.count, 0)
        attach("chapter list")
    }

    /// Words typed and then left by backgrounding and killing the app are there
    /// on the next launch (#1232).
    func testTypedWordsSurviveBackgroundingAndAKill() {
        let word = "Zephyr\(Int.random(in: 1000...9999))"
        openSample()
        openChapter("Low Tide")
        let text = app.textViews.firstMatch
        XCTAssertTrue(text.waitForExistence(timeout: 10))
        text.tap()
        text.typeText(" \(word).")
        attach("editing")

        XCUIDevice.shared.press(.home)
        app.terminate()
        app.launch()
        openSample()
        openChapter("Low Tide")
        let reopened = app.textViews.firstMatch
        XCTAssertTrue(reopened.waitForExistence(timeout: 10))
        let value = reopened.value as? String ?? ""
        XCTAssertTrue(value.contains(word), "lost the typed word")
    }

    /// On a wide screen the chapter list stays beside the chapter (#1234).
    func testAWideScreenShowsTheListBesideTheChapter() throws {
        openSample()
        // The unfolded Duo is wide as it stands; a Pro Max gets there in
        // landscape. The folded Duo doesn't rotate, so it stays narrow.
        if !isWide { XCUIDevice.shared.orientation = .landscapeLeft }
        defer { XCUIDevice.shared.orientation = .portrait }
        openChapter("The Lamp Room")
        XCTAssertTrue(app.textViews.firstMatch.waitForExistence(timeout: 10))
        attach("wide")
        guard isWide else {
            let width = Int(app.windows.firstMatch.frame.width)
            throw XCTSkip("a \(width)pt window shows one column: unfold the Duo, or use a Pro Max")
        }
        XCTAssertTrue(app.staticTexts["Low Tide"].waitForExistence(timeout: 5))
        XCTAssertTrue(app.staticTexts["Low Tide"].isHittable, "the list should stay visible")
        // Choosing another chapter keeps the split.
        app.staticTexts["Low Tide"].tap()
        XCTAssertTrue(app.staticTexts["The Lamp Room"].isHittable)
    }

    // MARK: - The iPhone Duo half open (#1284, #1285)

    /// Half open and upright, the chapter is a two-page spread, one page on
    /// each side of the crease; a swipe turns it, and "Add TODO" on a
    /// selection marks the chapter (#1284).
    func testHalfOpenAndUprightShowsTheChapterAsABook() throws {
        try requirePose("book")
        // Large type, so the sample's short chapter runs to several spreads.
        app.terminate()
        app.launchArguments = ["-UIPreferredContentSizeCategoryName", "UICTContentSizeCategoryAccessibilityXXXL"]
        app.launch()
        openSample()
        let spread = app.scrollViews["BookSpread"]
        if !spread.waitForExistence(timeout: 5) { openChapter("The Lamp Room") }
        XCTAssertTrue(spread.waitForExistence(timeout: 10), "half open and upright should show the spread")
        attach("book")

        let crease = app.windows.firstMatch.frame.midX
        let pages = app.textViews.matching(identifier: "BookPage")
        let left = pages.element(boundBy: 0).frame
        let right = pages.element(boundBy: 1).frame
        XCTAssertLessThan(left.maxX, crease, "the left page should end before the crease")
        XCTAssertGreaterThan(right.minX, crease, "the right page should start after the crease")
        XCTAssertEqual(left.width, right.width, accuracy: 1)

        // Turn to the second spread. Only the spread on screen is visible to
        // accessibility, so page 3 appears once it's turned to.
        let numbers = app.staticTexts.matching(identifier: "PageNumber")
        let first = numbers.element(boundBy: 0).label
        XCTAssertTrue(first.hasPrefix("Page 1 of "), "the first spread should start at page 1, not \(first)")
        XCTAssertGreaterThan(Int(first.dropFirst("Page 1 of ".count)) ?? 0, 2, "large type should run past a spread")
        spread.swipeLeft()
        let pageThree = app.staticTexts.matching(NSPredicate(format: "label BEGINSWITH 'Page 3 of'")).firstMatch
        XCTAssertTrue(pageThree.waitForHittable(timeout: 5), "a swipe should turn to pages 3 and 4")
        attach("turned")

        // Mark a word on the left-hand page, which now holds only its own text.
        let page = pages.element(boundBy: 0)
        XCTAssertFalse((page.value as? String ?? "").hasPrefix("The stairs"), "page 3 shouldn't read as page 1")
        // A long press selects a word; XCTest's double tap doesn't, on a page.
        page.coordinate(withNormalizedOffset: CGVector(dx: 0.3, dy: 0.05)).press(forDuration: 1.2)
        let add = app.descendants(matching: .any)["Add TODO"].firstMatch
        XCTAssertTrue(add.waitForExistence(timeout: 5), "a selection should offer Add TODO")
        add.tap()
        let marked = pages.matching(NSPredicate(format: "value CONTAINS 'xxx '")).firstMatch
        XCTAssertTrue(marked.waitForExistence(timeout: 5), "the marker should be on the page")
        XCTAssertTrue(marked.isHittable, "the spread should stay where the marker went")
        attach("marked")
    }

    /// Half open and sideways like a laptop, with the on-screen keyboard up,
    /// the chapter's text stays above the crease; with the keyboard put away,
    /// it uses the whole screen again (#1285).
    func testHalfOpenAndSidewaysKeepsTheWritingAboveTheKeyboard() throws {
        try requirePose("laptop")
        openSample()
        let text = app.textViews.firstMatch
        if !text.waitForExistence(timeout: 5) {
            // The list slides over the chapter here: show it, then choose.
            let sidebar = app.buttons["Show Sidebar"]
            if sidebar.waitForExistence(timeout: 5) { sidebar.tap() }
            openChapter("The Lamp Room")
        }
        XCTAssertTrue(text.waitForExistence(timeout: 10))
        let crease = app.windows.firstMatch.frame.midY
        XCTAssertGreaterThan(app.windows.firstMatch.frame.height, app.windows.firstMatch.frame.width, "sideways")
        // Choosing the chapter puts the list away, so the writing is uncovered.
        XCTAssertFalse(app.staticTexts["Low Tide"].isHittable, "the list shouldn't cover the chapter")

        text.tap()
        let keyboard = app.keyboards.firstMatch
        XCTAssertTrue(keyboard.waitForExistence(timeout: 10), "the on-screen keyboard should be up")
        XCTAssertGreaterThan(keyboard.frame.minY, crease - 150, "the keyboard belongs on the lower half")
        XCTAssertLessThanOrEqual(text.frame.maxY, crease + 1, "the writing should stay above the crease")
        // Tap the keys: `typeText` types as a hardware keyboard would, and
        // iOS puts the on-screen one away for it.
        let before = text.value as? String ?? ""
        for key in ["g", "u", "l", "l"] {
            // Upper or lower case, whichever the keyboard is showing.
            keyboard.keys.matching(NSPredicate(format: "label ==[c] %@", key)).firstMatch.tap()
        }
        XCTAssertNotEqual(text.value as? String ?? "", before, "the keys should type into the chapter")
        XCTAssertTrue(keyboard.exists, "the on-screen keyboard should still be up")
        XCTAssertLessThanOrEqual(text.frame.maxY, crease + 1, "typing shouldn't push the writing onto the fold")
        attach("laptop")

        // With the keyboard put away, the chapter has the whole screen again.
        app.buttons["Hide the keyboard"].tap()
        XCTAssertTrue(
            keyboard.waitForNonExistence(timeout: 5), "the keyboard should go away")
        XCTAssertGreaterThan(text.frame.maxY, crease + 100, "without the keyboard, the chapter should fill the screen")
    }

    // MARK: - Helpers

    /// Skip unless `ios/test-duo.sh` put the Duo in `pose` for this run. No
    /// API can fold the simulator, so the script does it and says so.
    private func requirePose(_ pose: String) throws {
        let current = ProcessInfo.processInfo.environment["QUILL_DUO_POSE"]
        guard current == pose else {
            throw XCTSkip("needs the iPhone Duo half open as a \(pose); ios/test-duo.sh runs it")
        }
    }

    /// Wide enough for a regular width, where the list sits beside the
    /// chapter: a Pro Max in landscape (about 950pt) or the unfolded Duo
    /// (about 950 by 670).
    private var isWide: Bool {
        app.windows.firstMatch.frame.width >= 900
    }

    private func openSample() {
        let sample = app.buttons["SampleNovel"]
        if sample.waitForExistence(timeout: 5) { sample.tap() }
    }

    private func openChapter(_ title: String) {
        let row = app.staticTexts[title]
        if !row.waitForExistence(timeout: 5) {
            // Compact layout already showing a chapter: go back to the list.
            app.navigationBars.buttons.element(boundBy: 0).tap()
        }
        XCTAssertTrue(row.waitForExistence(timeout: 10))
        row.tap()
    }

    private func attach(_ name: String) {
        let shot = XCTAttachment(screenshot: app.screenshot())
        shot.name = name
        shot.lifetime = .keepAlways
        add(shot)
    }
}

extension XCUIElement {
    /// Wait for the element to be on screen and tappable, not just present.
    func waitForHittable(timeout: TimeInterval) -> Bool {
        let hittable = XCTNSPredicateExpectation(predicate: NSPredicate(format: "hittable == true"), object: self)
        return XCTWaiter().wait(for: [hittable], timeout: timeout) == .completed
    }
}
