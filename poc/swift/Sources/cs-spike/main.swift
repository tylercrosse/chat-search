import AppKit
import Foundation
import QuartzCore
import SwiftUI

// A plain executable, not an app bundle: NSApplication is created by hand so the same binary can
// run headless benches and a window. `swift run cs-spike` and nothing else.

struct Options {
    var db: URL?
    var config: URL?
    var binary: String?
    var limit = 60
    var command = "ui"
    var interval = Duration.milliseconds(100)
    var rows = 3200
    /// Bring the window to the front for the run. Steals focus, so it is opt-in — but a latency
    /// measured in a background app is partly a measurement of how macOS treats background apps.
    var front = false
    /// Hold a `beginActivity` assertion. Also opt-in, because whether one is needed is one of the
    /// things being measured.
    var activity = false
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
        case "--rows": if let v = next(), let n = Int(v) { o.rows = n }
        case "--interval": if let v = next(), let n = Int(v) { o.interval = .milliseconds(n) }
        case "--front": o.front = true
        case "--activity": o.activity = true
        case "--help", "-h":
            print("""
                cs-spike [--db PATH] [--config PATH] [--bin PATH] [--limit N] <command>

                  ui          a search field and a result list (default)
                  transport   headless: cost of one `cs search --json` per keystroke
                  states      headless: what a client can tell apart when there is no index
                  rebuild     headless: query a scratch index while `cs index` rewrites it
                  typing      windowed: keystroke → frame, at --interval ms per character
                  list        windowed: --rows rows through List / LazyVStack / VStack
                """)
            exit(0)
        default: if !a.hasPrefix("-") { o.command = a }
        }
        i += 1
    }
    return o
}

let options = parse(Array(CommandLine.arguments.dropFirst()))

guard let binary = CsClient.locate(binary: options.binary) else {
    FileHandle.standardError.write(
        Data("cs not found. Build it (`cargo build --release`) or set CS_BIN.\n".utf8))
    exit(2)
}
let client = CsClient(binary: binary, db: options.db, config: options.config)

/// Owns the window, the display link and the run loop. Every windowed measurement needs all three
/// and none of them exist under `swift run` unless something makes them.
@MainActor
final class Host: NSObject, NSApplicationDelegate {
    let model: SearchModel
    let recorder = FrameRecorder()
    let command: String
    let options: Options
    var window: NSWindow!
    private var keyMonitor: Any?
    private var activity: NSObjectProtocol?

    init(model: SearchModel, command: String, options: Options) {
        self.model = model
        self.command = command
        self.options = options
    }

    func applicationDidFinishLaunching(_ note: Notification) {
        if options.activity {
            activity = ProcessInfo.processInfo.beginActivity(
                options: [.userInitiated, .latencyCritical],
                reason: "measuring keystroke latency")
        }
        let window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 900, height: 620),
            styleMask: [.titled, .closable, .resizable], backing: .buffered, defer: false)
        window.title = "cs-spike"
        window.center()
        let hosting = NSHostingView(rootView: SearchView(model: model))
        window.contentView = hosting
        window.orderFrontRegardless()
        self.window = window
        if options.front { NSApp.activate(ignoringOtherApps: true) }

        recorder.attach(to: hosting)
        model.recorder = recorder

        // The true start of a keystroke. `onChange` on the text binding fires after SwiftUI has
        // already done work, so measuring from there quietly discounts the front of the path;
        // `NSEvent.timestamp` is on the same clock as `CACurrentMediaTime`.
        keyMonitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { [weak self] event in
            self?.model.noteKeystroke(at: event.timestamp)
            return event
        }

        switch command {
        case "typing":
            Task { @MainActor in
                print("typing — hardware keystroke to the frame that shows the answer\n")
                await Bench.typing(model: model, recorder: recorder, interval: options.interval)
                NSApp.terminate(nil)
            }
        case "list":
            Task { @MainActor in
                guard let rows = await Self.loadRows(model.client, limit: options.rows) else {
                    print("could not load rows")
                    NSApp.terminate(nil)
                    return
                }
                await Bench.list(model: model, recorder: recorder, rows: rows, window: window)
                NSApp.terminate(nil)
            }
        default:
            NSApp.activate(ignoringOtherApps: true)
        }
    }

    /// Browse, not search: the empty query is what `cs` answers with the whole corpus, which is
    /// the list size question 2 is about.
    static func loadRows(_ client: CsClient, limit: Int) async -> [Conversation]? {
        do {
            let t = ContinuousClock.now
            let r = try await client.search("", limit: limit, prefix: false)
            print(
                "  loaded \(r.response.results.count) conversations in \(fmt(t.duration(to: .now).ms)) ms "
                    + "(\(fmt(Double(r.timing.bytes) / 1024 / 1024)) MB of json, "
                    + "\(fmt(r.timing.decode)) ms to decode)")
            return r.response.results
        } catch {
            print("  \(error)")
            return nil
        }
    }
}

// Top-level code is an async main-actor context, so the headless benches need no run loop and no
// semaphore — they are just awaited. The windowed ones hand control to `NSApp.run()` instead.
switch options.command {
case "transport":
    await Bench.transport(client: client, limit: options.limit)

case "states":
    await Bench.states(client: client)

case "rebuild":
    guard let db = options.db else {
        FileHandle.standardError.write(
            Data("rebuild needs --db pointing at a scratch index; it deletes and rewrites it\n".utf8))
        exit(2)
    }
    await Bench.rebuild(client: client, db: db)

default:
    let app = NSApplication.shared
    // `.accessory` for the benches so a scripted run does not steal focus from whatever is in
    // front; `.regular` for the interactive one, which has to be typed into.
    app.setActivationPolicy(options.command == "ui" || options.front ? .regular : .accessory)
    let host = Host(
        model: SearchModel(client: client, limit: options.limit),
        command: options.command, options: options)
    app.delegate = host
    app.run()
}
