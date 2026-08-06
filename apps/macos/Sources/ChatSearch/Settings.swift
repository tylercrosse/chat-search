import AppKit
import CsTheme
import Observation

/// The three settings, alive, so that changing one changes what is on screen rather than what the
/// next launch will look like.
///
/// `chat-search-me9.8.22` made all four reachable by flag and persisted, resolved once at launch
/// and handed down as a `let`. That is the right shape for a flag and the wrong one for a window:
/// a settings window whose controls only took effect on relaunch would be a picture of a
/// preference. So the resolved `Choice` becomes state here, every view reads the theme off the
/// environment (`Root`), and the whole of "preview on selection" is that this object is observed.
///
/// **It owns the choice; it does not own the rules.** Which keys the four settings live in, what a
/// side carries, and what two menus naming one direction mean are all `ThemeChoice`'s, so the
/// window and the flags write the same keys by construction rather than by agreement.
@MainActor
@Observable
final class ThemeSettings {
    /// Which side is drawn, whatever macOS is doing.
    private(set) var appearance: Appearance
    /// The direction each side takes its colours from. Colour is all a side carries.
    private(set) var light: Theme
    private(set) var dark: Theme
    /// Where the type scale and the geometry come from, for both sides. No control stands for this
    /// one — it moves when the two side menus agree, and `SettingsForm` says so in a caption rather
    /// than leaving a setting that changes the window invisible.
    private(set) var layout: Theme
    /// A token set read off disk, when this launch found one. Not a control and not remembered —
    /// the file is the memory — but the window has to know, because while it is in force it is
    /// what is on screen and the two menus below are choosing what comes back without it
    /// (`chat-search-me9.8.10`).
    private(set) var user: Theme?
    /// And where it was read from, which is the part somebody who has forgotten needs.
    private(set) var userFile: URL?
    /// What the views draw. Stored rather than computed: `Theme.init` resolves every colour token
    /// into a dynamic `NSColor`, which is work worth doing when a choice changes and not once per
    /// `body`.
    private(set) var theme: Theme

    /// False for a scripted run, and it gates every write here for the reason it gates the four in
    /// `ThemeChoice.resolve`: `--shot` drives these controls to show that driving them works
    /// (`Measure.settings`), and a run that took a picture of somebody's preferences and left its
    /// own behind would be the same defect from a new direction.
    private let remembers: Bool

    /// The two surfaces SwiftUI does not own — the application's appearance and the window's own
    /// ground. Set by `AppHost`, which is the only thing that has a window.
    @ObservationIgnored var onChange: (() -> Void)?

    init(_ choice: ThemeChoice.Choice, remembers: Bool) {
        appearance = choice.appearance
        light = choice.light
        dark = choice.dark
        layout = choice.layout
        user = choice.user
        userFile = choice.userFile
        theme = choice.theme
        self.remembers = remembers
    }

    func choose(appearance: Appearance) {
        guard appearance != self.appearance else { return }
        self.appearance = appearance
        if remembers { ThemeChoice.remember(appearance: appearance) }
        onChange?()
    }

    func choose(light: Theme) {
        guard light.name != self.light.name else { return }
        self.light = light
        sidesChanged()
    }

    func choose(dark: Theme) {
        guard dark.name != self.dark.name else { return }
        self.dark = dark
        sidesChanged()
    }

    /// Both menus on one direction is that direction whole, which is where the layout moves.
    private func sidesChanged() {
        layout = ThemeChoice.whole(light, dark) ?? layout
        // `user` rides along rather than being dropped, so that a menu moved while a token set off
        // disk is in force changes what comes back when the file goes and not what is on screen.
        // The window disables both menus in that state; this is what makes it true rather than
        // enforced in one place (`SettingsForm`).
        theme = ThemeChoice.Choice(
            light: light, dark: dark, layout: layout, user: user, appearance: appearance
        ).theme
        if remembers { ThemeChoice.remember(light: light, dark: dark) }
        onChange?()
    }
}
