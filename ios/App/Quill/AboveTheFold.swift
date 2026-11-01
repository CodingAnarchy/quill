import QuillKit
import SwiftUI
import UIKit

extension View {
    /// Keep this view above the crease while the iPhone Duo is half open like
    /// a laptop and the on-screen keyboard is up (#1285).
    func aboveTheFold() -> some View { modifier(AboveTheFold()) }
}

/// Half open and sideways, the Duo's lower half rests on the table and holds
/// the keyboard, and the writing belongs on the upper half. iOS already
/// keeps the text clear of the keyboard, but how high the keyboard reaches
/// depends on its kind and settings (with predictions off, it stops short of
/// the crease). This pads the view's bottom up to the crease, so the text,
/// and the line being typed, never sit on the fold.
///
/// Without the on-screen keyboard (a hardware keyboard, or the keyboard put
/// away), the view keeps the whole screen.
private struct AboveTheFold: ViewModifier {
    @Environment(AppState.self) private var app
    /// The bottom of the view in the window's coordinates, where the posture
    /// reports the crease.
    @State private var bottom: CGFloat = 0
    @State private var keyboardUp = false

    /// Taller than the bar above the keys, which is all that shows with a
    /// hardware keyboard.
    private static let keyboardHeight: CGFloat = 150

    func body(content: Content) -> some View {
        content
            .padding(.bottom, overlap)
            .onGeometryChange(for: CGFloat.self) { $0.frame(in: .global).maxY } action: {
                bottom = $0
            }
            .onReceive(NotificationCenter.default.publisher(for: UIResponder.keyboardWillChangeFrameNotification)) {
                let frame = $0.userInfo?[UIResponder.keyboardFrameEndUserInfoKey] as? CGRect
                keyboardUp = (frame?.height ?? 0) > Self.keyboardHeight
            }
            .onReceive(NotificationCenter.default.publisher(for: UIResponder.keyboardWillHideNotification)) { _ in
                keyboardUp = false
            }
    }

    /// How far the view reaches past the crease, and so how much to give up.
    private var overlap: CGFloat {
        guard keyboardUp, case .laptop(let crease) = app.posture else { return 0 }
        return max(0, bottom - crease)
    }
}
