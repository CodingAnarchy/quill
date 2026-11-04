import OSLog
import UIKit

/// A record of every touch the app receives and what it hit, written to the
/// system log, for the UI tests only (they launch the app with
/// `QUILL_TRACE_TOUCHES=1`). When a CI run's tap seems to do nothing, the
/// simulator's log saved beside the results bundle shows whether the touch
/// reached the app at all. Never on for the author.
enum TouchTrace {
    static let log = Logger(subsystem: "dev.quill.Quill", category: "trace")
    static let isOn = ProcessInfo.processInfo.environment["QUILL_TRACE_TOUCHES"] == "1"

    @MainActor private static var watching = false

    /// Watch the touches on every window the app has now, and the main
    /// thread for stalls.
    @MainActor static func install() {
        guard isOn else { return }
        if !watching {
            watching = true
            watchMainThread()
        }
        for case let scene as UIWindowScene in UIApplication.shared.connectedScenes {
            for window in scene.windows
            where !(window.gestureRecognizers ?? []).contains(where: { $0 is Recognizer }) {
                window.addGestureRecognizer(Recognizer())
            }
        }
    }

    /// Log each time the main thread takes longer than a quarter second to
    /// answer, and for how long, so a dropped touch can be told apart from a
    /// busy app.
    private static func watchMainThread() {
        let watcher = Thread {
            while true {
                let sent = DispatchTime.now()
                let answered = DispatchSemaphore(value: 0)
                DispatchQueue.main.async { answered.signal() }
                if answered.wait(timeout: sent + .milliseconds(250)) == .timedOut {
                    answered.wait()
                    let stalled = (DispatchTime.now().uptimeNanoseconds - sent.uptimeNanoseconds) / 1_000_000
                    log.notice("main thread stalled for \(stalled)ms")
                }
                Thread.sleep(forTimeInterval: 0.1)
            }
        }
        watcher.name = "TouchTrace main-thread watcher"
        watcher.start()
    }

    /// Note an action the app took, so the log shows whether a tap that
    /// arrived went on to do anything.
    static func note(_ action: String) {
        guard isOn else { return }
        log.notice("action: \(action, privacy: .public)")
    }

    /// Sees touches without taking part: it never recognizes, cancels, or
    /// delays them, so the app behaves exactly as it does untraced. It follows
    /// each touch to its end, so the log shows how long it was held, whether
    /// it ended or was cancelled, and which of the app's gestures were acting
    /// on it then.
    private final class Recognizer: UIGestureRecognizer {
        private var downAt: [ObjectIdentifier: TimeInterval] = [:]

        override init(target: Any? = nil, action: Selector? = nil) {
            super.init(target: target, action: action)
            cancelsTouchesInView = false
            delaysTouchesBegan = false
            delaysTouchesEnded = false
        }

        override func touchesBegan(_ touches: Set<UITouch>, with event: UIEvent) {
            for touch in touches {
                downAt[ObjectIdentifier(touch)] = touch.timestamp
                trace("began", touch)
            }
        }

        override func touchesEnded(_ touches: Set<UITouch>, with event: UIEvent) {
            finish("ended", touches)
        }

        override func touchesCancelled(_ touches: Set<UITouch>, with event: UIEvent) {
            finish("cancelled", touches)
        }

        override func reset() {
            super.reset()
            downAt = [:]
        }

        override func canPrevent(_ preventedGestureRecognizer: UIGestureRecognizer) -> Bool { false }
        override func canBePrevented(by preventingGestureRecognizer: UIGestureRecognizer) -> Bool { false }

        private func finish(_ phase: String, _ touches: Set<UITouch>) {
            for touch in touches {
                let held = downAt.removeValue(forKey: ObjectIdentifier(touch)).map { touch.timestamp - $0 } ?? -1
                let acting = (touch.gestureRecognizers ?? [])
                    .filter { $0 !== self && $0.state != .possible && $0.state != .failed }
                    .map { "\(type(of: $0))(\($0.state.rawValue))" }
                    .joined(separator: " ")
                let shown = touch.view?.window == nil ? ", its view gone from the window" : ""
                trace("\(phase) after \(Int(held * 1000))ms [\(acting)]\(shown)", touch)
            }
            if downAt.isEmpty { state = .failed }
        }

        private func trace(_ phase: String, _ touch: UITouch) {
            let point = touch.location(in: nil)
            let hit = touch.view.map { String(describing: type(of: $0)) } ?? "nothing"
            TouchTrace.log.notice(
                "touch \(phase, privacy: .public) at \(Int(point.x)),\(Int(point.y)) on \(hit, privacy: .public)")
        }
    }
}
