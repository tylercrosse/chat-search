import CsTheme
import Foundation

/// Which of the compiled-in directions this launch draws, and the one thing this app remembers
/// between launches.
///
/// The seam under it needed nothing to make this possible — `Theme` is a plain `Sendable` value
/// behind an environment key, so a second direction is an override at the root and no view change
/// (`chat-search-me9.8.8`). What had to be decided is where the choice comes from and where it
/// goes, and both answers are here rather than spread between a flag and a view.
///
/// ## Why a flag is the affordance
///
/// This app has no bundle, no Dock icon and no menu bar: it is launched by
/// `swift run -c release chat-search`, so a terminal *is* its front door and a flag is the
/// idiomatic control for it. `--theme` is not in the family of `--size`, `--group` and `--measure`
/// for that reason — those are instruments, deliberately not preferences, and this one is a
/// preference and sticks. An in-app picker is what this wants when the app grows a menu bar to put
/// it in; `chat-search-me9.8.21`.
enum ThemeChoice {
    /// The key, in this executable's own defaults domain.
    static let key = "theme"

    /// Where the choice persists, said in the form somebody can actually go and look at.
    ///
    /// `UserDefaults` works without a bundle — probed, not assumed: a non-bundled executable gets
    /// a domain named after the executable, and `~/Library/Preferences/chat-search.plist` is
    /// written and read back across runs. The catch worth writing down is that the domain is the
    /// *executable name*, so the day this app gains a bundle identifier the preference moves and
    /// the old one is orphaned rather than migrated. That costs one re-pick of a theme, which is
    /// why it did not buy a hand-rolled file under `~/.config/chat-search/` — that directory is
    /// `cs`'s, this is the client's own state, and a plist is inspectable with `defaults read`
    /// where a new dotfile format would be inspectable with nothing.
    static let location = "~/Library/Preferences/chat-search.plist"

    /// The direction to draw, and the complaints that go with it.
    ///
    /// Order: what was asked for on the command line, then what was chosen last time, then the
    /// direction the build ships. A name this build does not carry never silently becomes another
    /// one — the flag that did nothing is the failure mode this whole function exists to avoid —
    /// so it says so and leaves the previous answer standing.
    ///
    /// - Parameter remember: false for a scripted run. `--measure` and `--shot` take a picture or
    ///   a number in whatever direction the script names, and neither is somebody choosing a
    ///   theme; the same reason both stay out of the query log (`docs/DECISIONS.md` ADR 22).
    static func resolve(
        named requested: String?,
        remember: Bool,
        say: (String) -> Void = { FileHandle.standardError.write(Data(($0 + "\n").utf8)) }
    ) -> Theme {
        if let requested {
            if let theme = Theme.direction(named: requested) {
                if remember {
                    UserDefaults.standard.set(theme.name, forKey: key)
                    say(
                        "theme: \(theme.name) — remembered in \(location). "
                            + "`defaults delete chat-search \(key)` forgets it.")
                }
                return theme
            }
            say(
                "no direction called \(requested). This build carries "
                    + Theme.directionNames.joined(separator: ", ") + ".")
        }

        guard let remembered = UserDefaults.standard.string(forKey: key) else {
            return .shipped
        }
        guard let theme = Theme.direction(named: remembered) else {
            // A build that dropped a direction somebody had chosen. Silence here would read as the
            // preference having been forgotten, and it has not been — it is still in the plist,
            // and it will come back the moment that direction is generated in again.
            say(
                "the remembered theme \(remembered) is not in this build; drawing "
                    + "\(Theme.shipped.name). It is still in \(location).")
            return .shipped
        }
        return theme
    }
}
