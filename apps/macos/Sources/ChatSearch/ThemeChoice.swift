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
/// ## And a fifth thing, which is not a setting
///
/// A token set read off a file (`chat-search-me9.8.10`) is not remembered anywhere, because the
/// file *is* the memory: it is there or it is not. When it is, it supersedes the three settings
/// above rather than joining them — whole or not at all, ADR 25 — and they keep resolving
/// underneath so that deleting the file draws exactly what was on screen before it arrived.
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

        /// Where to look for a token set off disk. Always a path — the standard location when no
        /// flag named one — because "there is no file there" is a thing this has to be able to say.
        var file: URL = ThemeFile.standardLocation
        /// Whether `--theme-file` named it. The difference decides whether an absent file is worth
        /// a sentence: at the standard location it is this app's ordinary state, and named on a
        /// command line it is a flag that did nothing.
        var fileNamed = false
        /// `--no-theme-file`, and the run that is about to write one. Both want the directions.
        var ignoreFile = false

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
        /// A token set read off a file somebody wrote, when there is one, and it supersedes all
        /// three fields above rather than joining them.
        ///
        /// Whole or not at all, which is `docs/DECISIONS.md` ADR 25 rule 3: a user theme is drawn
        /// as authored, entire, and never merged with a direction. So it cannot be a side — a user
        /// theme's light half beside a direction's dark half is exactly the palette nobody designed
        /// that the rule is about, and the mixing above stays closed under the fence because both
        /// of its halves are still directions.
        ///
        /// The three below it are resolved and remembered anyway, so removing the file draws
        /// exactly what was on screen before it arrived.
        var user: Theme?
        /// Which file that came out of, for the one surface that has to name it — the settings
        /// window, where the two side menus go quiet while it is in force and owe an explanation
        /// somebody can act on. The theme's own name is the file's stem and not its path.
        var userFile: URL?
        /// Which side is drawn, whatever macOS is doing.
        var appearance: Appearance

        var theme: Theme { user ?? .composed(light: light, dark: dark, layout: layout) }
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

        let user = userTheme(asked, instead: direction.name, scripted: !remember, say)
        return Choice(
            light: light, dark: dark, layout: direction, user: user,
            userFile: user == nil ? nil : asked.file, appearance: appearance)
    }

    // MARK: - The token set that is not compiled in

    /// A theme read off disk, measured and announced — or nil, having said why not.
    ///
    /// Resolved last, after the three directions above it, for one reason: a file that turns out
    /// not to be a theme owes the sentence "drawing terminal instead", and until the directions are
    /// settled there is nothing to name there.
    ///
    /// - Parameter scripted: `--measure`, `--shot` and `--clipboard`, which read *no* file unless a
    ///   flag named one. Same rule and the same reason as the four preferences they refuse to
    ///   write: a scripted run draws what the script named it, and a picture that changed because
    ///   of a file in somebody's home directory is a picture of that home directory. A flag naming
    ///   the file is the script naming it, so that case loads.
    private static func userTheme(
        _ asked: Request, instead direction: String, scripted: Bool, _ say: (String) -> Void
    ) -> Theme? {
        guard !asked.ignoreFile else { return nil }
        if scripted, !asked.fileNamed {
            if FileManager.default.isReadableFile(atPath: asked.file.path) {
                say(
                    "theme file: \(asked.file.path) is not loaded — a scripted run draws what its "
                        + "flags name, the same reason it writes none of the four preferences. "
                        + "`--theme-file PATH` is how a script asks for one.")
            }
            return nil
        }

        let theme: Theme
        do {
            theme = try ThemeFile.load(asked.file)
        } catch {
            guard let unreadable = error as? ThemeFile.Unreadable else { return nil }
            // An absent file at the standard location is this app's ordinary state and says
            // nothing. Every other way of not being a theme is said out loud, because the person
            // who wrote the file is the only one who can fix it and the only one who will look.
            if case .missing = unreadable, !asked.fileNamed { return nil }
            unreadable.sentences.forEach(say)
            say("drawing \(direction) instead.")
            return nil
        }

        // A file called `paper.css` beside a direction called `paper` is not an error and not a
        // collision — nothing resolves a user theme by name — but two things on one screen with
        // one name is worth one sentence.
        if Theme.direction(named: theme.name) != nil {
            say(
                "the theme file is called \(theme.name), which is also a direction this build "
                    + "carries. The file is what is drawn.")
        }

        // Always measured, by ThemeCheck and not by a second copy of these rules, and drawn
        // whatever the readings say (docs/DECISIONS.md ADR 25). The failures are whole sentences
        // on stderr rather than the table `--verify-theme` prints: nobody who has just launched an
        // app has a table in front of them, and there is no modal and no banner because the only
        // person who can be nagged here is the one who wrote the file.
        let report = ThemeCheck.measure(theme, as: .userTheme)
        say("theme: \(theme.name) · \(ThemeClass.userTheme.label), from \(asked.file.path).")
        guard !report.holds else {
            say("  It clears the same fence a shipped direction has to.")
            return theme
        }
        report.failures.forEach { say("  \($0)") }
        say(
            "  Drawn anyway, because you loaded it and it is your screen — what that costs is "
                + "docs/DECISIONS.md ADR 25. `--theme-file \(asked.file.path) --verify-theme` "
                + "prints the whole table.")
        return theme
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
