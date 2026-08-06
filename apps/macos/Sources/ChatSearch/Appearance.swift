import AppKit
import CsTheme

/// The axis that is not a direction: which side of a theme the app draws, independently of which
/// direction each side's colours came from.
///
/// `chat-search-me9.8.9` shipped one axis — a direction, with the system deciding which of its two
/// palettes you saw — and this is the second (`chat-search-me9.8.22`). Three values rather than a
/// boolean, because *follow the system* is a thing somebody chooses and not the absence of a
/// choice: the difference shows the moment a preference is written down, where "no value" and
/// "system" have to be told apart.
enum Appearance: String, CaseIterable, Sendable {
    /// Whatever macOS is doing, which is what this app did before there was a way not to.
    case system
    case light
    case dark

    /// What `NSApp.appearance` is set to, and `nil` — inherit — is the whole of `system`.
    ///
    /// This one assignment is the entire mechanism. Every colour in the app is a dynamic `NSColor`
    /// whose provider is called with the appearance in force where it is being drawn
    /// (`Theme.dynamic`), so an override at the application reaches every token with no palette
    /// re-resolve, no view change, and no second code path for "the app is forcing dark".
    var nsAppearance: NSAppearance? {
        switch self {
        case .system: nil
        case .light: NSAppearance(named: .aqua)
        case .dark: NSAppearance(named: .darkAqua)
        }
    }

    /// For a `--help` line, and for a complaint about a value this build has no reading for.
    static var names: String { allCases.map(\.rawValue).joined(separator: "|") }
}

/// Whether the override reached the *colours*, which is not the same question as whether the
/// window went dark.
///
/// `chat-search-me9.8.22` asked for a probe rather than an assumption, and the failure it is
/// looking for is one a picture cannot rule out: an appearance that reached only the window's own
/// chrome draws a dark frame around a light window, and a screenshot of that is easy to read as a
/// screenshot of a light theme. So the reading is taken from the app's own view tree — the
/// appearance in force where a row is drawn — and the colour is sampled back out of the same token
/// a view would have asked for.
@MainActor
enum AppearanceProbe {
    /// One line, printed by the scripted runs beside their other reports. It rides with `--shot`
    /// and `--measure` rather than with every launch for the reason the rest of those reports do:
    /// this is evidence for somebody checking, and a person looking at the window can already see
    /// which side they got.
    static func line(theme: Theme, view: NSView, asked: Appearance) -> String {
        let appearance = view.effectiveAppearance
        let head =
            "appearance: \(asked.rawValue) → the view tree draws in \(appearance.name.rawValue)"
        let (rgb, side) = theme.drawn(.bg, in: appearance)
        guard let rgb, let side else {
            return head + ", and --bg resolves to \(rgb?.hex ?? "nothing") — which is NEITHER "
                + "authored side of \(theme.name). The override did not reach the provider."
        }
        return head + ", --bg \(rgb.hex), which is the \(side) side of \(theme.name)."
    }
}
