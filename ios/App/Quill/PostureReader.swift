import QuillKit
import SwiftUI
import UIKit

/// Reports the iPhone Duo's ``Posture`` to `onChange`, from its hinge and
/// from where its screen sits under this view. Place it as a full-size
/// background of the view whose layout depends on the posture, so the crease
/// comes back in that view's coordinates.
///
/// The hinge API is new in iOS 27.1 (and its SDK). Built with an older SDK,
/// or run on an older iOS, every device reads as `.flat`.
struct PostureReader: UIViewRepresentable {
    let onChange: (Posture) -> Void

    func makeUIView(context: Context) -> PostureReaderView {
        let view = PostureReaderView()
        view.onChange = onChange
        return view
    }

    func updateUIView(_ view: PostureReaderView, context: Context) {
        view.onChange = onChange
    }
}

final class PostureReaderView: UIView {
    var onChange: (Posture) -> Void = { _ in }
    /// The hinge is half open and has stopped moving.
    private var halfOpen = false
    private var angle: CGFloat = 0
    private var settling: Task<Void, Never>?
    private var posture = Posture.flat

    /// How long the hinge must hold still before half open counts. Opening
    /// or closing passes through every angle in between, and the hinge reads
    /// as partially open the whole way (from the outer screen, too).
    private static let settle = Duration.milliseconds(300)
    /// A smaller change of angle (in radians, about 5°) is a hand's jitter,
    /// not the phone being opened or closed.
    private static let jitter: CGFloat = 0.09

    override init(frame: CGRect) {
        super.init(frame: frame)
        isUserInteractionEnabled = false
        // Xcode 27.1 is the first with the hinge API, and the first with
        // Swift 6.4.1; `canImport` can't test for a UIKit header.
        #if compiler(>=6.4.1)
            if #available(iOS 27.1, *) {
                addInteraction(
                    UIHingeInteraction { [weak self] _, update in
                        self?.hingeMoved(
                            partiallyOpen: update.hinge?.status == .partiallyOpen, angle: update.hinge?.angle ?? 0)
                    })
            }
        #endif
    }

    required init?(coder: NSCoder) { fatalError("not used from a storyboard") }

    /// Open or closed counts at once. Half open counts once the hinge has
    /// held within a few degrees of one angle for a moment; moving further
    /// than that drops back to flat and starts the wait again. Measuring from
    /// where the hinge came to rest, not from the last update, catches a slow
    /// fold made of small steps.
    private func hingeMoved(partiallyOpen: Bool, angle: CGFloat) {
        guard partiallyOpen else {
            settling?.cancel()
            settling = nil
            halfOpen = false
            report()
            return
        }
        if abs(angle - self.angle) > Self.jitter {
            self.angle = angle
            settling?.cancel()
            settling = nil
            if halfOpen {
                halfOpen = false
                report()
            }
        }
        guard !halfOpen, settling == nil else { return }
        settling = Task { [weak self] in
            try? await Task.sleep(for: Self.settle)
            guard !Task.isCancelled, let self else { return }
            settling = nil
            halfOpen = true
            report()
        }
    }

    // Turning the phone from book to laptop changes the layout, not the hinge.
    override func layoutSubviews() {
        super.layoutSubviews()
        report()
    }

    private func report() {
        guard let screen = window?.windowScene?.screen else { return }
        let next = Posture(partiallyOpen: halfOpen, screen: convert(screen.bounds, from: screen.coordinateSpace))
        guard next != posture else { return }
        posture = next
        onChange(next)
    }
}
