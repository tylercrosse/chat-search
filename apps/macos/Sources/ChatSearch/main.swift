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

    /// Nobody is typing into this run. Both scripted modes stay out of the front, so they can be
    /// run beside whatever a person is actually doing — and out of the query log, because their
    /// queries are a benchmark's rather than a need somebody had. See `CsClient.driven`.
    var scripted: Bool { measure || shot }
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
        case "--verify-theme": o.verifyTheme = true
        // Kept as typed rather than resolved here: a name this build does not carry gets a
        // sentence on stderr from `ThemeChoice`, which is also where the remembered one is read,
        // so there is one place that turns a name into a theme and one place that complains. The
        // same for an appearance nobody has a reading for.
        case "--theme": o.theme = next()
        case "--theme-light": o.themeLight = next()
        case "--theme-dark": o.themeDark = next()
        case "--appearance": o.appearance = next()
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
                  --measure            type the measurement phrases and print keystroke→frame
                  --interval MS        milliseconds between simulated keystrokes (default 100)
                  --verify-theme       re-measure every compiled-in direction and exit
                  --shot               search, open the first result, write the window to --out
                  --query TEXT         what --shot searches for (default "borrow checker")
                  --out PATH           where --shot writes its PNG (default /tmp/chat-search.png)
                  --longest            --shot opens the longest conversation, not the best
                  --size WxH           open the window at this size (default 900x620)
                  --group AXIS         open grouped by none|project|run|source (default none)
                  --folded             open with every group folded (needs --group)
                  --no-timeline        open with the bottom drawer shut, so a keystroke spawns
                                       two processes rather than three
                """)
            exit(0)
        default: break
        }
        i += 1
    }
    return o
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
if options.verifyTheme { exit(ThemeCheck.run(Theme.directions, as: .direction)) }

// Resolved once, here, and handed down. Not read from the environment's default by each view: the
// default is what the build ships and this is what the person chose, and only one of those two is
// allowed to be the answer once a choice exists. Two answers now — a theme, which may be one
// direction's light beside another's dark, and the appearance it is drawn in.
let (theme, appearance) = ThemeChoice.resolve(
    ThemeChoice.Request(
        direction: options.theme, light: options.themeLight, dark: options.themeDark,
        appearance: options.appearance),
    remember: !options.scripted)

guard let binary = CsClient.locate(binary: options.binary) else {
    FileHandle.standardError.write(
        Data("cs not found. Build it (`cargo build --release`) or set CS_BIN.\n".utf8))
    exit(2)
}

/// Owns the window and, under `--measure`, the display link. Neither exists under `swift run`
/// unless something makes them.
@MainActor
final class AppHost: NSObject, NSApplicationDelegate {
    let model: SearchModel
    let options: Options
    /// The direction in force, injected at the root rather than reached for in a view. One
    /// override is the whole of what several themes in one binary costs the views, which is what
    /// the token seam was built before the views to buy (`chat-search-me9.8.8`).
    let theme: Theme
    /// And which of its two sides is drawn, which costs the views nothing at all.
    let appearance: Appearance
    private var frames: FrameClock?

    init(model: SearchModel, options: Options, theme: Theme, appearance: Appearance) {
        self.model = model
        self.options = options
        self.theme = theme
        self.appearance = appearance
    }

    func applicationDidFinishLaunching(_ note: Notification) {
        // Before the window, so nothing is drawn in one appearance and then re-drawn in another.
        // `nil` for `system` is not a no-op worth skipping: it is what says this app has no opinion
        // when nobody has expressed one, and it is what an override is cleared back to.
        NSApp.appearance = appearance.nsAppearance
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
        window.backgroundColor = theme.nsColor(.bg)
        window.center()
        let hosting = NSHostingView(rootView: Shell(model: model).environment(\.theme, theme))
        window.contentView = hosting
        window.orderFrontRegardless()
        // A scripted run does not steal focus — `.accessory` above — which also keeps the number
        // comparable with `poc/swift/RESULTS.md` §1, taken the same way. A latency measured in a
        // frontmost app and one measured in a background app are not the same measurement.
        if !options.scripted { NSApp.activate(ignoringOtherApps: true) }

        // Taken off the hosting view rather than off the window, because the window's own chrome
        // following an override says nothing about whether the colours did (`AppearanceProbe`).
        // With the scripted runs, where the rest of the evidence is: a person looking at the
        // window can already see which side they got.
        if options.scripted {
            print(AppearanceProbe.line(theme: theme, view: hosting, asked: appearance))
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
    theme: theme,
    appearance: appearance)
app.delegate = host
app.run()
