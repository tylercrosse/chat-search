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
    /// Re-measure the shipped tokens and exit. Also not a user affordance: it is the gate, and it
    /// is a flag rather than a test because the Command Line Tools SDK has no test framework in it.
    var verifyTheme = false
    /// Search, open the first result and draw the window to a PNG, then quit. The third
    /// non-affordance, and the only way to check the reader from a script: what it draws is a
    /// picture, and no exit code describes one.
    var shot = false
    var shotQuery = "borrow checker"
    var shotPath = "/tmp/chat-search.png"
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
        case "--verify-theme": o.verifyTheme = true
        case "--shot": o.shot = true
        case "--query": if let v = next() { o.shotQuery = v }
        case "--out": if let v = next() { o.shotPath = v }
        case "--help", "-h":
            print("""
                chat-search [--db PATH] [--config PATH] [--bin PATH] [--limit N]

                  --measure            type the measurement phrases and print keystroke→frame
                  --interval MS        milliseconds between simulated keystrokes (default 100)
                  --verify-theme       re-measure the shipped tokens and exit
                  --shot               search, open the first result, write the window to --out
                  --query TEXT         what --shot searches for (default "borrow checker")
                  --out PATH           where --shot writes its PNG (default /tmp/chat-search.png)
                """)
            exit(0)
        default: break
        }
        i += 1
    }
    return o
}

let options = parse(Array(CommandLine.arguments.dropFirst()))

// Before `cs` is looked for, and before there is a window: the tokens are checkable without an
// index, and a gate that needed a built binary and a live archive would be a gate people skip.
if options.verifyTheme { exit(ThemeCheck.run(.shipped)) }

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
    private var frames: FrameClock?

    init(model: SearchModel, options: Options) {
        self.model = model
        self.options = options
    }

    func applicationDidFinishLaunching(_ note: Notification) {
        let window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 900, height: 620),
            styleMask: [.titled, .closable, .miniaturizable, .resizable],
            backing: .buffered, defer: false)
        window.title = "chat-search"
        // The one colour SwiftUI does not own. It shows through for a frame during a live resize,
        // and the system default is a grey nobody in this app chose.
        window.backgroundColor = Theme.shipped.nsColor(.bg)
        window.center()
        let hosting = NSHostingView(rootView: SearchView(model: model))
        window.contentView = hosting
        window.orderFrontRegardless()
        // A scripted run does not steal focus — `.accessory` above — which also keeps the number
        // comparable with `poc/swift/RESULTS.md` §1, taken the same way. A latency measured in a
        // frontmost app and one measured in a background app are not the same measurement.
        if !scripted { NSApp.activate(ignoringOtherApps: true) }

        if options.shot {
            Task { @MainActor in
                await Measure.shot(
                    model: model, window: window, query: options.shotQuery, to: options.shotPath)
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

    /// Nobody is typing into this run. Both scripted modes stay out of the front so they can be
    /// run beside whatever a person is actually doing.
    private var scripted: Bool { options.measure || options.shot }
}

let app = NSApplication.shared
app.setActivationPolicy(options.measure || options.shot ? .accessory : .regular)
let host = AppHost(
    model: SearchModel(client: CsClient(binary: binary, db: options.db, config: options.config),
        limit: options.limit),
    options: options)
app.delegate = host
app.run()
