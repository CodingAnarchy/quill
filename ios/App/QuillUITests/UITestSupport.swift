import XCTest

/// Steps the UI tests share: opening the sample novel and its chapters, and
/// the type-then-kill check.
extension XCUIApplication {
    func openSample() {
        let sample = buttons["SampleNovel"]
        if sample.waitForExistence(timeout: 5) { sample.tap() }
    }

    func openChapter(_ title: String) {
        let row = staticTexts[title]
        if !row.waitForExistence(timeout: 5) {
            // Compact layout already showing a chapter: go back to the list.
            navigationBars.buttons.element(boundBy: 0).tap()
        }
        XCTAssertTrue(row.waitForExistence(timeout: 10))
        row.tap()
    }

    /// Whether `element` is on screen and tappable, without failing the
    /// test when it isn't there at all.
    func shows(_ element: XCUIElement) -> Bool {
        element.exists && element.isHittable
    }

    /// Type a new word into a chapter, then background and kill the app;
    /// the word is there on the next launch (#1232).
    func checkTypedWordsSurviveAKill(file: StaticString = #filePath, line: UInt = #line) {
        let word = "Zephyr\(Int.random(in: 1000...9999))"
        openSample()
        openChapter("Low Tide")
        let text = textViews.firstMatch
        XCTAssertTrue(text.waitForExistence(timeout: 10), file: file, line: line)
        text.tap()
        text.typeText(" \(word).")

        XCUIDevice.shared.press(.home)
        terminate()
        launch()
        openSample()
        openChapter("Low Tide")
        let reopened = textViews.firstMatch
        XCTAssertTrue(reopened.waitForExistence(timeout: 10), file: file, line: line)
        XCTAssertTrue((reopened.value as? String ?? "").contains(word), "lost the typed word", file: file, line: line)
    }
}

extension XCTestCase {
    /// Keep a screenshot of `app` with the test's results.
    func attach(_ app: XCUIApplication, _ name: String) {
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
