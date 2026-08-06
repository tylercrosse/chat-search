import CsTheme
import Foundation

/// What this launch draws, and everything this app remembers between launches.
///
/// Three settings on two axes (`chat-search-me9.8.22`): an **appearance** of system, light or dark,
/// and a **direction** for each side. `chat-search-me9.8.9` shipped one axis — a direction, with
/// the system deciding which of its two palettes you saw — so "GitHub Light in the day and
/// Solarized Dark at night" could not be said at all. Now it can.
///
/// The seam under it needed nothing to make either possible. `Theme` keeps `light` and `dark` as
/// two independently solved `Palette`s and resolves each token through one dynamic `NSColor`, so a
/// side is swapped by composing a value and an appearance is forced by one `NSApp.appearance` — no
/// palette re-resolve, no view change (`chat-search-me9.8.8`). What had to be decided is where the
/// three answers come from and where they go, and both are here rather than spread between four
/// flags and a view.
///
/// ## What a side carries, and what it does not
///
/// A side override is **colour only**. The type scale and the geometry come from the direction,
/// once, for both sides — `Theme.composed` carries the argument, and the short of it is that an
/// appearance flip happens at sunset with nobody watching, so it may change what the window looks
/// like and may not change where the words are.
///
/// ## Why a flag is the affordance
///
/// This app has no bundle, no Dock icon and no menu bar: it is launched by
/// `swift run -c release chat-search`, so a terminal *is* its front door and a flag is the
/// idiomatic control for it. These four are not in the family of `--size`, `--group` and
/// `--measure` — those are instruments, deliberately not preferences, and these stick. The settings
/// window is `chat-search-me9.8.21`, and it is a surface over this rather than the way in.
enum ThemeChoice {
    /// The direction: colour on both sides unless a side says otherwise, and the type scale and the
    /// geometry for both sides always. The key `chat-search-me9.8.9` wrote, still meaning what it
    /// meant then.
    static let directionKey = "theme"
    /// A direction whose *light* colours this app wears when it is drawing light. Nothing else of
    /// that direction comes with them.
    static let lightKey = "theme-light"
    /// The same for the dark side.
    static let darkKey = "theme-dark"
    /// system, light or dark — which side is drawn, independently of where either side came from.
    static let appearanceKey = "appearance"

    /// Where the choices persist, said in the form somebody can actually go and look at.
    ///
    /// `UserDefaults` works without a bundle — probed, not assumed: a non-bundled executable gets
    /// a domain named after the executable, and `~/Library/Preferences/chat-search.plist` is
    /// written and read back across runs. The catch worth writing down is that the domain is the
    /// *executable name*, so the day this app gains a bundle identifier the preferences move and
    /// the old ones are orphaned rather than migrated. That costs one re-pick, which is why it did
    /// not buy a hand-rolled file under `~/.config/chat-search/` — that directory is `cs`'s, this
    /// is the client's own state, and a plist is inspectable with `defaults read` where a new
    /// dotfile format would be inspectable with nothing.
    static let location = "~/Library/Preferences/chat-search.plist"

    /// What a command line asked for, kept as typed.
    ///
    /// Every field is optional and `nil` means *did not say*, which is not the same as anything it
    /// could have said — that difference is the whole reason `Appearance` has a `system` case
    /// rather than being absent when nothing is forced.
    struct Request {
        var direction: String?
        var light: String?
        var dark: String?
        var appearance: String?

        var isEmpty: Bool {
            direction == nil && light == nil && dark == nil && appearance == nil
        }
    }

    /// What a launch settled on, kept in its parts.
    ///
    /// The parts and not only the answer, because `Theme.composed` is one-way: once three
    /// directions have become "terminal with paper's light" there is no taking that apart again,
    /// and the settings window (`chat-search-me9.8.21`) has a control standing for three of these
    /// four fields. `theme` is derived rather than stored, so what is drawn and what the controls
    /// say cannot drift.
    struct Choice {
        /// The direction whose *light* colours the light side wears.
        var light: Theme
        /// The direction whose *dark* colours the dark side wears.
        var dark: Theme
        /// The direction the type scale and the geometry come from, for both sides. Colour is the
        /// only thing that travels with a side (`chat-search-me9.8.22`), so this is a fourth
        /// setting rather than an aspect of the other two, and it belongs to `--theme`.
        var layout: Theme
        /// Which side is drawn, whatever macOS is doing.
        var appearance: Appearance

        var theme: Theme { .composed(light: light, dark: dark, layout: layout) }
    }

    /// The theme to draw, the appearance to draw it in, and the complaints that go with both.
    ///
    /// Order for each setting: what was asked for on the command line, then what was chosen last
    /// time, then what the build ships. A name this build does not carry never silently becomes
    /// another one — the flag that did nothing is the failure mode this whole function exists to
    /// avoid — so it says so and leaves the previous answer standing.
    ///
    /// - Parameter remember: false for a scripted run. `--measure` and `--shot` take a picture or a
    ///   number in whatever direction and appearance the script names, and neither is somebody
    ///   choosing a theme; the same reason both stay out of the query log (`docs/DECISIONS.md`
    ///   ADR 22). It gates every write here, and there are now four of them.
    static func resolve(
        _ asked: Request,
        remember: Bool,
        say: (String) -> Void = { FileHandle.standardError.write(Data(($0 + "\n").utf8)) }
    ) -> Choice {
        let store = UserDefaults.standard
        // Read before anything is written, because writing is one of the things that changes it: a
        // `theme` with nothing beside it is a preference written by a build that had one axis.
        let oldShaped =
            store.string(forKey: directionKey) != nil
            && store.string(forKey: lightKey) == nil
            && store.string(forKey: darkKey) == nil
            && store.string(forKey: appearanceKey) == nil

        // The direction. Both sides' colours unless a side is overridden, and both sides' type and
        // geometry whatever happens.
        var direction = Theme.shipped
        var directionNamed = false
        if let name = asked.direction, let theme = carried(name, say) {
            direction = theme
            directionNamed = true
            if remember {
                store.set(theme.name, forKey: directionKey)
                // `--theme` means "this direction on both sides", so it clears the two side
                // overrides rather than leaving one standing to contradict what it just said.
                store.removeObject(forKey: lightKey)
                store.removeObject(forKey: darkKey)
                say(
                    "theme: \(theme.name) on both sides — remembered in \(location). "
                        + "`defaults delete chat-search \(directionKey)` forgets it.")
            }
        }
        if !directionNamed,
            let theme = remembered(directionKey, instead: "drawing \(Theme.shipped.name)", say)
        {
            direction = theme
        }

        // Each side, colour only. A remembered override is ignored when this command line named a
        // direction, for the same reason writing one clears it.
        var light = direction
        var dark = direction
        if !directionNamed {
            let instead = "that side keeps \(direction.name)'s"
            if let theme = remembered(lightKey, instead: instead, say) { light = theme }
            if let theme = remembered(darkKey, instead: instead, say) { dark = theme }
        }
        if let name = asked.light, let theme = carried(name, say) {
            light = theme
            if remember { keep(theme, forKey: lightKey, side: "light", store: store, say: say) }
        }
        if let name = asked.dark, let theme = carried(name, say) {
            dark = theme
            if remember { keep(theme, forKey: darkKey, side: "dark", store: store, say: say) }
        }

        // The appearance, which is not a direction's to have an opinion about.
        var appearance = Appearance.system
        if let name = asked.appearance, let value = Appearance(rawValue: name) {
            appearance = value
            if remember {
                store.set(value.rawValue, forKey: appearanceKey)
                say(
                    "appearance: \(value.rawValue) — remembered in \(location). "
                        + "`defaults delete chat-search \(appearanceKey)` forgets it.")
            }
        } else {
            if let name = asked.appearance {
                say("no appearance called \(name). This flag takes \(Appearance.names).")
            }
            if let name = store.string(forKey: appearanceKey) {
                if let value = Appearance(rawValue: name) {
                    appearance = value
                } else {
                    say(
                        "the remembered appearance \(name) is not one this build knows; following "
                            + "the system. It is still in \(location).")
                }
            }
        }

        // An old-shaped preference is READ rather than converted. A `theme` written by
        // `chat-search-me9.8.9` meant "this direction on both sides, following the system", which
        // is exactly what it still means here — so there is nothing to convert, and nothing is
        // rewritten, because a launch that quietly writes a preference nobody asked for is what the
        // scripted-run rule above exists to prevent. What is owed is the sentence: the same value
        // now has two more axes beside it, and silence would leave them to be discovered.
        if oldShaped && asked.isEmpty {
            say(
                "theme: \(direction.name) on both sides, appearance following the system — that is "
                    + "what a `theme` set before this build means. `--theme-light NAME` and "
                    + "`--theme-dark NAME` set a direction per side, `--appearance "
                    + "\(Appearance.names)` overrides the system, and `--appearance system` records "
                    + "this reading so this line stops.")
        }

        return Choice(light: light, dark: dark, layout: direction, appearance: appearance)
    }

    // MARK: - Writing the same four keys from somewhere that is not a flag

    /// Two side menus naming one direction, or nil when they disagree.
    ///
    /// The settings window has three controls and there are four settings, so something has to say
    /// what the two side menus mean for the fourth. They mean `--theme NAME`: both menus on one
    /// direction is that direction *whole*, which is the only way the window reaches a direction's
    /// type scale and geometry at all. The alternative — side keys always, layout never — is a
    /// window where choosing `paper` twice draws paper's colours in `terminal`'s metrics while
    /// `--theme paper` draws the serif face, so the same choice said two ways gives two results.
    ///
    /// Menus that disagree are `--theme-light` and `--theme-dark`: colour per side, and the layout
    /// direction stays exactly as it was, because nothing on screen asked to change it.
    static func whole(_ light: Theme, _ dark: Theme) -> Theme? {
        light.name == dark.name ? light : nil
    }

    /// Write down which direction each side's colours come from, in the keys the flags write.
    ///
    /// No sentence on stderr, where every write above has one. The flags talk because a flag can
    /// silently do nothing and the failure looks exactly like success; a menu that redraws the app
    /// under the window has already said it, to somebody who is looking.
    static func remember(light: Theme, dark: Theme, store: UserDefaults = .standard) {
        if let whole = whole(light, dark) {
            store.set(whole.name, forKey: directionKey)
            // Cleared for the reason `--theme` clears them: "this direction on both sides" and a
            // leftover side override are two answers to one question.
            store.removeObject(forKey: lightKey)
            store.removeObject(forKey: darkKey)
        } else {
            store.set(light.name, forKey: lightKey)
            store.set(dark.name, forKey: darkKey)
        }
    }

    /// The same for which side is drawn.
    static func remember(appearance: Appearance, store: UserDefaults = .standard) {
        store.set(appearance.rawValue, forKey: appearanceKey)
    }

    /// A direction this build carries, or nil having said which ones it does.
    private static func carried(_ name: String, _ say: (String) -> Void) -> Theme? {
        guard let theme = Theme.direction(named: name) else {
            say(
                "no direction called \(name). This build carries "
                    + Theme.directionNames.joined(separator: ", ") + ".")
            return nil
        }
        return theme
    }

    /// A remembered direction, or nil having said what became of it.
    ///
    /// Silence about a name that is no longer compiled in would read as the preference having been
    /// forgotten, and it has not been — it is still in the plist, and it comes back the moment that
    /// direction is generated in again.
    private static func remembered(
        _ key: String, instead: String, _ say: (String) -> Void
    ) -> Theme? {
        guard let name = UserDefaults.standard.string(forKey: key) else { return nil }
        guard let theme = Theme.direction(named: name) else {
            say(
                "the remembered \(key) \(name) is not in this build; \(instead). "
                    + "It is still in \(location).")
            return nil
        }
        return theme
    }

    /// One side's override, written down and said out loud — including what it does *not* bring
    /// with it, which is the part somebody picking `paper` for one side will not expect.
    private static func keep(
        _ theme: Theme, forKey key: String, side: String, store: UserDefaults,
        say: (String) -> Void
    ) {
        store.set(theme.name, forKey: key)
        say(
            "\(key): \(theme.name)'s \(side) colours, and only its colours — remembered in "
                + "\(location). `defaults delete chat-search \(key)` forgets it.")
    }
}
