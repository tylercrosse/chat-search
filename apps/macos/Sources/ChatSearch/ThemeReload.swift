import CsTheme
import Foundation

/// The second half of `chat-search-me9.8.10`'s seam: the file, drawn again when it changes.
///
/// `ThemeWatch` says *the file moved*; `ThemeSettings` holds what is drawn; this decides what a
/// move means and what gets said about it. Both of those are decisions rather than plumbing, which
/// is why they are here and not in either of them.
///
/// ## What is on screen does not change until a whole file loads
///
/// A save is not atomic in most editors, so a watch reads files that are empty, half a palette, or
/// a token list with one line still being typed — and `ThemeFile.load` refuses all three, because a
/// theme is drawn entire or not at all (`docs/DECISIONS.md` ADR 25 rule 3). The obvious thing to do
/// with a refusal is what a *launch* does with one: fall back to the shipped direction and say why.
/// Doing that here would flick the whole window back to a palette nobody is working on and then
/// forward again, on every save, which is worse than relaunching — and it cannot be avoided by
/// reading more carefully, because a file caught mid-write and a file with a typo in it are the
/// same file at the moment they are read.
///
/// So: **the last theme that loaded whole stays on screen until another one does.** Unreadable,
/// half-written, or deleted — what is drawn does not change, and the reasons are said. ADR 25 rule
/// 4 is unchanged and this is its other half: falling back to the direction is what to draw when
/// there is nothing else to draw, which at launch is always the case and mid-session never is. The
/// way back to a direction is `--no-theme-file` and a relaunch, which is also the way to look at
/// one beside what you are dialling.
///
/// ## The launch line is not said again on every save
///
/// The whole announcement — the name, the class, the path, the failures, what an unfenced theme
/// costs — is a thing to read once. On a reload the app has already said it, and the redraw under
/// the window says the rest to somebody who is looking (`ThemeChoice.remember` makes the same
/// argument about the settings window). So a reload says only what it did not say last time: a save
/// that clears the fence says nothing at all, a save that breaks something says what broke, and a
/// save that breaks it again says nothing, because it is the same sentence about the same file.
@MainActor
final class ThemeReload {
    private let settings: ThemeSettings
    private let url: URL
    private let say: (String) -> Void
    /// The sentences that describe the file as it stands, so that the next reading can say what
    /// changed rather than everything it found.
    private var said: [String]
    private var watch: ThemeWatch?

    /// Every sentence this has said, in order.
    ///
    /// Not private, for the reason `MarkedText.builds` is not: the claim is that a save says only
    /// what changed, and neither a picture nor an exit code carries it. `--watch-theme` reads this
    /// after each of the saves it makes.
    private(set) var spoken: [String] = []

    init(
        settings: ThemeSettings, url: URL,
        say: @escaping (String) -> Void = {
            FileHandle.standardError.write(Data(($0 + "\n").utf8))
        }
    ) {
        self.settings = settings
        self.url = url
        self.say = say
        said = Self.alreadySaid(settings, url)
    }

    /// What the launch already put on stderr about this file, so that the first save says what it
    /// changed rather than repeating what is on screen.
    ///
    /// Both of the two things a launch can have said. For a file it drew, the readings — taken
    /// again rather than carried through the resolved choice, because `ThemeCheck.measure` decides
    /// nothing and touches no file, so measuring twice costs less than a field that has to be kept
    /// true. For a file it could not, the reasons, which means opening a file that was opened a
    /// moment ago: one read, and it is what makes a broken file saved broken again say nothing.
    private static func alreadySaid(_ settings: ThemeSettings, _ url: URL) -> [String] {
        if let user = settings.user { return ThemeCheck.measure(user, as: .userTheme).failures }
        do {
            _ = try ThemeFile.load(url)
            return []
        } catch let unreadable as ThemeFile.Unreadable {
            return unreadable.sentences
        } catch {
            return []
        }
    }

    /// Begin following the file. Nothing before this point differs from `chat-search-me9.8.10`.
    func start() {
        let watch = ThemeWatch(url) { [weak self] change in self?.arrived(change) }
        watch.start()
        self.watch = watch
    }

    /// Stop following it, for a scripted pass that is about to take the file away.
    func stop() { watch?.stop() }

    private func arrived(_ change: Result<Theme, ThemeFile.Unreadable>) {
        switch change {
        case .success(let theme): draw(theme)
        case .failure(let unreadable): hold(unreadable)
        }
    }

    /// A whole file. This is the only thing that changes what is on screen.
    private func draw(_ theme: Theme) {
        let report = ThemeCheck.measure(theme, as: .userTheme)
        // Nothing has been said about a file the launch did not draw: it was written into place
        // while the app was up, or the launch read one that was not a theme. Whoever draws it
        // first says the sentence a launch says, and every save after that says only the change.
        let first = settings.user == nil
        settings.draw(user: theme, from: url)
        guard !first else {
            said = report.failures
            ThemeChoice.announce(theme, from: url, report, tell)
            return
        }
        report.failures.filter { !said.contains($0) }.forEach(tell)
        said = report.failures
    }

    /// A file that is not a theme, or is not there. What is drawn does not move.
    private func hold(_ unreadable: ThemeFile.Unreadable) {
        let fresh = unreadable.sentences.filter { !said.contains($0) }
        said = unreadable.sentences
        guard !fresh.isEmpty else { return }
        fresh.forEach(tell)
        guard let user = settings.user else {
            // The launch never got a theme out of this file either, so what is on screen is the
            // direction it already said it was drawing.
            tell("still drawing \(settings.theme.name).")
            return
        }
        tell(
            "still drawing \(user.name) — the last file that loaded whole stays on screen until "
                + "another one does. `--no-theme-file` and a relaunch draw a direction.")
    }

    private func tell(_ sentence: String) {
        spoken.append(sentence)
        say(sentence)
    }
}
