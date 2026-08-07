import AppKit
import SwiftUI

/// How far into its own window this app may not draw, because three buttons are already there.
///
/// ADR 27 gave the window `.fullSizeContentView`, so the content reaches the top edge and the band
/// the traffic lights sit in is `--bg` rather than a grey the system chose. What it does not give
/// back is the lights themselves: they stay system-drawn, on top, at coordinates nothing in this
/// repository decides. The first strip of the content — the search bar — therefore starts to the
/// right of them.
///
/// **Measured off the window rather than stated**, and the difference is not hypothetical. The
/// same SDK bump ADR 27 is about moved the titlebar from 28 pt to 32 and the window's corner
/// radius from ~15 to ~21; a number typed into a view here would have been wrong on the release
/// this bead was written against. `standardWindowButton` is the window saying where it actually
/// put them, and it goes on being right when Apple moves them again.
enum TitleBar {
    /// What the measurement returns on macOS 26.5.2 at the moment this was written, kept as the
    /// answer for a window that has no buttons to ask — a borderless one, or a style mask some
    /// later run assembles differently. Insetting by the wrong amount puts the chips a few points
    /// off; insetting by nothing puts them under the close button.
    static let statedInset: CGFloat = 82

    /// The right edge of the last light, plus the gap the strip would have had from the window
    /// edge anyway.
    ///
    /// All three buttons are asked and the furthest is taken, rather than asking the zoom button
    /// and assuming it is last: which of them is present is a property of the style mask — a
    /// window without `.miniaturizable` has two — and `max` is right for every arrangement of
    /// them, including the one where a later macOS reverses the order.
    @MainActor
    static func inset(of window: NSWindow, gap: CGFloat = 12) -> CGFloat {
        let lights: [NSWindow.ButtonType] = [.closeButton, .miniaturizeButton, .zoomButton]
        let edge = lights.compactMap { window.standardWindowButton($0)?.frame.maxX }.max()
        guard let edge else { return statedInset }
        return edge + gap
    }
}

private struct TitleBarInsetKey: EnvironmentKey {
    /// The stated number, so a preview or a test that builds the view tree without a window gets a
    /// strip that is inset rather than one drawn under the lights. Every window this app opens
    /// overrides it with the measurement.
    static let defaultValue = TitleBar.statedInset
}

extension EnvironmentValues {
    /// Where the content may start, at the top of the window only. Read by the one strip that is
    /// in the lights' band; everything below it is inset by the window's own margin and has no
    /// business knowing this number.
    var titleBarInset: CGFloat {
        get { self[TitleBarInsetKey.self] }
        set { self[TitleBarInsetKey.self] = newValue }
    }
}
