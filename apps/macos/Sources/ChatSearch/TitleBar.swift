import AppKit
import SwiftUI

/// The band at the top of the window that this app draws and does not entirely own.
///
/// ADR 27 gave the window `.fullSizeContentView`, so the content reaches the top edge and the band
/// the traffic lights sit in is `--bg` rather than a grey the system chose. What it does not give
/// back is the lights themselves: they stay system-drawn, on top, at coordinates nothing in this
/// repository decides. So the first strip of the content — the search bar — starts to the right of
/// them and is at least as tall as the band they are centred in.
///
/// **Both numbers are measured off the window rather than stated**, and the difference is not
/// hypothetical. The same SDK bump ADR 27 is about moved this band from 28 pt to 32 and the
/// window's corner radius from ~15 to ~21; either number typed into a view here would already have
/// been wrong once, on a release nobody had to opt in to. The window knows both before it is ever
/// shown, which is what makes asking it cheaper than remembering.
enum TitleBar {
    /// What a window with a titlebar reports, and there is nothing to draw in the band without
    /// one. Both fields are in points, in the content view's coordinates.
    struct Metrics: Sendable, Equatable {
        /// Where the content may start, at the top of the window only: the right edge of the last
        /// traffic light, plus the gap the strip would have had from the window edge anyway.
        var inset: CGFloat
        /// How tall the band is. The strip in it may be taller — it is, by 2 pt, because the chips
        /// and the field are what they are — but it may not be shorter, or a rule meant to sit
        /// under the search bar lands across the lights instead.
        var band: CGFloat

        /// What the measurement returns on macOS 26.5.2 at the moment this was written, kept as
        /// the answer for a window with no titlebar to ask — a borderless one, or a style mask
        /// some later run assembles differently. Insetting by a slightly wrong amount moves the
        /// chips a few points; insetting by nothing puts them under the close button.
        static let stated = Metrics(inset: 81, band: 32)
    }

    /// A full-screen window has neither. The lights move into the auto-hiding overlay and the
    /// band goes to zero, so the strip starts at the window edge like any other row.
    ///
    /// **This has to be read off the style mask and cannot be inferred from the buttons**, which
    /// is the trap: `standardWindowButton(_:)?.frame.maxX` goes on reporting 69 in full screen,
    /// the same as windowed, so a measurement that only asked them would inset 81 points into a
    /// band that is not there. `contentLayoutRect` tells the truth — it becomes the whole frame —
    /// and `.fullScreen` in the mask says the same thing without depending on that being true of
    /// every future release.
    static let fullScreen = Metrics(inset: 0, band: 0)

    /// Both numbers, off a real window, in whichever of the two states it is in.
    ///
    /// All three buttons are asked and the furthest is taken, rather than asking the zoom button
    /// and assuming it is last: which of them exists is a property of the style mask — a window
    /// without `.miniaturizable` has two — and `max` is right for every arrangement of them,
    /// including one where a later macOS reverses the order. The band is the height the window
    /// says it is *not* laying content out in, which is the only definition that does not involve
    /// naming a release.
    @MainActor
    static func metrics(of window: NSWindow, gap: CGFloat = 12) -> Metrics {
        guard !window.styleMask.contains(.fullScreen) else { return fullScreen }
        let lights: [NSWindow.ButtonType] = [.closeButton, .miniaturizeButton, .zoomButton]
        let edge = lights.compactMap { window.standardWindowButton($0)?.frame.maxX }.max()
        let band = window.frame.height - window.contentLayoutRect.height
        return Metrics(
            inset: edge.map { $0 + gap } ?? Metrics.stated.inset,
            band: band > 0 ? band : Metrics.stated.band)
    }
}

private struct TitleBarKey: EnvironmentKey {
    /// The stated numbers, so a view tree built without a window is inset rather than drawn under
    /// the lights. Every window this app opens overrides them with the measurement.
    static let defaultValue = TitleBar.Metrics.stated
}

extension EnvironmentValues {
    /// Read by the one strip that is in the lights' band. Everything below it is inset by the
    /// window's ordinary 12 pt margin and has no business knowing any of this.
    var titleBar: TitleBar.Metrics {
        get { self[TitleBarKey.self] }
        set { self[TitleBarKey.self] = newValue }
    }
}
