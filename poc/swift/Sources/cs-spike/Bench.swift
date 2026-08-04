import AppKit
import Foundation
import QuartzCore
import SwiftUI

/// The measurements the spike exists to take. Each one is scripted rather than clicked, so the
/// numbers in RESULTS.md are re-derivable by anyone with the repo and this machine's corpus.
enum Bench {
    /// Queries typed one character at a time, the way they are actually entered. Prefixes of real
    /// queries and not random strings: `bor`, `borr`, `borro`… have wildly different result-set
    /// sizes, and the short ones — the expensive ones — are exactly the ones a debounce would
    /// hide.
    static let phrases = ["borrow checker", "ratatui preview", "sqlite fts5", "launchd"]

    // MARK: - Transport

    /// Question 1, without a window in the way: what does one keystroke cost end to end when it
    /// is a whole process?
    static func transport(client: CsClient, limit: Int) async {
        print("transport — one `cs search --json` per keystroke")
        print(machineLine() + "\n")

        // Warm first. A cold 324 MB index is a one-off cost at app launch and would otherwise
        // dominate a p95 that is meant to describe steady typing.
        for phrase in phrases { _ = try? await client.search(phrase, limit: limit) }

        // Two client-side levers, both free to change and neither obviously priced: how many rows
        // to ask for, and whether the last word is treated as incomplete. `--prefix` is what makes
        // typeahead work at all, so if it is expensive that is a fact about the whole design.
        print("  47 prefixes × 3 passes per cell. `seam` is total − sqlite differenced per sample,")
        print("  which is what a `--stdio` transport or a C ABI would be buying back.\n")
        print("    limit  --prefix   total min  total p50  total p95   sqlite min  sqlite p50   seam min  seam p50   stdout")
        for l in [10, 30, 60, 120] {
            for prefix in [true, false] {
                let m = await sweep(client: client, limit: l, prefix: prefix, passes: 3)
                print(
                    "    \(pad(String(l), 7))\(pad(prefix ? "yes" : "no", 11))"
                        + "\(pad(fmt(m.total.min), 11))\(pad(fmt(m.total.p50), 11))\(pad(fmt(m.total.p95), 12))"
                        + "\(pad(fmt(m.server.min), 12))\(pad(fmt(m.server.p50), 13))"
                        + "\(pad(fmt(m.seam.min), 11))\(pad(fmt(m.seam.p50), 11))"
                        + fmt(m.bytes.p50 / 1024) + " KB")
            }
        }

        print("\n  breakdown at limit \(limit), --prefix on:")
        let m = await sweep(client: client, limit: limit, prefix: true, passes: 3)
        print(row("fork/exec", m.launch))
        print(row("spawn→last byte", m.process))
        print(row("json decode", m.decode))
        print(row("of which sqlite (cs ms)", m.server))
        print(row("TOTAL round trip", m.total))
        print("")
        print(row("the seam alone", m.seam))
        print("  by prefix length (p50 ms, all phrases):")
        for n in m.perLength.keys.sorted() {
            print("    \(pad(String(n), 4)) chars: \(fmt(m.perLength[n]!.p50))  (n=\(m.perLength[n]!.count))")
        }
    }

    private struct Sweep {
        var launch = Samples(), process = Samples(), decode = Samples()
        var server = Samples(), total = Samples(), bytes = Samples()
        /// Differenced per sample rather than between percentiles: p95(a) − p95(b) is not p95(a−b)
        /// and would quietly overstate what the seam costs at the tail.
        var seam = Samples()
        var perLength: [Int: Samples] = [:]
    }

    private static func sweep(client: CsClient, limit: Int, prefix: Bool, passes: Int) async -> Sweep {
        var s = Sweep()
        for _ in 0..<passes {
            for phrase in phrases {
                for n in 1...phrase.count {
                    let typed = String(phrase.prefix(n))
                    let r: QueryResult
                    do {
                        r = try await client.search(typed, limit: limit, prefix: prefix)
                    } catch CsError.undecodable(let why, let data) {
                        print("  ! \"\(typed)\": the contract did not decode — \(why)")
                        print("    first 200 bytes: \(String(decoding: data.prefix(200), as: UTF8.self))")
                        continue
                    } catch {
                        print("  ! \"\(typed)\": \(error)")
                        continue
                    }
                    s.launch.add(r.timing.launch)
                    s.process.add(r.timing.process)
                    s.decode.add(r.timing.decode)
                    s.server.add(r.timing.serverMs)
                    s.total.add(r.timing.total)
                    s.seam.add(r.timing.overhead)
                    s.bytes.add(Double(r.timing.bytes))
                    s.perLength[n, default: Samples()].add(r.timing.total)
                }
            }
        }
        return s
    }

    private static func pad(_ s: String, _ n: Int) -> String {
        s.padding(toLength: Swift.max(n, s.count + 1), withPad: " ", startingAt: 0)
    }

    // MARK: - Index states

    /// Question 3. Not "what should the client show" — what does `cs` actually do, since the JSON
    /// contract says nothing about failure and the client has to infer everything from stderr.
    static func states(client: CsClient) async {
        print("index states — what a client can actually tell apart\n")

        func probe(_ label: String, _ c: CsClient) async {
            do {
                let r = try await c.search("borrow", limit: 5)
                print("  \(label): ok, \(r.response.results.count) results in \(fmt(r.timing.total)) ms")
            } catch CsError.unhealthy(let h) {
                print("  \(label): \(describe(h))")
            } catch {
                print("  \(label): \(error)")
            }
        }

        await probe("real index          ", client)

        let scratch = URL(fileURLWithPath: "/tmp/cs-spike-states")
        try? FileManager.default.createDirectory(at: scratch, withIntermediateDirectories: true)

        var missing = client
        missing.db = scratch.appendingPathComponent("absent.db")
        try? FileManager.default.removeItem(at: missing.db!)
        await probe("db path does not exist", missing)
        let created = FileManager.default.fileExists(atPath: missing.db!.path)
        print("      → cs \(created ? "CREATED an empty db file at that path" : "left the path alone")")

        var empty = client
        empty.db = scratch.appendingPathComponent("empty.db")
        try? FileManager.default.removeItem(at: empty.db!)
        FileManager.default.createFile(atPath: empty.db!.path, contents: Data())
        await probe("zero-byte db file   ", empty)

        var garbage = client
        garbage.db = scratch.appendingPathComponent("garbage.db")
        try? FileManager.default.removeItem(at: garbage.db!)
        FileManager.default.createFile(
            atPath: garbage.db!.path, contents: Data(repeating: 0x41, count: 4096))
        await probe("not a database      ", garbage)

        var noBinary = client
        noBinary.binary = URL(fileURLWithPath: "/nonexistent/cs")
        await probe("cs missing          ", noBinary)
    }

    /// The rebuild, watched from a client. Runs `cs index` against a scratch index while querying
    /// it every 250 ms, and records what each query saw — which is the only way to find out
    /// whether "index being rebuilt" is a state a client can even detect.
    static func rebuild(client: CsClient, db: URL) async {
        print("rebuild — querying a scratch index while `cs index` rewrites it\n")

        var indexer = client
        indexer.db = db
        let proc = Process()
        proc.executableURL = client.binary
        var args = ["index", "--db", db.path]
        if let config = client.config { args += ["--config", config.path] }
        proc.arguments = args
        proc.standardOutput = Pipe()
        proc.standardError = Pipe()

        let t0 = ContinuousClock.now
        do { try proc.run() } catch {
            print("  could not start `cs index`: \(error)")
            return
        }

        // A big limit on purpose: the question is not "does a query succeed" but "is the answer
        // complete", and the only way to see that from outside is to watch the count climb.
        var observations: [(Double, String)] = []
        while proc.isRunning {
            let at = t0.duration(to: .now).ms
            do {
                let r = try await indexer.search("the", limit: 4000, prefix: false)
                observations.append((at, "ok, \(r.response.results.count) conversations"))
            } catch CsError.unhealthy(let h) {
                observations.append((at, describe(h)))
            } catch {
                observations.append((at, "\(error)"))
            }
            try? await Task.sleep(for: .milliseconds(250))
        }
        proc.waitUntilExit()
        let elapsed = t0.duration(to: .now).ms

        // Collapse runs: forty identical lines say less than "this state held for 6 seconds".
        var runs: [(String, Double, Double, Int)] = []
        for (at, what) in observations {
            if var last = runs.last, last.0 == what {
                last.2 = at
                last.3 += 1
                runs[runs.count - 1] = last
            } else {
                runs.append((what, at, at, 1))
            }
        }
        for (what, from, to, n) in runs {
            print("  \(fmt(from))–\(fmt(to)) ms (\(n) queries): \(what)")
        }
        print("\n  `cs index` finished in \(fmt(elapsed)) ms; \(observations.count) queries observed it")
        await probeFinal(indexer)
    }

    private static func probeFinal(_ c: CsClient) async {
        if let r = try? await c.search("the", limit: 4000, prefix: false) {
            print("  after: \(r.response.results.count) conversations in \(fmt(r.timing.total)) ms")
        }
    }

    // MARK: - Typing, with the window up

    /// Question 1 again, but through the interface: hardware keystroke to the frame that shows the
    /// result. `interval` is the gap between characters — 100 ms is unhurried, 40 ms is fast
    /// typing, and the difference is how many processes get killed before they answer.
    @MainActor
    static func typing(model: SearchModel, recorder: FrameRecorder, interval: Duration) async {
        print("  " + machineLine())
        for phrase in phrases {
            model.query = ""
            try? await Task.sleep(for: .milliseconds(400))
            recorder.reset()
            model.toData = Samples()
            model.supersededCount = 0

            for ch in phrase {
                // Stand in for the hardware event. The GUI path reads `NSEvent.timestamp`, which
                // is the same clock; here there is no event to read, so the character is stamped
                // at the moment it is appended. Mutating `query` is all that happens — the view's
                // `onChange` runs the search, exactly as it does for a real keypress.
                model.noteKeystroke(at: CACurrentMediaTime())
                model.query.append(ch)
                try? await Task.sleep(for: interval)
            }
            try? await Task.sleep(for: .milliseconds(600))

            print("  \"\(phrase)\" at \(fmt(Double(interval.components.attoseconds) / 1e15 + Double(interval.components.seconds) * 1000)) ms/char")
            print(row("    keystroke→data", model.toData))
            print(row("    keystroke→frame", recorder.toFrame))
            print(row("    main-thread lag", recorder.lag))
            print("      \(model.supersededCount) queries killed by the next keystroke, "
                + "\(recorder.missed) vsyncs missed, \(fmt(memoryFootprintMB())) MB")
        }
    }

    // MARK: - Snapshot

    /// Draw the window to a PNG from inside the process.
    ///
    /// Not a nicety: `screencapture` needs a screen-recording grant that a background shell does
    /// not have, and without one there is no way to check that a measured "frame" contained
    /// anything. `cacheDisplay` asks the view to draw into a bitmap, so it proves the rows
    /// rendered rather than that a timer fired.
    ///
    /// It draws the list faithfully and the chrome unreliably — the text field and footer come out
    /// blank, because they are not drawn by the same path. Good enough for the thing it is for.
    @MainActor
    static func snapshot(model: SearchModel, window: NSWindow, query: String, to path: String) async {
        model.query = query
        try? await Task.sleep(for: .seconds(2))
        guard let view = window.contentView,
              let rep = view.bitmapImageRepForCachingDisplay(in: view.bounds)
        else {
            print("could not make a bitmap for the window")
            return
        }
        view.cacheDisplay(in: view.bounds, to: rep)
        guard let png = rep.representation(using: .png, properties: [:]) else {
            print("could not encode the bitmap")
            return
        }
        try? png.write(to: URL(fileURLWithPath: path))
        print("\(model.conversations.count) rows for \"\(query)\" → \(path) (\(png.count) bytes)")
        for conv in model.conversations.prefix(3) {
            print("  \(conv.source) · \(conv.endedDate ?? "no date") · \(conv.title ?? "«null»")")
            if let hit = conv.hits.first {
                print("    \(hit.kind)/\(hit.role) \(hit.snippetSpans.count) marks: \(hit.snippet.prefix(90))")
            }
        }
    }

    // MARK: - List size

    /// Question 2. Three containers, the same rows, the same window: how much does SwiftUI do for
    /// free and where does it stop doing it.
    @MainActor
    static func list(model: SearchModel, recorder: FrameRecorder, rows: [Conversation], window: NSWindow) async {
        print("list — \(rows.count) rows, each three lines with a marked-up snippet\n")
        let baseline = memoryFootprintMB()
        print("  empty app footprint: \(fmt(baseline)) MB\n")

        for container in RowContainer.allCases {
            model.load([])
            model.container = container
            try? await Task.sleep(for: .milliseconds(500))
            recorder.reset()

            let t0 = CACurrentMediaTime()
            recorder.pendingSince = t0
            model.load(rows)
            // Long enough for even the eager case to finish building; the number wanted is the
            // first frame after assignment, not this ceiling.
            try? await Task.sleep(for: .seconds(6))

            let built = recorder.toFrame.values.first ?? -1
            let after = memoryFootprintMB()
            print("  \(container.label):")
            print("    first frame with the rows: \(fmt(built)) ms")
            print("    footprint after building: \(fmt(after)) MB (+\(fmt(after - baseline)))")

            // Building is half the question. The other half is what happens under the scroller —
            // and the two containers that both look lazy differ only there, because one recycles
            // rows and the other keeps every one it has ever built.
            guard let scroll = findScrollView(window.contentView) else {
                print("    (no scroll view found to drive)\n")
                continue
            }
            let doc = scroll.documentView?.frame.height ?? 0
            let visible = scroll.contentView.bounds.height
            let travel = max(0, doc - visible)
            print("    document height: \(fmt(doc)) pt over \(rows.count) rows")

            // A trackpad-sized step, which is what "scrolling feels smooth" is actually about.
            await drive(scroll, recorder: recorder, from: 0, step: 60, steps: 90)
            print(row("    smooth scroll (60pt)", recorder.lag))
            print("      \(recorder.missed) of \(recorder.lag.count) vsyncs missed")

            // And a scrollbar drag: a screenful or more per frame, where nothing on screen was on
            // screen a moment ago.
            await drive(scroll, recorder: recorder, from: 0, step: travel / 120, steps: 120)
            print(row("    fling (whole doc)", recorder.lag))
            print("      \(recorder.missed) of \(recorder.lag.count) vsyncs missed")

            let scrolled = memoryFootprintMB()
            print("    footprint after scrolling the whole list: \(fmt(scrolled)) MB "
                + "(+\(fmt(scrolled - after)) over building)")
            print("")
        }
    }

    /// One scroll pass, one point per vsync-ish tick. `reflectScrolledClipView` is what makes the
    /// container actually do the work rather than move a layer, which is the whole point.
    @MainActor
    private static func drive(
        _ scroll: NSScrollView, recorder: FrameRecorder, from: Double, step: Double, steps: Int
    ) async {
        recorder.reset()
        for i in 0...steps {
            scroll.contentView.scroll(to: NSPoint(x: 0, y: from + step * Double(i)))
            scroll.reflectScrolledClipView(scroll.contentView)
            try? await Task.sleep(for: .milliseconds(8))
        }
    }

    @MainActor
    private static func findScrollView(_ view: NSView?) -> NSScrollView? {
        guard let view else { return nil }
        if let s = view as? NSScrollView { return s }
        for sub in view.subviews {
            if let s = findScrollView(sub) { return s }
        }
        return nil
    }

    // MARK: -

    static func describe(_ h: IndexHealth) -> String {
        switch h {
        case .ready: "ready"
        case .noIndex(let d): "NO INDEX — \(d)"
        case .rebuilding(let d): "REBUILDING — \(d)"
        case .noBinary(let d): "NO BINARY — \(d)"
        case .failed(let d): "UNCLASSIFIED — \(d)"
        }
    }
}

func fmt(_ v: Double) -> String { String(format: "%.1f", v) }
