import AppKit
import CsKit
import CsTheme
import Foundation
import SwiftUI

// A plain executable, not an app bundle. `NSApplication` is created by hand, the same way the
// spike does it, so this runs out of a checkout with `swift run` and nothing else — no Xcode
// project, no asset catalog, no signing. When something here needs a bundle (a Dock icon, a
// login item, a URL scheme) that is the moment to add one, and not before.

struct Options {
    /// All three override what `cs` would resolve for itself, which is how this gets exercised
    /// without touching the real archive. See AGENTS.md.
    var db: URL?
    var config: URL?
    var binary: String?
    var limit = 60
    /// Type the measurement phrases into the real interface and print keystroke→frame, then quit.
    /// Not a user affordance: it is the only way to take that number on what ships rather than on
    /// the instrument that first took it.
    var measure = false
    var interval = Duration.milliseconds(100)
    /// Re-measure every compiled-in direction and exit. Also not a user affordance: it is the gate,
    /// and it is a flag rather than a test because the Command Line Tools SDK has no test
    /// framework in it.
    var verifyTheme = false
    /// The four that *are* preferences — they stick, where `--size` and `--group` deliberately do
    /// not. A direction on both sides, a direction's colours on one side, and which side is drawn.
    /// Kept as typed, because turning a name into a theme happens once, in `ThemeChoice`.
    var theme: String?
    var themeLight: String?
    var themeDark: String?
    var appearance: String?
    /// A token set to read off disk instead of drawing a direction (`chat-search-me9.8.10`). Not a
    /// preference and not remembered: the file is the memory, and it is there or it is not. `nil`
    /// is `ThemeFile.standardLocation` — which is a different thing from having named that path,
    /// because a file that is absent where a flag pointed is a flag that did nothing.
    var themeFile: URL?
    /// Draw a direction for this run with the file left where it is. The way to see what the app
    /// ships beside what you are dialling, without moving anything.
    var noThemeFile = false
    /// Write the theme this launch would draw to the theme file's path and exit. The fifth
    /// non-affordance, and the answer to what would otherwise be an hour of typing: a user theme
    /// has to be complete (ADR 25), so dialling one in starts from a direction rather than a blank
    /// page. It writes values `poc/ui/palette.py` solved and this build already carries.
    var writeTheme = false
    /// Search, open the first result and draw the window to a PNG, then quit. The third
    /// non-affordance, and the only way to check the reader from a script: what it draws is a
    /// picture, and no exit code describes one.
    var shot = false
    var shotQuery = "borrow checker"
    var shotPath = "/tmp/chat-search.png"
    /// Open the longest conversation the query returned rather than the best one. The minimap's
    /// hard case is length — `chat-search-me9.8.18` — and no query puts the corpus's longest
    /// conversation first, so without this it cannot be reached from a script at all.
    var longest = false
    /// The window's opening size. A verification affordance in the same family as the three
    /// above: the row's column plan has to hold at several widths (`chat-search-me9.8.2`), and a
    /// window that always opens at one size makes checking that a manual drag nobody repeats.
    var size = CGSize(width: 900, height: 620)
    /// Which axis the list opens grouped by. The same kind of affordance as `--size`: a grouped
    /// list rebuilds sections on every keystroke where an ungrouped one rebuilds rows, and
    /// `--measure` cannot take that number on a mode it has no way to enter.
    var group = Grouping.none
    /// Whether the bottom drawer opens shut. Not a preference and not remembered — the same
    /// reading as `--group` and `--folded`: it is an instrument affordance, because the drawer is
    /// a third `cs` per keystroke and `--measure` has to be able to take the number both ways.
    var timeline = true
    /// Whether the list opens with every group folded. The same kind of affordance as `--group`,
    /// and needed for the same reason: a folded list draws heads where an open one draws heads and
    /// rows, so it is a different render and a different picture, and neither `--measure` nor
    /// `--shot` can reach it by typing. Not a preference — it is not remembered, and the fold a
    /// person performs by hand is the one that counts.
    var folded = false
    /// Whether the settings window is up when the app comes up. The same kind of affordance as
    /// `--folded` and needed for a sharper version of the same reason: the window is reached by
    /// Cmd-comma, which is a key no script can press, and it is a second window — so `--shot`
    /// cannot photograph it and driving its controls cannot be checked without a way in.
    var settings = false
    /// Press the Edit menu's keys in the search field and report what each moved. The fourth
    /// non-affordance, and a mode of its own rather than a section of `--shot` for a reason that is
    /// the subject of the check: a key equivalent is delivered to the key window's first responder,
    /// an inactive application has no key window, so this is the one scripted run that has to be
    /// frontmost. `chat-search-me9.8.24`.
    var clipboard = false

    /// Nobody is typing into this run. Every scripted mode stays out of the query log, because its
    /// queries are a benchmark's rather than a need somebody had, and out of the four preference
    /// keys. See `CsClient.driven`.
    ///
    /// It now also decides that a run reads no theme file unless a flag names one, for the reason
    /// it decides the rest: `--shot` draws what the script named it, and a frame that changed
    /// because of a file in somebody's home directory is a frame of that home directory.
    /// `--write-theme` is here because it must not remember whatever it was told to write out.
    var scripted: Bool { measure || shot || clipboard || writeTheme }

    /// ...and all but one stay out of the *front*, so they can be run beside whatever a person is
    /// actually doing, and so a latency taken here is comparable with `poc/swift/RESULTS.md` §1,
    /// which was taken the same way. `--clipboard` is the exception because a background app's menu
    /// bar delivers nothing — the thing it would be checking is the thing it would be missing.
    var takesFront: Bool { clipboard }
}

func parse(_ argv: [String]) -> Options {
    var o = Options()
    var i = 0
    while i < argv.count {
        let a = argv[i]
        func next() -> String? {
            i += 1
            return i < argv.count ? argv[i] : nil
        }
        switch a {
        case "--db": if let v = next() { o.db = URL(fileURLWithPath: v) }
        case "--config": if let v = next() { o.config = URL(fileURLWithPath: v) }
        case "--bin": o.binary = next()
        case "--limit": if let v = next(), let n = Int(v) { o.limit = n }
        case "--interval": if let v = next(), let n = Int(v) { o.interval = .milliseconds(n) }
        case "--measure": o.measure = true
        case "--size": if let v = next(), let size = parseSize(v) { o.size = size }
        // An axis this build has no name for leaves the list ungrouped rather than guessing at
        // one, for the reason `--size` ignores a malformed value: an instrument that quietly
        // changes what it is measuring is worse than one that ignores you.
        case "--group": if let v = next(), let axis = Grouping(rawValue: v) { o.group = axis }
        case "--folded": o.folded = true
        case "--no-timeline": o.timeline = false
        case "--settings": o.settings = true
        case "--clipboard": o.clipboard = true
        case "--verify-theme": o.verifyTheme = true
        // Kept as typed rather than resolved here: a name this build does not carry gets a
        // sentence on stderr from `ThemeChoice`, which is also where the remembered one is read,
        // so there is one place that turns a name into a theme and one place that complains. The
        // same for an appearance nobody has a reading for.
        case "--theme": o.theme = next()
        case "--theme-light": o.themeLight = next()
        case "--theme-dark": o.themeDark = next()
        case "--appearance": o.appearance = next()
        case "--theme-file": if let v = next() { o.themeFile = URL(fileURLWithPath: v) }
        case "--no-theme-file": o.noThemeFile = true
        case "--write-theme": o.writeTheme = true
        case "--shot": o.shot = true
        case "--query": if let v = next() { o.shotQuery = v }
        case "--out": if let v = next() { o.shotPath = v }
        case "--longest": o.longest = true
        case "--help", "-h":
            let names = Theme.directionNames.joined(separator: "|")
            print("""
                chat-search [--db PATH] [--config PATH] [--bin PATH] [--limit N]

                  --theme NAME         draw this direction on both sides, and remember it: \(names)
                                       remembered in \(ThemeChoice.location)
                  --theme-light NAME   take the light side's colours from this direction instead
                  --theme-dark NAME    the same for the dark side. Colours only: the type scale
                                       and the row metrics come from --theme, for both sides
                  --appearance WHICH   \(Appearance.names) — which side is drawn, whatever
                                       macOS is doing. Also remembered
                  --theme-file PATH    draw the token set in this file instead of a direction.
                                       Default \(ThemeFile.standardLocation.path)
                                       when it is there. Measured on load and drawn either way
                  --no-theme-file      ignore that file for this run and draw a direction
                  --write-theme        write the theme this launch would draw to the theme
                                       file's path, as the file --theme-file reads, and exit
                  --settings           open the settings window at launch. All three settings
                                       are on it, and it is Cmd-comma the rest of the time
                  --clipboard          press the Edit menu's keys in the search field and print
                                       what each one moved. The only run that takes the front
                  --measure            type the measurement phrases and print keystroke→frame
                  --interval MS        milliseconds between simulated keystrokes (default 100)
                  --verify-theme       re-measure every compiled-in direction and exit. With
                                       --theme-file, measure that file instead
                  --shot               search, open the first result, write the window to --out
                  --query TEXT         what --shot searches for (default "borrow checker")
                  --out PATH           where --shot writes its PNG (default /tmp/chat-search.png)
                  --longest            --shot opens the longest conversation, not the best
                  --size WxH           open the window at this size (default 900x620)
                  --group AXIS         open grouped by none|project|run|source (default none).
                                       Cmd-1 to Cmd-4 are the same four while it is running
                  --folded             open with every group folded (needs --group)
                  --no-timeline        open with the bottom drawer shut, so a keystroke spawns
                                       two processes rather than three. Cmd-T is the same
                                       switch while the app is running
                """)
            exit(0)
        default: break
        }
        i += 1
    }
    return o
}

/// Sentences on stderr, where every other complaint about a theme goes (`ThemeChoice.resolve`).
func complain(_ sentences: [String]) {
    FileHandle.standardError.write(Data(sentences.map { $0 + "\n" }.joined().utf8))
}

/// A direction, written out as the file `--theme-file` reads back.
///
/// It refuses to overwrite, and that is the whole of its policy. A theme file is somebody's
/// afternoon of nudging values, `--write-theme` is the first thing they will run again when they
/// want to start over, and a flag that silently replaces the work is worse than one that makes you
/// move it yourself. `--theme-file` names another path when the answer is "beside it, not over it".
func writeTheme(_ theme: Theme, to url: URL) -> Int32 {
    guard !FileManager.default.fileExists(atPath: url.path) else {
        complain([
            "there is already a file at \(url.path). Move it, or name another path with "
                + "`--theme-file PATH` — this never writes over one."
        ])
        return 2
    }
    do {
        try FileManager.default.createDirectory(
            at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
        try ThemeFile.text(for: theme).write(to: url, atomically: true, encoding: .utf8)
    } catch {
        complain(["could not write \(url.path): \(error.localizedDescription)"])
        return 2
    }
    print(
        "\(url.path) — \(theme.name)'s tokens, as a \(ThemeClass.userTheme.label). Edit and "
            + "relaunch; it is measured on load and drawn either way (docs/DECISIONS.md ADR 25).")
    return 0
}

/// `WxH`, or nil for anything else. A malformed value leaves the default standing rather than
/// opening a window somebody has to go and find: this is an instrument, and an instrument that
/// silently changes the thing being measured is worse than one that ignores you.
func parseSize(_ text: String) -> CGSize? {
    let parts = text.lowercased().split(separator: "x")
    guard parts.count == 2, let w = Double(parts[0]), let h = Double(parts[1]), w > 0, h > 0
    else { return nil }
    return CGSize(width: w, height: h)
}

let options = parse(Array(CommandLine.arguments.dropFirst()))

// Before `cs` is looked for, and before there is a window: the tokens are checkable without an
// index, and a gate that needed a built binary and a live archive would be a gate people skip.
//
// Every direction and not `.shipped`, because any of them can be chosen at launch and a picker
// that offers an unmeasured palette is a fence with a gate left open. `as: .direction` is named
// rather than defaulted, here and everywhere else: what is compiled into this binary is what the
// project ships, and shipping is what makes these measurements binding (docs/DECISIONS.md ADR 25).
// A token set that arrived some other way is a different promise.
//
// Named a file, and it is the file being asked about instead. The gate stays what it was — every
// direction, and nothing outside the binary — because a gate that read `$HOME` would pass or fail
// on whose machine it ran. `--theme-file X --verify-theme` is somebody checking a candidate before
// they load it, and it exits on the readings and never on the policy (`ThemeCheck.run`): 1 for a
// token set that missed, 2 for a file that is not a token set at all.
if options.verifyTheme {
    guard let named = options.themeFile else {
        exit(ThemeCheck.run(Theme.directions, as: .direction))
    }
    do {
        exit(ThemeCheck.run(try ThemeFile.load(named), as: .userTheme))
    } catch {
        complain((error as? ThemeFile.Unreadable)?.sentences ?? ["\(named.path): \(error)"])
        exit(2)
    }
}

// Resolved once, here, and handed down. Not read from the environment's default by each view: the
// default is what the build ships and this is what the person chose, and only one of those two is
// allowed to be the answer once a choice exists. Two answers now — a theme, which may be one
// direction's light beside another's dark, and the appearance it is drawn in.
//
// Resolved once and then *held*, where it used to be resolved once and then fixed: the settings
// window moves these while the app is running (`chat-search-me9.8.21`), so what the launch settled
// on is this object's starting state rather than the last word.
let settings = ThemeSettings(
    ThemeChoice.resolve(
        ThemeChoice.Request(
            direction: options.theme, light: options.themeLight, dark: options.themeDark,
            appearance: options.appearance,
            file: options.themeFile ?? ThemeFile.standardLocation,
            fileNamed: options.themeFile != nil,
            // The run that is about to write a theme file does not first read the one it is about
            // to overwrite, which would make `--write-theme` a copy of itself.
            ignoreFile: options.noThemeFile || options.writeTheme),
        remember: !options.scripted),
    remembers: !options.scripted)

// After the theme is resolved, because what this writes is what this launch would have drawn —
// `--theme paper --write-theme` is how you start from paper. Before `cs` is looked for, with
// `--verify-theme`, because neither needs an index.
if options.writeTheme {
    exit(writeTheme(settings.theme, to: options.themeFile ?? ThemeFile.standardLocation))
}

guard let binary = CsClient.locate(binary: options.binary) else {
    FileHandle.standardError.write(
        Data("cs not found. Build it (`cargo build --release`) or set CS_BIN.\n".utf8))
    exit(2)
}

/// Owns the windows and, under `--measure`, the display link. None of them exists under
/// `swift run` unless something makes them.
@MainActor
final class AppHost: NSObject, NSApplicationDelegate, NSWindowDelegate, NSMenuItemValidation {
    let model: SearchModel
    let options: Options
    /// The direction and the side in force, injected at the root rather than reached for in a
    /// view. One override is the whole of what several themes in one binary costs the views, which
    /// is what the token seam was built before the views to buy (`chat-search-me9.8.8`).
    let settings: ThemeSettings
    private var window: NSWindow?
    /// Built the first time Cmd-comma is pressed and kept afterwards, so the window comes back
    /// where it was left rather than centred again.
    private var settingsWindow: NSWindow?
    private var frames: FrameClock?

    init(model: SearchModel, options: Options, settings: ThemeSettings) {
        self.model = model
        self.options = options
        self.settings = settings
    }

    func applicationDidFinishLaunching(_ note: Notification) {
        // Before the window, so nothing is drawn in one appearance and then re-drawn in another.
        // `nil` for `system` is not a no-op worth skipping: it is what says this app has no opinion
        // when nobody has expressed one, and it is what an override is cleared back to.
        NSApp.appearance = settings.appearance.nsAppearance
        // The two surfaces SwiftUI does not own, kept in step with the ones it does.
        settings.onChange = { [weak self] in self?.themeChanged() }
        // A hand-made `NSApplication` has no menu bar at all, so this is what makes Cmd-comma and
        // Cmd-Q into keys that do something. Installed in every run rather than only in the ones a
        // person is looking at: an accessory app does not draw a menu bar, which makes this inert
        // under `--shot` and `--measure` rather than a second behaviour to reason about.
        NSApp.mainMenu = MainMenu.build(
            target: self, settings: #selector(showSettings(_:)),
            timeline: #selector(toggleTimeline(_:)), group: #selector(groupBy(_:)))
        model.group(by: options.group)
        // After the axis, because folding is a property of the groups that axis produced. It moves
        // the default rather than folding what is on screen now, so a group that arrives on a later
        // keystroke is folded too — otherwise a scripted run would measure a list that unfolded
        // itself as it typed.
        if options.folded { model.foldAll(true) }
        let window = NSWindow(
            contentRect: NSRect(origin: .zero, size: options.size),
            styleMask: [.titled, .closable, .miniaturizable, .resizable],
            backing: .buffered, defer: false)
        window.title = "chat-search"
        // The one colour SwiftUI does not own. It shows through for a frame during a live resize,
        // and the system default is a grey nobody in this app chose.
        window.backgroundColor = settings.theme.nsColor(.bg)
        window.center()
        // The delegate is for one thing: this window closing takes the settings window with it.
        // `applicationShouldTerminateAfterLastWindowClosed` is what makes closing the window quit
        // the app, and a settings panel left open behind it would quietly turn one close into two.
        window.delegate = self
        let hosting = NSHostingView(rootView: Root(model: model, settings: settings))
        window.contentView = hosting
        window.orderFrontRegardless()
        self.window = window
        // A scripted run does not steal focus — `.accessory` above — which also keeps the number
        // comparable with `poc/swift/RESULTS.md` §1, taken the same way. A latency measured in a
        // frontmost app and one measured in a background app are not the same measurement.
        //
        // `--clipboard` is the exception and it is the check itself that forces it: a key
        // equivalent goes to the key window's first responder and an inactive app has no key
        // window, so a pass that pressed Cmd-C from the background would be reporting on the
        // background rather than on the menu (`chat-search-me9.8.24`).
        if !options.scripted || options.takesFront { NSApp.activate(ignoringOtherApps: true) }

        // Taken off the hosting view rather than off the window, because the window's own chrome
        // following an override says nothing about whether the colours did (`AppearanceProbe`).
        // With the scripted runs, where the rest of the evidence is: a person looking at the
        // window can already see which side they got.
        if options.scripted {
            print(
                AppearanceProbe.line(
                    theme: settings.theme, view: hosting, asked: settings.appearance))
        }

        // The settings window's own pass, and it replaces the reader's rather than following it:
        // what this run has to show is three controls and what moving them does to the app behind
        // them, and twenty seconds of scrolling a transcript first says nothing about any of it.
        // It opens the window by pressing Cmd-comma rather than being handed one, which is why
        // this run is the only one that does not open it here.
        if options.shot, options.settings {
            Task { @MainActor in
                await Measure.settings(
                    model: model, settings: settings, view: hosting, query: options.shotQuery,
                    to: options.shotPath)
                NSApp.terminate(nil)
            }
            return
        }
        if options.settings { showSettings(nil) }

        // Before `--shot`'s pass and after nothing, because this one wants the box empty and the
        // drawer shut, and it opens the drawer itself when it gets to the transcript's half.
        if options.clipboard {
            Task { @MainActor in
                await Measure.clipboard(model: model, window: window, query: options.shotQuery)
                NSApp.terminate(nil)
            }
            return
        }

        if options.shot {
            // A display link under `--shot` as well as under `--measure`: the drawer is driven
            // through a scroll here, and whether that stutters is a number rather than a picture.
            // `model.frames` is deliberately left alone — that one stamps keystrokes, which this
            // run does not make.
            let frames = FrameClock()
            frames.attach(to: hosting)
            self.frames = frames
            Task { @MainActor in
                await Measure.shot(
                    model: model, window: window, query: options.shotQuery, to: options.shotPath,
                    longest: options.longest, frames: frames)
                NSApp.terminate(nil)
            }
            return
        }

        guard options.measure else { return }
        let frames = FrameClock()
        frames.attach(to: hosting)
        self.frames = frames
        model.frames = frames
        Task { @MainActor in
            await Measure.typing(model: model, frames: frames, interval: options.interval)
            NSApp.terminate(nil)
        }
    }

    /// Cmd-comma, and the only thing on the menu that is this app's rather than AppKit's.
    ///
    /// A window and not a sheet on the main one: three settings are a place you go and come back
    /// from, and a sheet would cover the app it is previewing, which is the whole of the preview.
    @objc func showSettings(_ sender: Any?) {
        if let settingsWindow {
            settingsWindow.makeKeyAndOrderFront(sender)
            return
        }
        // `NSWindow(contentViewController:)` rather than a content view and a size: SwiftUI knows
        // how tall three controls and two captions are and this is the one way to ask it, which
        // matters more here than anywhere else in the app — every other window in this process
        // opens at a size somebody chose.
        let window = NSWindow(
            contentViewController: NSHostingController(rootView: SettingsForm(settings: settings)))
        window.styleMask = [.titled, .closable]
        window.title = "Settings"
        // A settings window is not the app's life. Without this, closing it once deallocates it and
        // the second Cmd-comma opens a window that has already been released.
        window.isReleasedWhenClosed = false
        window.center()
        window.makeKeyAndOrderFront(sender)
        settingsWindow = window
    }

    /// Cmd-T, and the keyboard's only way into the bottom drawer.
    ///
    /// It calls exactly what the drawer's own button calls, which is the point: the menu is a second
    /// route to one act and not a second implementation of it. What `--no-timeline` opens with, this
    /// key takes back, and neither touches the query box — so shutting the drawer still cannot
    /// change which conversations the list holds (`TimelineModel.toggle`).
    @objc func toggleTimeline(_ sender: Any?) { model.toggleTimeline() }

    /// Cmd-1 to Cmd-4, and the keyboard's only way onto an axis.
    ///
    /// The same method the chip calls, for the same reason Cmd-T is: two routes to one act, and the
    /// consequences of switching — the accordion clearing, the cursor being placed again — are
    /// `SearchModel.group(by:)`'s to have, not a menu's to reimplement. Which axis is on the item
    /// rather than in this method, so adding one to `Grouping` adds a key and not a branch.
    ///
    /// An axis already in force returns early inside the model, so the key is inert on the item
    /// that is already checked — which is what a click on the chip in force already did.
    @objc func groupBy(_ sender: NSMenuItem) {
        guard let axis = sender.representedObject as? Grouping else { return }
        model.group(by: axis)
    }

    /// The verb on the View item and the mark beside the axis, written at the moment AppKit asks
    /// whether the item is live.
    ///
    /// `MainMenu` holds no model and should not: which of *Hide* and *Show* is true right now, and
    /// which of four axes the list is cut along, are facts about this window, and validation is the
    /// one callback that runs both before the menu is drawn and before a key equivalent fires.
    /// Everything else on the bar goes to `nil` and up the responder chain, so this is asked only
    /// about the items aimed here by name — and all of them are always live, which is the answer
    /// that keeps them off the grey list `MainMenu` is built to avoid.
    ///
    /// The mark is a checkmark and the group is exclusive, which is what makes it the radio control
    /// the chips are: `.on` is written on the axis in force and `.off` on the other three every
    /// time, rather than moved from the one that had it. A mark left behind by a `--group` flag or
    /// by a click on a chip would be a menu disagreeing with the screen about a thing the screen is
    /// right about.
    func validateMenuItem(_ item: NSMenuItem) -> Bool {
        if item.action == #selector(toggleTimeline(_:)) {
            item.title = model.timeline.shown ? "Hide Timeline" : "Show Timeline"
        }
        if item.action == #selector(groupBy(_:)) {
            item.state = item.representedObject as? Grouping == model.grouping ? .on : .off
        }
        return true
    }

    /// What has to change outside SwiftUI when a control moves. Everything else is the environment.
    private func themeChanged() {
        NSApp.appearance = settings.appearance.nsAppearance
        window?.backgroundColor = settings.theme.nsColor(.bg)
    }

    /// The main window closing takes the settings window with it, so that closing the app's window
    /// still quits the app rather than leaving a settings panel holding the process open.
    func windowWillClose(_ note: Notification) {
        settingsWindow?.close()
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ app: NSApplication) -> Bool { true }

    /// The last thing this process does, and the only place §6's other event can be emitted.
    ///
    /// A window is what knows a query went unanswered, and it knows it right up to here: quit
    /// with something in the box and nothing opened, and the log gets one `Search`. Without it
    /// this app records only the searches that worked (`chat-search-pdw`).
    func applicationWillTerminate(_ note: Notification) {
        model.recordAbandonment()
    }
}

let app = NSApplication.shared
app.setActivationPolicy(options.scripted ? .accessory : .regular)
let host = AppHost(
    model: SearchModel(
        client: CsClient(
            binary: binary, db: options.db, config: options.config, driven: options.scripted),
        limit: options.limit, timeline: options.timeline),
    options: options,
    settings: settings)
app.delegate = host
app.run()
