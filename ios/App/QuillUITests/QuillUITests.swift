import XCTest

/// End-to-end checks of the app in a simulator, against the bundled sample
/// novel and the real Rust core. The layout on each iPhone size, upright and
/// sideways, is checked in ``PhoneLayoutUITests``; the iPhone Duo's poses are
/// checked here, with `ios/test-duo.sh` folding the phone.
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
        app.openSample()
        XCTAssertTrue(app.staticTexts["The Lamp Room"].waitForExistence(timeout: 10))
        XCTAssertTrue(app.staticTexts["Low Tide"].exists)
        XCTAssertTrue(app.staticTexts["What the Fog Kept"].exists)
        let counts = app.staticTexts.matching(NSPredicate(format: "label ENDSWITH ' words'"))
        XCTAssertGreaterThan(counts.count, 0)
        attach(app, "chapter list")
    }

    /// Words typed and then left by backgrounding and killing the app are there
    /// on the next launch (#1232).
    func testTypedWordsSurviveBackgroundingAndAKill() {
        app.checkTypedWordsSurviveAKill()
    }

    /// A book started on the phone opens on its first chapter with the
    /// keyboard up, ready to type (#1297).
    func testANewBookOpensReadyToType() {
        let title = "Harbor \(Int.random(in: 1000...9999))"
        app.openLibrary()
        app.buttons["New Book…"].tap()
        let field = app.textFields["NewBookTitle"]
        XCTAssertTrue(field.waitForExistence(timeout: UITest.patience), "the new book form should open")
        field.tap()
        field.typeText(title)
        app.buttons["Create"].tap()

        let text = app.textViews.firstMatch
        XCTAssertTrue(text.waitForExistence(timeout: UITest.patience), "the first chapter should open")
        XCTAssertTrue(
            text.waitForKeyboardFocus(timeout: UITest.patience), "the chapter should be ready to type in")
        text.typeText("The tide was out.")
        let typed = XCTNSPredicateExpectation(
            predicate: NSPredicate(format: "value CONTAINS %@", "The tide was out."), object: text)
        XCTAssertEqual(
            XCTWaiter().wait(for: [typed], timeout: UITest.patience), .completed,
            "the words should be in the chapter")
        attach(app, "new book")

        // Delete it again, so the next test's launch doesn't reopen it.
        app.openLibrary()
        let row = app.buttons[title]
        XCTAssertTrue(row.waitForExistence(timeout: UITest.patience), "the library lists the new book")
        row.swipeLeft()
        app.buttons["Delete"].tap()
        app.buttons["Delete Book"].tap()
        XCTAssertTrue(row.waitForNonExistence(timeout: UITest.patience), "the book is deleted")
    }

    /// A chapter's earlier version can be read, compared, and restored, and
    /// Undo takes the restore back (#1299).
    func testAnEarlierVersionCanBeRestored() {
        app.openSample()
        app.openChapter("Low Tide")
        let text = app.textViews.firstMatch
        XCTAssertTrue(text.waitForExistence(timeout: UITest.patience), "the chapter should open")
        text.tap()
        XCTAssertTrue(text.waitForKeyboardFocus(timeout: UITest.patience), "the chapter should take the keyboard")
        let word = "Gull\(Int.random(in: 1000...9999))"
        text.typeText(" \(word).")

        // Leaving the app saves a version with the word in it.
        XCUIDevice.shared.press(.home)
        _ = app.wait(for: .runningBackgroundSuspended, timeout: 10)
        app.activate()

        let history = app.buttons["History"]
        XCTAssertTrue(history.waitForHittable(timeout: UITest.patience), "the chapter offers its history")
        history.tap()
        let versions = app.cells
        XCTAssertTrue(versions.element(boundBy: 1).waitForExistence(timeout: UITest.patience), "two versions at least")
        // The oldest version is the sample as it came, without the word.
        versions.element(boundBy: versions.count - 1).tap()
        let old = app.textViews["VersionText"]
        XCTAssertTrue(old.waitForExistence(timeout: UITest.patience), "the version should open")
        let version = old.value as? String ?? ""
        XCTAssertFalse(version.contains(word), "the old version is from before the word")
        app.segmentedControls["VersionMode"].buttons["Compare"].tap()
        XCTAssertTrue((old.value as? String ?? "").contains(word), "the comparison shows the word added since")
        attach(app, "comparison")

        app.buttons["Restore This Version"].tap()
        let confirm = app.buttons["Restore"]
        XCTAssertTrue(confirm.waitForHittable(timeout: UITest.patience), "the restore asks to confirm")
        confirm.tap()
        XCTAssertTrue(text.waitForExistence(timeout: UITest.patience), "back at the chapter")
        let restored = NSPredicate(format: "NOT (value CONTAINS %@)", word)
        XCTAssertEqual(
            XCTWaiter().wait(for: [XCTNSPredicateExpectation(predicate: restored, object: text)], timeout: UITest.patience),
            .completed, "the chapter went back to the old version")
        XCTAssertEqual(text.value as? String, version, "exactly the old version, quotes and all")
    }

    /// Exporting the book opens the share sheet with the file (#1300).
    func testExportingTheBookOpensTheShareSheet() {
        app.openSample()
        let export = app.buttons["Export"]
        XCTAssertTrue(export.waitForHittable(timeout: UITest.patience), "the book offers an export")
        export.tap()
        app.buttons["Manuscript (Word)"].tap()
        let sheet = app.otherElements["ActivityListView"]
        XCTAssertTrue(sheet.waitForExistence(timeout: UITest.patience), "the share sheet should open")
        attach(app, "share sheet")
    }

    // MARK: - The iPhone Duo (#1234, #1284, #1285)

    /// Unfolded and upright, the Duo keeps the chapter list beside the
    /// chapter (#1234).
    func testUnfoldedDuoShowsTheListBesideTheChapter() throws {
        try requirePose("open")
        app.openSample()
        app.openChapter("The Lamp Room")
        XCTAssertTrue(app.textViews.firstMatch.waitForExistence(timeout: 10))
        attach(app, "unfolded")
        XCTAssertGreaterThan(app.windows.firstMatch.frame.width, 900, "unfolded and upright is wide")
        XCTAssertTrue(app.staticTexts["Low Tide"].waitForExistence(timeout: 5))
        XCTAssertTrue(app.staticTexts["Low Tide"].isHittable, "the list should stay visible")
        // Choosing another chapter keeps the split.
        app.staticTexts["Low Tide"].tap()
        XCTAssertTrue(app.staticTexts["The Lamp Room"].isHittable)
    }

    /// Half open and upright, the chapter is a two-page spread, one page on
    /// each side of the crease; a swipe turns it, and "Add TODO" on a
    /// selection marks the chapter (#1284).
    func testHalfOpenAndUprightShowsTheChapterAsABook() throws {
        try requirePose("book")
        // Large type, so the sample's short chapter runs to several spreads.
        app.terminate()
        app.launchArguments = ["-UIPreferredContentSizeCategoryName", "UICTContentSizeCategoryAccessibilityXXXL"]
        app.launch()
        app.openSample()
        let spread = app.scrollViews["BookSpread"]
        if !spread.waitForExistence(timeout: 5) { app.openChapter("The Lamp Room") }
        XCTAssertTrue(spread.waitForExistence(timeout: 10), "half open and upright should show the spread")
        attach(app, "book")

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
        attach(app, "turned")

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
        attach(app, "marked")
    }

    /// Half open and sideways like a laptop, with the on-screen keyboard up,
    /// the chapter's text stays above the crease; with the keyboard put away,
    /// it uses the whole screen again (#1285).
    func testHalfOpenAndSidewaysKeepsTheWritingAboveTheKeyboard() throws {
        try requirePose("laptop")
        app.openSample()
        let text = app.textViews.firstMatch
        if !text.waitForExistence(timeout: 5) {
            // The list slides over the chapter here: show it, then choose.
            let sidebar = app.buttons["Show Sidebar"]
            if sidebar.waitForExistence(timeout: 5) { sidebar.tap() }
            app.openChapter("The Lamp Room")
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
        attach(app, "laptop")

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
            throw XCTSkip("needs the iPhone Duo in its \(pose) pose; ios/test-duo.sh runs it")
        }
    }
}
