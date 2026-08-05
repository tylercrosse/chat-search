import AppKit
import CsKit
import Darwin
import Foundation
import QuartzCore

// The one instrument the app keeps, and the reason it keeps it: a number measured on the spike is
// a number about the spike. `poc/swift/RESULTS.md` §1 timed a window with a three-way container
// picker and a five-field bench footer in it, and the app has neither, so the figure had to be
// taken again on what actually ships. Everything here is behind `--measure` and nothing in the
// ordinary path touches it.

/// How late a keystroke's answer was to a frame.
///
/// A display link fires on the main run loop at every refresh whether or not anything is drawn,
/// so the interval between fires says nothing — it is flat at 1/refresh even while the interface
/// is frozen. What jank actually is: the main thread was busy, so the callback for a vsync arrives
/// long after that vsync happened. `lag = now − link.timestamp` is that directly.
@MainActor
final class FrameClock {
    private var link: CADisplayLink?
    /// Read off the link rather than assumed: a hard-coded 60 would hide every dropped frame on
    /// the 120 Hz display this was measured on.
    private(set) var refreshHz: Double = 60
    /// Set when an answer is assigned; cleared at the first vsync the main thread is free to
    /// service, which is the first moment that answer could have been on screen.
    var pendingSince: Double?
    private(set) var toFrame: [Double] = []
    private(set) var lag: [Double] = []

    func attach(to view: NSView) {
        let link = view.displayLink(target: self, selector: #selector(tick(_:)))
        link.add(to: .main, forMode: .common)
        self.link = link
    }

    @objc private func tick(_ link: CADisplayLink) {
        let now = CACurrentMediaTime()
        let period = link.targetTimestamp - link.timestamp
        if period > 0 { refreshHz = 1 / period }
        lag.append((now - link.timestamp) * 1000)
        if let since = pendingSince {
            toFrame.append((now - since) * 1000)
            pendingSince = nil
        }
    }

    func reset() {
        toFrame = []
        lag = []
    }

    /// Vsyncs the main thread was more than one refresh period late for. Below that it made the
    /// frame; above it, something was skipped.
    var missed: Int {
        let period = 1000 / refreshHz
        return lag.filter { $0 > period }.count
    }
}

enum Measure {
    /// The same four phrases `poc/swift/RESULTS.md` §1 was measured with. Duplicated from the
    /// spike's `Bench.phrases` deliberately: the two runs are only comparable if the queries are,
    /// and a measurement fixture is not something the product's library should carry.
    static let phrases = ["borrow checker", "ratatui preview", "sqlite fts5", "launchd"]

    /// Type each phrase one character at a time and report what reached a frame.
    ///
    /// The character is stamped at the moment it is appended rather than read off an
    /// `NSEvent` — there is no hardware event to read in a scripted run — which is the same
    /// method the spike's `typing` bench used, so the numbers line up with §1's.
    @MainActor
    static func typing(model: SearchModel, frames: FrameClock, interval: Duration) async {
        print("keystroke → frame, on the promoted app. \(machineLine())")
        print("  \(fmt(interval.ms)) ms per character, no debounce, one `cs search --json` each")
        print("  \(drivenLine())\n")

        for phrase in phrases {
            model.query = ""
            try? await Task.sleep(for: .milliseconds(400))
            frames.reset()

            for ch in phrase {
                model.noteKeystroke(at: CACurrentMediaTime())
                model.query.append(ch)
                try? await Task.sleep(for: interval)
            }
            try? await Task.sleep(for: .milliseconds(600))

            let f = frames.toFrame
            // The index's own state, printed because a run against a rebuilding index and a run
            // against a ready one are different measurements — and because it is the shortest
            // proof that the successful envelope's `index_state` arrives here at all.
            print("  \"\(phrase)\" — \(model.conversations.count) rows on screen, "
                + "index \(describe(model.health))")
            print("    keystroke→frame  \(line(f))")
            print("    main-thread lag  \(line(frames.lag))")
            print("      \(f.count) of \(phrase.count) keystrokes rendered, "
                + "\(frames.missed) of \(frames.lag.count) vsyncs missed")
        }
    }

    /// Search, open the first result, and draw the window to a PNG from inside the process.
    ///
    /// `cacheDisplay(in:to:)` and not a screenshot: it renders the view hierarchy into a bitmap
    /// with no window server and no screen-recording grant, which is what makes this runnable
    /// from a script and on a machine nobody is sitting at. `poc/swift`'s `snapshot` bench took
    /// the same route for the same reason, and this is the drawer's version of it — the one part
    /// of the app whose correctness is a thing you have to look at.
    @MainActor
    static func shot(model: SearchModel, window: NSWindow, query: String, to path: String) async {
        print(drivenLine())
        model.query = query
        model.queryChanged()
        try? await Task.sleep(for: .seconds(2))
        guard let conv = model.conversations.first else {
            print("nothing matched \"\(query)\" — nothing to open")
            return
        }
        model.reader.open(conv, query: query)
        // Long enough for a `cs show` on the corpus's longest conversation, which measured 50–90
        // ms, plus the layout of however many messages it turned out to have.
        try? await Task.sleep(for: .seconds(2))

        // Printed beside the image, because a picture cannot say which of the two it is: a drawer
        // with no messages in it and a drawer that never loaded look identical at a glance.
        print("\"\(query)\" → \(model.conversations.count) rows, opened \(conv.convId)")
        print("  \(path) \(capture(window, to: path))")
        if let t = model.reader.transcript {
            let folds = t.drawnMessages.reduce(into: [0, 0]) { n, b in
                n[b.fold == .expanded ? 0 : 1] += 1
            }
            print("  \(t.drawn) of \(t.count) drawn · \(folds[0]) expanded, \(folds[1]) collapsed "
                + "· \(t.threads) thread(s) · marked against \(t.terms)")
            print("  \(t.drawnMessages.filter { !$0.marks.isEmpty }.count) messages carry marks, "
                + "\(t.drawnMessages.filter(\.isError).count) failed results kept")
        } else {
            print("  no transcript: \(model.reader.failure ?? "still reading")")
        }

        // After the first picture, because it changes the screen. Typing on with a conversation
        // open is an ordinary thing to do and the drawer is supposed to survive it — including
        // when the row it was opened from is no longer in the results, which is the case a
        // list-driven selection closes without being asked to.
        let after = path.replacingOccurrences(of: ".png", with: "-typed-on.png")
        model.query += "zzz"
        model.queryChanged()
        try? await Task.sleep(for: .seconds(2))
        print("  typed on → \(model.conversations.count) rows, drawer "
            + (model.reader.conv == nil
                ? "closed" : "still open on \(model.reader.transcript?.count ?? 0) messages"))
        print("  \(after) \(capture(window, to: after))")

        // The same answer, cut three ways. A frame each, because a grouping is exactly the kind of
        // thing that has no number: whether a residue group of 41 rows reads as information or as
        // a dumping ground is a question about a picture. The counts are printed beside them for
        // the half a picture cannot state — which axis placed how much, and how much it could not.
        model.query = query
        model.queryChanged()
        try? await Task.sleep(for: .seconds(2))
        for axis in Grouping.allCases where axis != .none {
            model.group(by: axis)
            try? await Task.sleep(for: .milliseconds(400))
            let residue = model.groups.first(where: \.isResidue)?.items.count ?? 0
            let file = path.replacingOccurrences(of: ".png", with: "-by-\(axis.rawValue).png")
            print("  by \(axis.rawValue) → \(model.groups.count) groups over "
                + "\(model.conversations.count) rows, \(residue) in the residue")
            print("  \(file) \(capture(window, to: file))")
        }
        model.group(by: .none)

        // The second view. Empty by design and not by accident — there is no store to author into
        // (`chat-search-6eb.14`) — so what there is to check is that every shelf says which.
        model.surface = .library
        try? await Task.sleep(for: .milliseconds(400))
        let library = path.replacingOccurrences(of: ".png", with: "-library.png")
        print("  library → \(capture(window, to: library)) \(library)")
        model.surface = .search
    }

    /// The window's own view hierarchy as a PNG, with no window server in it.
    @MainActor
    private static func capture(_ window: NSWindow, to path: String) -> String {
        guard let view = window.contentView,
            let rep = view.bitmapImageRepForCachingDisplay(in: view.bounds)
        else { return "(no bitmap for the window)" }
        view.cacheDisplay(in: view.bounds, to: rep)
        guard let png = rep.representation(using: .png, properties: [:]) else {
            return "(could not encode the bitmap)"
        }
        do {
            try png.write(to: URL(fileURLWithPath: path))
            return "(\(png.count) bytes)"
        } catch {
            return "(\(error))"
        }
    }

    private static func describe(_ h: IndexHealth) -> String {
        switch h {
        case .ready: "ready"
        case .rebuilding: "rebuilding — complete, one build behind"
        case .unrecognised(let s): "\"\(s)\", unknown to this build"
        case .noIndex: "missing"
        case .building: "being built"
        case .stale: "unreadable"
        case .noBinary: "cs not found"
        case .failed(let d): "failed — \(d)"
        }
    }

    private static func line(_ values: [Double]) -> String {
        guard !values.isEmpty else { return "no samples" }
        return String(
            format: "min %6.1f  p50 %6.1f  p95 %6.1f  max %6.1f  (n=%d)",
            values.min() ?? 0, percentile(values, 0.50), percentile(values, 0.95),
            values.max() ?? 0, values.count)
    }

    /// Nearest-rank. With 10–40 samples an interpolating percentile invents precision the sample
    /// size does not have.
    private static func percentile(_ values: [Double], _ q: Double) -> Double {
        let sorted = values.sorted()
        let rank = max(0, min(sorted.count - 1, Int((q * Double(sorted.count)).rounded(.up)) - 1))
        return sorted[rank]
    }

    /// Printed by both scripted modes, because a run that deliberately records nothing should be
    /// the one saying so.
    ///
    /// This matters more since the app started recording abandonments. A scripted run quits with
    /// its last phrase still in the box and nothing opened, which is the exact shape of a person
    /// giving up on a search — so without `CsClient.driven` every `--measure` would append a need
    /// nobody had, to a file that cannot be rebuilt from anything. Nothing downstream could tell
    /// those lines apart afterwards either: `cs_core::querylog::Event::Driven` exists because
    /// that mistake is only ever fixable by hand, and this run avoids making it.
    static func drivenLine() -> String {
        "nothing here reaches queries.jsonl: CS_LOG_QUERIES=0 on every `cs` this run spawns"
    }

    /// Printed with the numbers. This is a working laptop with browsers and several agents on it,
    /// so a reading taken at load 12 means something different from the same one at load 2, and
    /// the load belongs beside the result rather than in somebody's memory.
    static func machineLine() -> String {
        var loads = [Double](repeating: 0, count: 3)
        let n = getloadavg(&loads, 3)
        let cores = ProcessInfo.processInfo.activeProcessorCount
        let load = n > 0
            ? loads.prefix(Int(n)).map { String(format: "%.1f", $0) }.joined(separator: " ")
            : "?"
        return "machine: \(cores) cores, load \(load)"
    }
}

func fmt(_ v: Double) -> String { String(format: "%.1f", v) }
