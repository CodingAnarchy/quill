import XCTest

/// Steps the UI tests share: opening the sample novel and its chapters, and
/// the type-then-kill check.
///
/// The waits are long because CI's runners are slow (steps take three to five
/// times as long as on a Mac), and a wait returns as soon as its condition
/// holds, so a long one costs nothing when things are quick. Every assertion
/// says what it was waiting for, so a CI failure explains itself.
enum UITest {
    /// How long a step that has to happen may take: a screen appearing, the
    /// keyboard arriving.
    static let patience: TimeInterval = 30
}

extension XCUIApplication {
    /// Open the sample novel from the library, wherever the app opened: the
    /// app reopens the last book, which another test may have created.
    func openSample() {
        openLibrary()
        let sample = buttons["SampleNovel"]
        // Books of the author's (or a test's) may push it down the list.
        let list = collectionViews.firstMatch
        for _ in 0..<5 {
            if sample.waitForExistence(timeout: 2) && sample.isHittable || !list.exists { break }
            list.swipeUp()
        }
        XCTAssertTrue(sample.waitForExistence(timeout: UITest.patience), "the library should list the sample")
        sample.tap(expecting: buttons["Projects"])
    }

    /// Back to the library from wherever the last test left the app: a
    /// chapter, or a project's chapter list.
    func openLibrary() {
        let newBook = buttons["New Book…"]
        for _ in 0..<3 where !newBook.waitForExistence(timeout: 3) {
            let projects = buttons["Projects"]
            if projects.exists && projects.isHittable {
                projects.tap()
            } else {
                navigationBars.buttons.element(boundBy: 0).tap()
            }
        }
        XCTAssertTrue(newBook.waitForExistence(timeout: UITest.patience), "the library should show")
    }

    func openChapter(_ title: String) {
        let row = staticTexts[title]
        if !row.waitForExistence(timeout: 5) {
            // Compact layout already showing a chapter: go back to the list.
            navigationBars.buttons.element(boundBy: 0).tap()
        }
        XCTAssertTrue(row.waitForExistence(timeout: UITest.patience), "the chapter list should show \"\(title)\"")
        row.tap(expecting: textViews.firstMatch)
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
        XCTAssertTrue(
            text.waitForExistence(timeout: UITest.patience), "the chapter should open", file: file, line: line)
        // On a slow runner a tap can land before the text view takes the
        // keyboard, and typing then has nowhere to go: wait for focus, and
        // tap once more if it doesn't come.
        text.tap()
        if !text.waitForKeyboardFocus(timeout: 5) { text.tap() }
        XCTAssertTrue(
            text.waitForKeyboardFocus(timeout: UITest.patience), "the chapter should take the keyboard", file: file,
            line: line)
        text.typeText(" \(word).")

        // The app saves as it leaves the foreground. Kill it only once iOS
        // has suspended it, which waits for that save's background task, or
        // after 10 seconds, as the simulator doesn't always suspend. On a slow
        // runner, killing right after Home can beat the save, which no person
        // switching apps could do.
        XCUIDevice.shared.press(.home)
        _ = wait(for: .runningBackgroundSuspended, timeout: 10)
        terminate()
        launch()
        openSample()
        openChapter("Low Tide")
        let reopened = textViews.firstMatch
        XCTAssertTrue(
            reopened.waitForExistence(timeout: UITest.patience), "the chapter should open again after relaunch",
            file: file, line: line)
        let saved = XCTNSPredicateExpectation(predicate: NSPredicate(format: "value CONTAINS %@", word), object: reopened)
        XCTAssertEqual(
            XCTWaiter().wait(for: [saved], timeout: UITest.patience), .completed, "lost the typed word", file: file,
            line: line)
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
    /// Wait for the element to have the keyboard, so typing reaches it.
    func waitForKeyboardFocus(timeout: TimeInterval) -> Bool {
        let focused = XCTNSPredicateExpectation(predicate: NSPredicate(format: "hasKeyboardFocus == true"), object: self)
        return XCTWaiter().wait(for: [focused], timeout: timeout) == .completed
    }

    /// Tap the element, and tap it once more if `result` doesn't come.
    ///
    /// The simulator sometimes drops a test's tap. XCTest sends each one
    /// through a virtual touch screen, and when backboardd registers that
    /// screen late it lets the touch down through ("unknown digitizer"), then
    /// skips the rest ("didn't see a previous touch down"), and UIKit cancels
    /// the touch. In the CI run that showed it (#1319), the app had the row on
    /// screen and an idle main thread, and nothing happened. The second tap
    /// comes only after the full wait, so a tap that was only slow isn't
    /// doubled. Use it for a tap that opens something `result` shows.
    func tap(expecting result: XCUIElement, timeout: TimeInterval = UITest.patience) {
        tap()
        if result.waitForHittable(timeout: timeout) || !(exists && isHittable) { return }
        tap()
    }

    /// Tap a button that closes what it's on (a confirmation's), and tap it
    /// once more if it's still there after the full wait: the same dropped
    /// tap as `tap(expecting:)`. What's behind a closing dialog can't be
    /// waited on instead, as XCTest fails a test that asks whether an element
    /// is hittable while it has no frame, which a view coming back from
    /// under a sheet briefly does.
    func tapUntilGone(timeout: TimeInterval = UITest.patience) {
        tap()
        if waitForNonExistence(timeout: timeout) { return }
        tap()
    }

    /// Wait for the element to be on screen and tappable, not just present.
    func waitForHittable(timeout: TimeInterval) -> Bool {
        let hittable = XCTNSPredicateExpectation(predicate: NSPredicate(format: "hittable == true"), object: self)
        return XCTWaiter().wait(for: [hittable], timeout: timeout) == .completed
    }
}
