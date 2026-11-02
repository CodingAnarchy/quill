import XCTest

/// End-to-end checks of the app in a simulator, against the bundled sample
/// novel and the real Rust core.
final class QuillUITests: XCTestCase {
    private var app: XCUIApplication!

    override func setUp() {
        continueAfterFailure = false
        app = XCUIApplication()
        XCUIDevice.shared.orientation = .portrait
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

    // MARK: - Helpers

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
