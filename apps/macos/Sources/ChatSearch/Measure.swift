import AppKit
import CsKit
import CsTheme
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
    static func shot(
        model: SearchModel, window: NSWindow, query: String, to path: String,
        longest: Bool = false, frames: FrameClock? = nil
    ) async {
        print(drivenLine())
        model.query = query
        model.queryChanged()
        try? await Task.sleep(for: .seconds(2))
        // Before anything is opened, because that is both the state a person browses in and the
        // only moment the list is the rightmost pane in the window.
        density(model: model, window: window)
        // `--longest` opens the biggest conversation the query returned rather than the best. The
        // minimap's hard case is the corpus's longest conversation and no query puts it first, so
        // without this the one thing that has to be looked at cannot be reached from a script.
        let pick = longest
            ? model.conversations.max { $0.msgCount < $1.msgCount } : model.conversations.first
        guard let conv = pick else {
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

        // Before the minimap is driven, so every preset is photographed at the same place in the
        // conversation and the pass that follows measures the scroll from where it always did.
        await presetPass(model: model, window: window, path: path)

        await minimapPass(model: model, window: window, path: path, frames: frames)

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

            // And the same axis shut, which is the picture the fold was argued from and against.
            // `poc/ui/NOTES.md` §2 states the cost of folding in exactly these terms — `source`
            // has four groups, so folded it is four rows and a lot of empty column — and whether
            // that reads as an index or as an empty screen is a question about a picture rather
            // than a number. The row count is printed beside it because a folded list cannot say
            // how much is behind it, which is the other half of the same question.
            model.foldAll(true)
            try? await Task.sleep(for: .milliseconds(400))
            let shut = path.replacingOccurrences(of: ".png", with: "-by-\(axis.rawValue)-folded.png")
            print("  by \(axis.rawValue), folded → \(model.foldedGroups) heads hiding "
                + "\(model.conversations.count) rows, cursor on \(describe(model.cursor))")
            print("  \(shut) \(capture(window, to: shut))")
            model.foldAll(false)
            await foldPass(model: model)
        }
        model.group(by: .none)

        await timelinePass(model: model, window: window, query: query, path: path)

        // The second view. Empty by design and not by accident — there is no store to author into
        // (`chat-search-6eb.14`) — so what there is to check is that every shelf says which.
        model.surface = .library
        try? await Task.sleep(for: .milliseconds(400))
        let library = path.replacingOccurrences(of: ".png", with: "-library.png")
        print("  library → \(capture(window, to: library)) \(library)")
        model.surface = .search
    }

    /// What a row costs, and therefore how much of the answer a screen holds.
    ///
    /// The number the type scale is spent against, and until `chat-search-me9.8.34` nothing
    /// reported it. `poc/ui/directions.html` fences rows-per-screen *between* directions and says
    /// so — it stands 800px in for a viewport nobody has — which makes it silent about what a row
    /// costs at a size somebody opens. Counting them off the PNG is the alternative and it is
    /// worst at exactly the size that matters: at the 720×480 floor the last row is cut by the
    /// edge, and whether a half-drawn row counts is a judgement made by whoever is looking. So
    /// both counts are printed and the arithmetic one is stated first.
    ///
    /// The height is a mean over the rows on screen rather than the document divided by its rows.
    /// A `List` estimates the height of everything it has not laid out yet, so the document is
    /// partly a guess and the guess is a bigger share of it in a small window — which read as
    /// *shorter rows at 720 than at 900*, a difference in the instrument reported as a difference
    /// in the row. The rows in the viewport are the ones that certainly have been measured.
    @MainActor
    private static func density(model: SearchModel, window: NSWindow) {
        let bounds = window.contentView?.bounds.size ?? window.frame.size
        let at = "\(Int(bounds.width))×\(Int(bounds.height))"
        // The results list is an `NSTableView` inside its scroll view, which is a fact about how
        // SwiftUI draws a `List` on this platform rather than a promise it makes. If that ever
        // stops being true the reading is skipped rather than guessed at.
        guard let scroll = rightmostScrollView(window), !model.conversations.isEmpty,
              let table = scroll.documentView as? NSTableView
        else {
            print("  density at \(at): no results list to measure")
            return
        }
        let drawn = table.rows(in: table.visibleRect)
        let heights = (0..<drawn.length).map { table.rect(ofRow: drawn.location + $0).height }
        let visible = scroll.contentView.bounds.height
        guard let row = heights.isEmpty ? nil : heights.reduce(0, +) / Double(heights.count),
              row > 0
        else {
            print("  density at \(at): the list has laid out no rows to measure")
            return
        }
        print("  density at \(at): \(fmt(row)) pt per row, \(fmt(visible)) pt of list → "
            + "\(Int((visible / row).rounded(.down))) rows on screen, "
            + "\(drawn.length) counting the one the edge cuts")
    }

    /// Every preset, on one conversation, plus the three claims about the fidelity model that no
    /// picture states (`chat-search-me9.8.36`).
    ///
    /// A frame each, because what a preset does to a transcript is exactly the kind of thing that
    /// has no number: whether `segments` reads as a summary of a long agent session or as a wall of
    /// `→ 1 message` is a question about a picture, and it is the question the prototype got wrong
    /// twice. The counts go beside them for the half a picture cannot state — how many rows the
    /// knobs left, how many of those are summaries rather than messages, and where the messages
    /// that are gone went.
    ///
    /// Then the three things the pictures are silent about. That `read` is still the wire's own
    /// answer, which is the one place this client spells a rule `cs_core::blocks::Density` also
    /// spells and therefore the one place they can drift apart. That a fold set on one message
    /// beats the band's knob and survives until it is cleared. And that opening another
    /// conversation leaves the knobs alone — the prototype's `defaultZoomFor` is what this is
    /// checking the absence of, and its absence is invisible in every frame.
    @MainActor
    private static func presetPass(
        model: SearchModel, window: NSWindow, path: String
    ) async {
        let reader = model.reader
        guard let opened = reader.conv, let transcript = reader.transcript else { return }
        let drawn = transcript.drawnMessages
        let query = model.query
        // Two other beads' claims are read off these further down, and this pass rebuilds every
        // message four times over by design. Snapshot, report the difference as a number of its
        // own, and put them back — see `MarkedText.restoreCounters`.
        let wasBuilt = (reader.markedText.builds, reader.markedText.reuses)
        let wasDrawn = MinimapBands.renders
        print("  presets, over \(drawn.count) drawn messages of \(opened.convId):")

        for preset in Fidelity.Preset.allCases {
            reader.apply(preset)
            try? await Task.sleep(for: .milliseconds(400))
            let levels = drawn.reduce(into: [Level: Int]()) { $0[reader.level(of: $1), default: 0] += 1 }
            let summaries = reader.rows.count {
                if case .summary = $0 { return true } else { return false }
            }
            print("    \(preset.rawValue.padding(toLength: 11, withPad: " ", startingAt: 0))"
                + "\(reader.rows.count) rows, \(summaries) of them run summaries · "
                + "\(levels[.expanded] ?? 0) full, \(levels[.collapsed] ?? 0) brief, "
                + "\(levels[.hidden] ?? 0) off")
            let file = path.replacingOccurrences(of: ".png", with: "-preset-\(preset.rawValue).png")
            print("    \(file) \(capture(window, to: file))")
            // What a summary line actually says, which is the half of `segments` a frame shows
            // four of and the transcript holds forty. The claim is that it carries calls,
            // failures and questions rather than a count, and a corpus where no run ever failed
            // or asked anything would let that claim pass untested.
            if preset == .segments {
                let drawnSegments = reader.rows.compactMap { row -> Segment? in
                    if case .summary(let segment) = row { return segment } else { return nil }
                }
                print("      of those, \(drawnSegments.count { $0.calls > 0 }) name calls, "
                    + "\(drawnSegments.count { $0.failures > 0 }) failures, "
                    + "\(drawnSegments.count { $0.questions > 0 }) questions, "
                    + "\(drawnSegments.count { $0.marked }) carry the match dot")
                if let richest = drawnSegments.max(by: { ($0.summary.count, $0.calls) < ($1.summary.count, $1.calls) }) {
                    print("      the fullest of them reads: → "
                        + richest.summary.map(\.text).joined(separator: " · "))
                }
            }
        }

        // Back to what the drawer opens at, so everything after this pass sees the screen it
        // always saw — and because the check below is a check about `read`.
        reader.apply(.read)
        try? await Task.sleep(for: .milliseconds(400))
        let agree = drawn.count { Level($0.fold) == Fidelity.Preset.read.fidelity.level(of: $0.band) }
        print("    read is `Density::Full` spelled in Swift, and agrees with the fold on the wire "
            + "for \(agree) of \(drawn.count) drawn messages")

        // A fold set by hand, against a preset that says the opposite. `outline` because it makes
        // every band brief, so one message going full is unambiguous.
        reader.apply(.outline)
        if let first = drawn.first {
            reader.toggle(first)
            try? await Task.sleep(for: .milliseconds(200))
            print("    one message opened by hand: it is \(reader.fold(of: first)) while its "
                + "\(first.band) knob says \(reader.fidelity.level(of: first.band).word), "
                + "\(reader.overrideCount) override in hand")
            reader.clearOverrides()
            try? await Task.sleep(for: .milliseconds(200))
            print("    cleared: it is \(reader.fold(of: first)) again")
        }

        // And the absence of `defaultZoomFor`. Driven with a preset the wire does not answer, so a
        // reader that re-derived the knobs from the transcript would land somewhere else and say so.
        reader.apply(.segments)
        let knobs = reader.fidelity
        if let other = model.conversations.first(where: { $0.id != opened.id }) {
            reader.open(other, query: query)
            try? await Task.sleep(for: .seconds(2))
            print("    opened \(other.convId) with segments in force: the knobs are "
                + (reader.fidelity == knobs ? "where they were left" : "MOVED, which is a defect"))
            reader.open(opened, query: query)
            try? await Task.sleep(for: .seconds(2))
        } else {
            print("    only one conversation matched — nothing to open beside it")
        }
        reader.apply(.read)
        try? await Task.sleep(for: .milliseconds(400))

        // What the whole pass cost, which is the one number here that is about the machine rather
        // than about the model: four presets, an override, and a conversation opened and closed.
        print("    the pass itself: \(reader.markedText.builds - wasBuilt.0) messages built and "
            + "\(MinimapBands.renders - wasDrawn) canvas renders, put back before the next pass "
            + "reads either counter")
        reader.markedText.restoreCounters(builds: wasBuilt.0, reuses: wasBuilt.1)
        MinimapBands.renders = wasDrawn
    }

    /// The scrubber, driven from both ends — and the three claims about it that a PNG cannot make.
    ///
    /// A picture shows the bars and the selection rectangle. What it cannot show is that dragging
    /// wrote `date:A..B` into the *query line* rather than into a variable beside it, that typing
    /// the same range by hand puts the rectangle back, and that the bars under the rectangle were
    /// never narrowed by it. The first two together are the whole of `poc/ui/NOTES.md` §17 — the
    /// prototype kept its timeline range in a tuple nothing drew, and two rounds of another view
    /// were written against that state before anyone noticed. The third is what the drawer is
    /// worth having for: a picture narrowed by its own selection cannot say what widening buys.
    ///
    /// So this drags, prints what the box says, retypes the same window by hand, and compares the
    /// bar heights before and after. There is no test target to put it in (`--verify-theme`'s
    /// note: the Command Line Tools SDK carries neither `Testing` nor `XCTest`), and an invariant
    /// checked by nobody is one that was true on the day it was written.
    @MainActor
    private static func timelinePass(
        model: SearchModel, window: NSWindow, query: String, path: String
    ) async {
        guard let before = model.timeline.drawn else {
            print("  timeline: nothing drawn — \(model.timeline.failure ?? "still counting")")
            return
        }
        let bars = before.buckets.map(\.conversations)
        print("  timeline: \(before.buckets.count) bars of \(before.bucketDays)d, "
            + "\(before.fromDate ?? "?") → \(before.untilDate ?? "?") · \(before.inRange) in "
            + "range · \(before.total) selected · \(before.undated) undated")

        // The right-hand third of the track, which is where a corpus with a retention floor in it
        // actually lives. Fractions and not instants: this is the gesture, not the arithmetic.
        model.scrub(from: 0.66, to: 0.95)
        try? await Task.sleep(for: .seconds(2))
        guard let dragged = model.timeline.drawn, let selected = dragged.window else {
            print("    a drag wrote no window: \(model.query.debugDescription)")
            return
        }
        print("    dragged → the box reads \(model.query.debugDescription)")
        print("    …which parsed back to \(selected.value ?? "nothing"), so the rectangle is "
            + "derived from the text rather than kept beside it")
        print("    bars unchanged by the window: "
            + "\(dragged.buckets.map(\.conversations) == bars)")
        print("    in range \(before.inRange) → \(dragged.inRange), "
            + "selected \(before.total) → \(dragged.total)")
        let scrubbed = path.replacingOccurrences(of: ".png", with: "-scrubbed.png")
        print("    \(scrubbed) \(capture(window, to: scrubbed))")

        // The other direction: the same window typed, not dragged. Cleared first so the selection
        // has to come back rather than merely stay.
        model.apply(chip: query)
        try? await Task.sleep(for: .seconds(1))
        let cleared = model.timeline.drawn?.window == nil
        model.apply(chip: "\(query) date:\(selected.value ?? "")")
        try? await Task.sleep(for: .seconds(1))
        print("    cleared to no window: \(cleared); typed back by hand: "
            + "\(model.timeline.drawn?.window?.value ?? "nothing")")

        // And shut, which is the state that has to change the picture and nothing else.
        model.apply(chip: query)
        try? await Task.sleep(for: .seconds(1))
        let open = model.conversations.count
        model.toggleTimeline()
        try? await Task.sleep(for: .milliseconds(400))
        let hidden = path.replacingOccurrences(of: ".png", with: "-no-timeline.png")
        print("    hidden → \(model.conversations.count) rows against \(open) open")
        print("    \(hidden) \(capture(window, to: hidden))")
        model.toggleTimeline()
        try? await Task.sleep(for: .seconds(1))
    }

    /// The settings window, and what moving each of its three controls does to the app behind it.
    ///
    /// Two claims live here and a picture only carries one of them. The window is a drawing, so it
    /// gets photographed; *changing any of the three redraws the running app without relaunching*
    /// is a before and an after, so every control is moved and the app's own view tree is read back
    /// through `AppearanceProbe` — the same reading `chat-search-me9.8.22` took, and for the same
    /// reason, since an override that reached only the window chrome looks identical in a PNG.
    ///
    /// The third claim is that none of it is written down. A scripted run never writes the
    /// preference, and this run drives the very controls whose ordinary job is to write it, so the
    /// four keys are read before and after and printed either way.
    @MainActor
    static func settings(
        model: SearchModel, settings: ThemeSettings, view: NSView, query: String, to path: String
    ) async {
        print(drivenLine())
        let keys = [
            ThemeChoice.directionKey, ThemeChoice.lightKey, ThemeChoice.darkKey,
            ThemeChoice.appearanceKey,
        ]
        func plist() -> [String: String] {
            keys.reduce(into: [:]) { $0[$1] = UserDefaults.standard.string(forKey: $1) }
        }
        let before = plist()

        // Rows first. A theme redrawing an empty window is a picture of a background colour, and
        // what these settings are for is the list and the badges and the ribbon under them.
        model.query = query
        model.queryChanged()
        try? await Task.sleep(for: .seconds(2))

        // The menu, and then Cmd-comma through it. `performKeyEquivalent` is the call AppKit makes
        // for a real keystroke, so this is the whole path a person's Cmd-comma takes minus the
        // hardware — with no menu bar, or with the item unwired, it returns false and no window
        // appears, which is the state this app was in before `chat-search-me9.8.21`.
        for line in menuLines() { print("  \(line)") }
        let handled = NSApp.mainMenu?.performKeyEquivalent(with: press(",", code: 43)) ?? false
        try? await Task.sleep(for: .milliseconds(500))
        guard let panel = NSApp.windows.first(where: { $0.title == "Settings" }) else {
            print("  Cmd-comma: handled \(handled), and no settings window came up")
            return
        }
        print("  Cmd-comma: handled \(handled), opened \"\(panel.title)\" at "
            + "\(Int(panel.frame.width))x\(Int(panel.frame.height))")
        print("settings: \(Theme.directions.count) directions in each menu, "
            + "\(Appearance.allCases.count) appearances, \(model.conversations.count) rows behind "
            + "the window")
        // As it opens, before anything is driven: three controls standing for what the launch
        // resolved, which is the only frame in this run that shows a remembered choice being read
        // back rather than a chosen one being applied.
        let opened = path.replacingOccurrences(of: ".png", with: "-settings-as-opened.png")
        print("  \(settings.appearance.rawValue), \(settings.light.name) light, "
            + "\(settings.dark.name) dark, type and spacing from \(settings.layout.name)")
        print("  \(opened) \(capture(panel.contentView, to: opened))")

        // Each appearance in turn: what the app's own view tree resolved, and both windows drawn in
        // it. The panel as well as the app, because the panel is the one view in this process that
        // is *not* themed — it follows the override through AppKit alone, which is the shortest
        // demonstration that the override is at the application and not in the token layer.
        for appearance in Appearance.allCases {
            settings.choose(appearance: appearance)
            try? await Task.sleep(for: .milliseconds(500))
            print("  " + AppearanceProbe.line(theme: settings.theme, view: view, asked: appearance))
            for (name, drawn) in [("settings", panel.contentView), ("app", view)] {
                let file = path.replacingOccurrences(
                    of: ".png", with: "-\(appearance.rawValue)-\(name).png")
                print("    \(file) \(capture(drawn, to: file))")
            }
        }

        // And a side, which is the control whose effect a probe can state exactly: the theme in
        // force gains a borrowed half, and `--bg` comes back as that direction's own value. Every
        // direction in the menu, because the menu offers every direction.
        settings.choose(appearance: .light)
        for direction in Theme.directions {
            settings.choose(light: direction)
            try? await Task.sleep(for: .milliseconds(500))
            print("  light theme → \(direction.name): drawing \(settings.theme.name), "
                + "type and spacing from \(settings.layout.name)")
            print("    " + AppearanceProbe.line(
                theme: settings.theme, view: view, asked: settings.appearance))
        }

        // The mixed pair the two menus exist for, photographed on both sides: one direction's light
        // beside another's dark, chosen from a window rather than from a command line.
        settings.choose(light: .paper)
        settings.choose(dark: .terminal)
        for appearance in [Appearance.light, .dark] {
            settings.choose(appearance: appearance)
            try? await Task.sleep(for: .milliseconds(500))
            let file = path.replacingOccurrences(
                of: ".png", with: "-mixed-\(appearance.rawValue).png")
            print("  \(settings.theme.name), \(appearance.rawValue)")
            print("    \(file) \(capture(view, to: file))")
        }

        let after = plist()
        print("  the four keys before: \(describe(before))")
        print("  the four keys after:  \(describe(after))")
        print("  a scripted run wrote nothing: \(before == after)")

        // Which leaves the write itself unshown, because this run is the one run that must not
        // make it. So it is made against a domain nobody reads, through the same two functions and
        // the same four key constants a person's click goes through.
        writes()

        // And Cmd-Q, the other thing the menu fixed. Pressed rather than described, and pressed
        // last: if the key reaches the item this process ends on it, so the line below is only ever
        // printed when it did not.
        print("  Cmd-Q: pressing it, and this run ends there if the menu handled it")
        _ = NSApp.mainMenu?.performKeyEquivalent(with: press("q", code: 12))
        try? await Task.sleep(for: .seconds(1))
        print("  Cmd-Q DID NOT QUIT — the item is on the menu and the key did not reach it")
    }

    /// What the window writes down, taken against a scratch defaults domain.
    ///
    /// The two rules worth checking are the ones a settings window can get wrong in a way no
    /// picture shows: the four keys are `ThemeChoice`'s and not a second set beside them, and two
    /// menus naming one direction collapses to `--theme NAME` — the direction whole, with the side
    /// overrides cleared rather than left behind to contradict it.
    ///
    /// The domain is removed afterwards, so `defaults read chat-search-settings-probe` says it does
    /// not exist. `cfprefsd` leaves an empty plist behind at that name anyway, which is residue
    /// rather than state — the app's own domain sits as the same empty file when nothing is set.
    @MainActor
    private static func writes() {
        let name = "chat-search-settings-probe"
        guard let scratch = UserDefaults(suiteName: name) else { return }
        func state() -> String {
            describe([
                ThemeChoice.directionKey, ThemeChoice.lightKey, ThemeChoice.darkKey,
                ThemeChoice.appearanceKey,
            ].reduce(into: [:]) { $0[$1] = scratch.string(forKey: $1) })
        }
        ThemeChoice.remember(appearance: .dark, store: scratch)
        print("  a window write, appearance dark:      \(state())")
        ThemeChoice.remember(light: .paper, dark: .terminal, store: scratch)
        print("  a window write, two sides that differ: \(state())")
        ThemeChoice.remember(light: .paper, dark: .paper, store: scratch)
        print("  a window write, two sides that agree:  \(state())")
        UserDefaults.standard.removePersistentDomain(forName: name)
    }

    /// The theme file, saved the ways an editor saves one, and what the app drew after each.
    ///
    /// The check `chat-search-me9.8.39` could not get any other way. What the watch claims is that
    /// **a save redraws a running app** and that **a save caught mid-write does not**, and neither
    /// is a picture or an exit code: the first is a difference between two frames a second apart,
    /// and the second is the *absence* of a difference at a moment nothing marks. So the saves are
    /// made here, from inside the app they are aimed at, and what was on screen afterwards is
    /// printed beside each one.
    ///
    /// **The saves are the real ones.** `write(atomically: false)` truncates the file and writes it
    /// again, which is an editor saving in place; `write(atomically: true)` writes a temporary file
    /// and renames it over, which is vim's default on macOS and the case that kills a watch
    /// registered on a descriptor. The inode is printed for that reason — a run where it never
    /// changes is a run that never checked the case the watch exists for.
    ///
    /// Three of the eleven states are photographed, because two of the claims are about colour and
    /// one is about the type scale, which is a relayout rather than a repaint.
    ///
    /// Its own file, under a directory of its own: a pass that watched `/tmp` would hear every
    /// other process on the machine, and one that saved over `~/.config/chat-search/theme.css`
    /// would cost whoever ran it an afternoon.
    @MainActor
    static func themeFile(
        model: SearchModel, settings: ThemeSettings, reload: ThemeReload, view: NSView, file: URL,
        query: String, to path: String
    ) async {
        print(drivenLine())
        // The dark side, forced, so that the colour this reads back off the theme is the colour on
        // the screen whatever the machine's own appearance is. A scripted run remembers none of it.
        settings.choose(appearance: .dark)
        // Rows, and a conversation open behind them: a theme redrawing an empty window is a picture
        // of a background colour, and the marks in the transcript are the one thing on screen that
        // is *built* from tokens rather than drawn in them (`MarkedText`).
        model.query = query
        model.queryChanged()
        try? await Task.sleep(for: .seconds(2))
        if let conv = model.conversations.first {
            model.reader.open(conv, query: query)
            try? await Task.sleep(for: .seconds(2))
        }
        print("\"\(query)\" → \(model.conversations.count) rows, "
            + "\(model.reader.transcript?.drawn ?? 0) messages in the drawer")
        print("theme file: \(file.path)")

        let source = ThemeFile.text(for: .shipped)
        var heard = 0
        var built = model.reader.markedText.builds

        /// What is on screen, what the file is now, and what the reload had to say about it.
        func drew(_ what: String, shot: String? = nil) {
            print("  \(what)")
            let theme = settings.theme
            let klass = settings.user == nil ? ThemeClass.direction : .userTheme
            // The four this pass moves, and the dark side of each because the appearance is forced
            // above — so these are the values on the screen and not one of the two it could be.
            print("    drawing \(theme.name) · \(klass.label) — --bg \(theme.dark[.bg].hex), "
                + "--ink-3 \(theme.dark[.ink3].hex), --hit-bg \(theme.dark[.hitBg].hex), "
                + "--fs-body \(fmt2(theme.size(.body)))pt")
            let marks = model.reader.markedText.builds
            print("    \(marks - built) messages re-marked, \(node(file))")
            built = marks
            let fresh = reload.spoken[heard...]
            heard = reload.spoken.count
            if fresh.isEmpty {
                print("    said nothing")
            } else {
                fresh.forEach { print("    said: \($0.trimmingCharacters(in: .whitespaces))") }
            }
            guard let shot else { return }
            let out = path.replacingOccurrences(of: ".png", with: "-\(shot).png")
            print("    \(out) \(capture(view, to: out))")
        }

        /// A save. `atomically: false` truncates the file and writes it again — an editor saving in
        /// place; `true` writes a temporary file and renames it over, which replaces the inode.
        func save(_ text: String, atomically: Bool = false) {
            do {
                try text.write(to: file, atomically: atomically, encoding: .utf8)
            } catch {
                print("    could not write \(file.path): \(error.localizedDescription)")
            }
        }

        /// Long enough for the watch to have settled and — when the file did not parse — to have
        /// looked a second time before saying so. Two quiet periods and some slack.
        func settle() async { try? await Task.sleep(for: .milliseconds(700)) }

        // Every edit is made on the last one rather than on the file `--write-theme` wrote, which is
        // what dialling a theme in actually looks like: nobody starts over between nudges, and a
        // value that moved two saves ago has to still be on screen three saves later.
        var text = source
        func edit(_ token: String, _ line: String) { text = rewrite(text, token, as: line) }

        drew("as launched", shot: "watch-as-launched")

        edit("--bg", "--bg: #2a1a3a;")
        save(text)
        await settle()
        drew("saved in place — truncated and written again, the same inode")

        edit("--bg", "--bg: #102030;")
        edit("--fs-body", "--fs-body: 16px;")
        save(text, atomically: true)
        await settle()
        drew(
            "saved atomically — a new file renamed over the old one, and the type scale moved",
            shot: "watch-after-a-save")

        // Two writes with a gap an editor's own truncate-then-write is well inside of, which is the
        // ordinary case and the one the quiet period is for: one reload, and nothing said about the
        // half-written file in the middle of it. `--hit-bg` because the marks in the drawer are
        // built from it and cached — the count beside each state is what says they were rebuilt.
        edit("--hit-bg", "--hit-bg: #7a3d00;")
        save(String(text.prefix(text.count / 2)))
        try? await Task.sleep(for: .milliseconds(10))
        save(text)
        await settle()
        drew("saved in two writes 10ms apart — half the file, then all of it")

        // The same two writes with the quiet period between them, which no interval can coalesce.
        // This is the state the acceptance criterion is about: what is on screen must not become
        // the shipped direction because somebody's editor was caught halfway.
        edit("--hit-bg", "--hit-bg: #14503c;")
        save(String(text.prefix(text.count / 2)))
        await settle()
        drew("caught between the two writes — half a file on disk")
        save(text)
        await settle()
        drew("and the second write lands")

        // A file that is whole and still not a theme. Two complaints from one typo, because a
        // mistyped name is both a token this build has never heard of and a hole where the real
        // one should have been.
        let typo = rewrite(text, "--ink-3", as: "--ink3: #8a9396;")
        save(typo)
        await settle()
        drew("a typo — `--ink3` for `--ink-3`, which is a name and a hole")
        save(typo)
        await settle()
        drew("the same typo saved again")

        edit("--ink-3", "--ink-3: #1e2426;")
        save(text)
        await settle()
        drew(
            "a whole theme that misses the fence — the quiet tier at the page's own colour",
            shot: "watch-unfenced")
        save(text)
        await settle()
        drew("the same unfenced theme saved again")

        try? FileManager.default.removeItem(at: file)
        await settle()
        drew("the file removed")

        edit("--ink-3", "--ink-3: #8a9396;")
        save(text)
        await settle()
        drew("and written back")

        // The watch goes before the file does, so that clearing up is not itself an event this run
        // reports on. The directory goes only when this run is the one that made it and nothing
        // else ended up in it: `--theme-file` can point anywhere, and `deletingLastPathComponent`
        // on a path somebody else chose is a directory this pass is not entitled to remove.
        reload.stop()
        try? FileManager.default.removeItem(at: file)
        let scratch = file.deletingLastPathComponent()
        let mine = scratch.lastPathComponent.hasPrefix("chat-search-watch-")
        let empty = (try? FileManager.default.contentsOfDirectory(atPath: scratch.path))?.isEmpty
        if mine, empty == true {
            try? FileManager.default.removeItem(at: scratch)
            print("  \(scratch.path) removed")
        } else {
            print("  \(file.path) removed")
        }
    }

    /// The same file with one declaration rewritten, which is what an edit is.
    ///
    /// The first occurrence, which is the dark block — `--write-theme` writes `:root` before
    /// `:root.light` — so what this changes is what `theme.dark` reads back. The whole replacement
    /// line rather than a value, because two of the edits above change the token's *name*, which is
    /// the commonest way a hand-typed file stops being a theme.
    private static func rewrite(_ text: String, _ token: String, as line: String) -> String {
        var lines = text.components(separatedBy: "\n")
        guard
            let at = lines.firstIndex(where: {
                $0.trimmingCharacters(in: .whitespaces).hasPrefix(token + ":")
            })
        else { return text }
        lines[at] = "  \(line)"
        return lines.joined(separator: "\n")
    }

    /// Which file is at that path, as the number that says whether it is still the same one.
    private static func node(_ url: URL) -> String {
        var info = stat()
        guard stat(url.path, &info) == 0 else { return "nothing at that path" }
        return "inode \(info.st_ino), \(info.st_size) bytes"
    }

    /// A key equivalent as AppKit would deliver it, minus the hardware.
    private static func press(_ character: String, code: UInt16, shift: Bool = false) -> NSEvent {
        NSEvent.keyEvent(
            with: .keyDown, location: .zero,
            modifierFlags: shift ? [.command, .shift] : .command, timestamp: 0,
            windowNumber: 0, context: nil, characters: character,
            charactersIgnoringModifiers: character, isARepeat: false, keyCode: code)!
    }

    /// The menu bar as it stands, with the key equivalent beside each item — which is the half of
    /// a menu that does the work, and the half that is invisible in a screenshot of a menu bar.
    @MainActor
    private static func menuLines() -> [String] {
        guard let bar = NSApp.mainMenu else { return ["main menu: none — nothing can be pressed"] }
        return ["main menu: \(bar.numberOfItems) menu(s)"]
            + bar.items.flatMap { menu in
                (menu.submenu?.items ?? []).map { item in
                    let key = item.keyEquivalent.isEmpty ? "" : "  ⌘\(item.keyEquivalent)"
                    return item.isSeparatorItem ? "  —" : "  \(item.title)\(key)"
                }
            }
    }

    /// The four preference keys, in a form one run can be diffed against itself with.
    private static func describe(_ keys: [String: String]) -> String {
        let set = keys.sorted { $0.key < $1.key }.map { "\($0.key)=\($0.value)" }
        return set.isEmpty ? "none set" : set.joined(separator: " ")
    }

    /// The clipboard, pressed through the menu that delivers it.
    ///
    /// `chat-search-me9.8.24` is one claim — Cmd-C, Cmd-V, Cmd-X, Cmd-A and Cmd-Z do something in
    /// the search field — and there is no picture of it: a box reading `borrow checker` looks the
    /// same however the text arrived. So the keys are pressed the way AppKit presses them, on
    /// `NSApp.mainMenu`, and the field editor is read back after each. Before this bead every one
    /// of them found no item, reached nothing and moved nothing; that is the whole of the bug, and
    /// it is why the check is on the menu rather than on the field, which was never broken.
    ///
    /// It has since grown a key that is not the clipboard's and not AppKit's: Cmd-T, the View item
    /// `chat-search-me9.8.26` added, whose effect is on this app's own model rather than on a text
    /// system somebody else wrote. It belongs in this run and not in `--shot` for the same reason
    /// everything else here does — what it moves is a boolean, and a picture of a shut drawer says
    /// nothing about which of the three ways of shutting it did the shutting.
    ///
    /// **This is the one scripted run that takes the front, and that is what makes it a run.** A
    /// key equivalent is resolved against the key window's first responder and an inactive
    /// application has no key window, so a pass that presses keys has to be frontmost — which is
    /// exactly what `--measure` and `--shot` must not be, since a latency taken in a background app
    /// and one taken in a frontmost app are not the same measurement. Folding this into `--shot`
    /// would have cost that run its comparability with `poc/swift/RESULTS.md` §1. Measured rather
    /// than assumed: with `--shot`'s `.accessory` policy every item on this bar validates as
    /// disabled and each key press moves nothing, which is what a background app's menu bar is.
    ///
    /// **It puts the pasteboard back.** A scripted run that ate somebody's clipboard would be the
    /// same mistake as one that appended to `queries.jsonl` — a benchmark writing over a thing a
    /// person put there on purpose (ADR 22). What it restores is the string, so a run that
    /// interrupts a copied image or a promised file leaves the string standing in its place. Stated
    /// rather than hidden: the general pasteboard cannot be snapshotted whole.
    @MainActor
    static func clipboard(model: SearchModel, window: NSWindow, query: String) async {
        print(drivenLine())
        print("the menu, and the clipboard it delivers. \(machineLine())")
        for line in menuLines() { print("  \(line)") }

        // Taking the front once is not enough on a machine with anything else running on it. A
        // build finishing, a terminal being scripted, a notification — anything that activates
        // takes key status away, and from that moment every item on this bar validates as grey and
        // every key press below moves nothing while still reporting that the menu matched it. That
        // is the failure this pass is most likely to have, and it looks exactly like the bug it is
        // checking for, so the front is reclaimed before each press and the reclaims are counted.
        var reclaimed = 0
        @discardableResult @MainActor func front() async -> Bool {
            guard !window.isKeyWindow else { return true }
            reclaimed += 1
            for _ in 0..<10 where !window.isKeyWindow {
                NSApp.activate(ignoringOtherApps: true)
                window.makeKeyAndOrderFront(nil)
                try? await Task.sleep(for: .milliseconds(200))
            }
            return window.isKeyWindow
        }

        try? await Task.sleep(for: .milliseconds(600))
        await front()
        print("  key window \(window.isKeyWindow), first responder "
            + (window.firstResponder.map { String(describing: type(of: $0)) } ?? "none"))
        validation("with an empty box")
        guard let editor = window.firstResponder as? NSTextView else {
            print("  the query box does not hold the first responder — nothing to drive")
            return
        }

        let held = NSPasteboard.general.string(forType: .string)
        var restored = false
        // A function and not only a `defer`, because this pass ends by pressing Cmd-W and the
        // process does not come back from that — `NSApp.terminate` unwinds no scope, so a restore
        // that lived in a `defer` alone would run on every path except the one this run takes.
        // Found by running it: the first pass to press Cmd-W left the phrase on the clipboard.
        func restore() {
            guard !restored else { return }
            restored = true
            NSPasteboard.general.clearContents()
            if let held { NSPasteboard.general.setString(held, forType: .string) }
            print("  the pasteboard is put back to \"\(held ?? "")\"")
        }
        defer { restore() }

        // A phrase with a filter token in it, because pasting a path or a phrase out of a terminal
        // is the ordinary reason this key gets reached for, and a grammar is the kind of thing
        // people paste rather than retype.
        let phrase = "dir:chat-search agent:codex"
        model.query = phrase
        try? await Task.sleep(for: .milliseconds(600))

        // `matched` is what the menu says and not what happened: `performKeyEquivalent` answers
        // that an item carries this key, which is false before this bead and true after it, but it
        // is true of a greyed item too. The state read back beside it is the evidence.
        func key(_ character: String, _ code: UInt16, shift: Bool = false) async -> String {
            await front()
            let matched = NSApp.mainMenu?.performKeyEquivalent(
                with: press(character, code: code, shift: shift)) ?? false
            try? await Task.sleep(for: .milliseconds(600))
            return "matched \(matched)"
        }
        // The field and the query both, because they are two facts: an edit that the field editor
        // performed and SwiftUI never heard about would leave the box reading one thing and the
        // search answering another, which is the way this can be wrong while looking right.
        func state() -> String {
            "field \"\(editor.string)\" · query \"\(model.query)\" · pasteboard "
                + "\"\(NSPasteboard.general.string(forType: .string) ?? "")\""
        }

        print("  the box holds \"\(editor.string)\"")
        print("  ⌘A → \(await key("a", 0)), selected \(editor.selectedRange().length) of "
            + "\(editor.string.count) characters")
        print("  ⌘C → \(await key("c", 8)), \(state())")
        print("  ⌘X → \(await key("x", 7)), \(state())")
        print("  ⌘V → \(await key("v", 9)), \(state())")
        print("  ⌘Z → \(await key("z", 6)), \(state())")
        print("  ⇧⌘Z → \(await key("z", 6, shift: true)), \(state())")
        _ = await key("a", 0)
        await front()
        validation("with the phrase selected")

        // The negative control, and it is a real one rather than a contrived key: Cmd-H is on no
        // menu here because `MainMenu` measured `hide:` as permanently grey in this process, so
        // pressing it shows what all six keys above looked like before this bead — a key equivalent
        // no item carries, matching nothing and moving nothing. The line beneath it is the other
        // half of that reading: the capability exists and it is the menu route that does not.
        print("  ⌘H, which this bar deliberately does not carry → \(await key("h", 4)), "
            + "the app is hidden: \(NSApp.isHidden)")
        NSApp.hide(nil)
        try? await Task.sleep(for: .milliseconds(900))
        print("    NSApp.hide(nil) called directly → the app is hidden: \(NSApp.isHidden), "
            + "so it is the item AppKit refuses and not the act")
        NSApp.unhide(nil)
        await front()
        try? await Task.sleep(for: .milliseconds(600))

        // Minimize, pressed for the reason the settings pass presses Cmd-Q rather than describing
        // it. Put back afterwards: a pass that left the window in the Dock would be reporting on a
        // run nobody could see the rest of.
        print("  ⌘M → \(await key("m", 46)), the window is in the Dock: \(window.isMiniaturized)")
        window.deminiaturize(nil)
        await front()
        try? await Task.sleep(for: .milliseconds(600))

        // Cmd-T, and it is the only key on this bar whose action is this app's own — every other
        // one resolves into AppKit. `chat-search-me9.8.26`: the drawer opened and shut from a button
        // in its corner and from nowhere else, and this window's one focused view is the query box,
        // so the menu is the only place the key could be bound.
        //
        // **Pressed twice, because the claim is a toggle.** One press proves a key matched an item;
        // it does not distinguish "the drawer shut" from "the drawer was already shut", and the
        // second press is what says the item is a switch rather than a one-way trip. The pair also
        // leaves the drawer as this run found it, which is the same courtesy the pasteboard gets.
        //
        // The title is read back through `update()` rather than off the item, because the verb is
        // written during validation and reading it without asking for one would report the title
        // this bar was *built* with (`AppHost.validateMenuItem`).
        @MainActor func timelineVerb() -> String {
            let menu = NSApp.mainMenu?.items.first { $0.title == "View" }?.submenu
            menu?.update()
            return menu?.items.first?.title ?? "there is no View menu"
        }
        for press in ["⌘T", "⌘T again"] {
            print("  \(press) → \(await key("t", 17)), the drawer is open: "
                + "\(model.timeline.shown), the item now reads \"\(timelineVerb())\"")
        }

        // The transcript's half, and the honest shape of it. A selection there is made by dragging,
        // a drag is the one gesture nothing here can produce — the same wall the fold pass names
        // from the other side, and posting synthetic mouse events would need the Accessibility
        // grant this app has never asked anybody for. So what is asked instead is where AppKit
        // *would* send Copy: `target(forAction:)` is the call the menu itself makes, so a target
        // here is the item resolving through the responder chain rather than dying at the end of it.
        //
        // With no selection it resolves nowhere, which is the correct answer and not a finding: a
        // Copy that offered itself with nothing selected would be the lie in the other direction.
        // So this half is *reasoned* rather than read, and the reasoning is named on the line
        // itself so nobody mistakes it for a measurement.
        model.query = query
        model.queryChanged()
        try? await Task.sleep(for: .seconds(2))
        guard let conv = model.conversations.first else {
            print("  nothing matched \"\(query)\" — the transcript's half is unchecked")
            restore()
            return
        }
        model.reader.open(conv, query: query)
        try? await Task.sleep(for: .seconds(2))
        await front()
        let copySelector = #selector(NSText.copy(_:))
        func lands() -> String {
            NSApp.target(forAction: copySelector).map { String(describing: type(of: $0)) }
                ?? "nowhere"
        }
        print("  \(model.reader.transcript?.drawn ?? 0) messages of \(conv.convId) open, "
            + "\(copyResponders(window).count) view(s) in the window answer `copy:`")
        print("    from the query box, Copy lands on \(lands())")
        // A conversation is open by this point, so the rightmost pane is the drawer.
        if let drawer = rightmostScrollView(window)?.documentView {
            window.makeFirstResponder(drawer)
            try? await Task.sleep(for: .milliseconds(600))
            print("    with the first responder moved into the drawer "
                + "(\(window.firstResponder.map { String(describing: type(of: $0)) } ?? "none")), "
                + "Copy lands on \(lands())")
        }
        print("    nowhere is right with nothing selected, and a selection in the transcript is "
            + "made with the pointer, which nothing here drives")
        print("    so that half is reasoned and not read: it is this same item on this same chain, "
            + "and `copy:` against nil is what SwiftUI's own TextEditingCommands installs")
        print("  the front was taken back \(reclaimed) time(s) during this pass")

        // And Cmd-W last, for the reason the settings pass presses Cmd-Q last: closing this
        // window is what quits this app, so if the key reaches the item the run ends on it and the
        // line below is only ever printed when it did not. The pasteboard goes back *before* it
        // rather than on the way out, since there is no way out.
        restore()
        print("  ⌘W: pressing it, and this run ends there if the menu handled it")
        _ = await key("w", 13)
        print("  ⌘W DID NOT CLOSE THE WINDOW — the item is on the menu and the key did not reach it")
    }

    /// Every item as AppKit validates it against whatever holds the first responder now.
    ///
    /// This is the half of a menu bar no screenshot states and the half this bead turns on: an item
    /// AppKit greys out is an item that reaches nothing, whoever put it there. It is read twice,
    /// with an empty box and with a phrase selected, because half of these are *supposed* to be
    /// grey in the first reading — a Copy that offered itself with nothing selected would be the
    /// same lie in the other direction.
    @MainActor
    private static func validation(_ when: String) {
        print("  the menu \(when):")
        for menu in NSApp.mainMenu?.items.compactMap(\.submenu) ?? [] {
            menu.update()
            print("    \(menu.title.isEmpty ? "(application)" : menu.title): "
                + menu.items.filter { !$0.isSeparatorItem && !$0.isAlternate }
                    .map { "\($0.title)\($0.isEnabled ? "" : " [grey]")" }
                    .joined(separator: ", "))
        }
    }

    /// Every view in the window that answers `copy:`, which is where an Edit menu's Copy lands.
    @MainActor
    private static func copyResponders(_ window: NSWindow) -> [NSView] {
        var found: [NSView] = []
        func walk(_ view: NSView?) {
            guard let view else { return }
            if view.responds(to: #selector(NSText.copy(_:))) { found.append(view) }
            for sub in view.subviews { walk(sub) }
        }
        walk(window.contentView)
        return found
    }

    /// The fold's other half, which is not a picture: where the cursor is allowed to be.
    ///
    /// `chat-search-me9.8.15` is two claims — a group folds, and a folded group cannot hold the
    /// cursor — and only the first of them shows up in a PNG. This drives the second: put the
    /// cursor on a row, shut the group around it, then walk the list a line at a time from the top
    /// and count what it landed on. There is no test target to put this in, for the same reason
    /// `--verify-theme` is a flag (the Command Line Tools SDK carries neither `Testing` nor
    /// `XCTest`), and an invariant checked by nobody is one that was true on the day it was
    /// written.
    @MainActor
    private static func foldPass(model: SearchModel) async {
        let axis = model.grouping
        guard let first = model.groups.first, let inside = first.items.first else { return }

        // A click on a head with the cursor already inside that group, which is the only gesture
        // that can strand it.
        model.cursor = .row(inside.id)
        model.toggleFold(first.key)
        let rescued = model.cursor == .head(first.key)

        // Every line from the top, by the key an arrow sends. `lines` is what the list draws, so a
        // row reached here that belongs to a folded group is a row on nobody's screen.
        model.cursor = model.lines.first
        var visited: [Cursor] = []
        for _ in 0...model.lines.count {
            if let cursor = model.cursor { visited.append(cursor) }
            model.moveSelection(by: 1)
        }
        let hidden = Set(
            model.groups.filter { model.isFolded($0.key) }.flatMap { $0.items.map(\.id) })
        let trespass = visited.filter {
            guard case .row(let id) = $0 else { return false }
            return hidden.contains(id)
        }

        print("  fold, by \(axis.rawValue): \(model.foldedGroups) of \(model.groups.count) shut, "
            + "\(model.lines.count) lines the cursor can reach")
        print("    folding under the cursor moved it to the head: \(rescued)")
        print("    rows reached inside a folded group: \(trespass.count) of \(hidden.count) hidden")

        // Switching axes clears the accordion. Away and back, because clicking the axis already in
        // force is inert by design and so would prove nothing.
        model.group(by: axis == .source ? .project : .source)
        model.group(by: axis)
        try? await Task.sleep(for: .milliseconds(200))
        print("    after switching axis and back: \(model.foldedGroups) folded")
    }

    /// The scroll relationship, driven from both ends.
    ///
    /// The minimap is the one part of this app whose correctness is a *relationship* rather than a
    /// drawing: the transcript's position has to reach the viewport box and a drag on the box has
    /// to reach the transcript. Neither half has a number in the ordinary sense, but both have a
    /// before and an after, so this drives each one and prints what moved. `chat-search-me9.8.18`
    /// costed three containers for this and took the reversible one — these are the numbers that
    /// say whether that holds.
    @MainActor
    private static func minimapPass(
        model: SearchModel, window: NSWindow, path: String, frames: FrameClock?
    ) async {
        let reader = model.reader
        guard !reader.minimap.isEmpty else { return }
        print("  minimap: \(reader.minimap.blocks.count) bands, "
            + "\(footprintMB()) MB in this process")
        print("    at rest          \(where_(reader))")

        // The transcript's own scroll view, and not the results list beside it: both are `List`s
        // and therefore both are `NSScrollView`s, so they are told apart by where they are. This
        // runs with a conversation open, which makes the drawer the rightmost of the two.
        guard let scroll = rightmostScrollView(window) else {
            print("    (no scroll view found in the drawer — nothing to drive)")
            return
        }
        let document = scroll.documentView?.frame.height ?? 0
        let visible = scroll.contentView.bounds.height
        print("    document         \(fmt(document)) pt over \(reader.minimap.blocks.count) "
            + "messages, \(fmt(visible)) pt of it on screen")
        // The same two numbers as SwiftUI publishes them, which is the check on the seam this
        // bead put in: the box is drawn from `onScrollGeometryChange` and the fling is driven
        // through AppKit, so a disagreement here would mean the box is measuring another view.
        print("    …as the list says  \(fmt(reader.viewport.document)) pt document, "
            + "\(fmt(reader.viewport.height)) pt viewport, at \(fmt(reader.viewport.offset)) pt")

        // Half the document in 60 steps, which is a fling rather than a nudge: the box has to
        // survive rows arriving and leaving faster than a person could ask for them.
        frames?.reset()
        let travel = max(0, document - visible) / 2
        var trace: [Double] = []
        for step in 0...60 {
            scroll.contentView.scroll(to: NSPoint(x: 0, y: travel * Double(step) / 60))
            scroll.reflectScrolledClipView(scroll.contentView)
            try? await Task.sleep(for: .milliseconds(8))
            trace.append(reader.visible?.lowerBound ?? 0)
        }
        try? await Task.sleep(for: .milliseconds(400))
        print("    after scrolling  \(where_(reader)) — \(MinimapBands.renders) canvas renders")
        print("    box over the fling  \(continuity(trace))")

        // The acceptance criterion a PNG cannot carry, and the fling above is too coarse to make
        // it: half a document in 60 steps moves a third of a viewport per step, which would move
        // a box that could only sit on message boundaries too. So the same trace at a resolution
        // finer than a message — twenty nudges of 6 pt, which is a fraction of every row in this
        // conversation. A box drawn from which rows exist stands still for all twenty and then
        // jumps; a box drawn from the viewport moves on every one.
        let start = scroll.contentView.bounds.origin.y
        var nudges: [Double] = [reader.visible?.lowerBound ?? 0]
        for step in 1...20 {
            scroll.contentView.scroll(to: NSPoint(x: 0, y: start + Double(step) * 6))
            scroll.reflectScrolledClipView(scroll.contentView)
            try? await Task.sleep(for: .milliseconds(24))
            nudges.append(reader.visible?.lowerBound ?? 0)
        }
        print("    box over 20 × 6 pt  \(continuity(nudges))")
        print("    marked text      \(marked(reader)) since the drawer opened")
        if let frames { print("    main-thread lag  \(line(frames.lag)), "
            + "\(frames.missed) of \(frames.lag.count) vsyncs missed") }
        let scrolled = path.replacingOccurrences(of: ".png", with: "-scrolled.png")
        print("    \(scrolled) \(capture(window, to: scrolled))")

        // And the other direction: a drag three quarters of the way down the map. The transcript
        // has to be somewhere else afterwards, and the box has to have followed it there — which
        // is both halves of the relationship in one gesture, since the box is drawn from the rows
        // the scroll produced and not from the request that caused it.
        frames?.reset()
        reader.scrub(to: 0.75)
        try? await Task.sleep(for: .milliseconds(700))
        reader.endScrub()
        print("    after a drag to 75%  \(where_(reader))")
        if let frames { print("    main-thread lag  \(line(frames.lag)), "
            + "\(frames.missed) of \(frames.lag.count) vsyncs missed") }
        // The keyboard's half, which has no pointer to drive it. It shares everything below the
        // gesture with the drag, so what this checks is narrow and is the only check there is:
        // that a step resolves at all, and that it resolves to the next message the transcript has
        // a row for rather than to the next message in the conversation — 952 of which have none.
        for _ in 0..<3 {
            reader.step(by: 1)
            try? await Task.sleep(for: .milliseconds(200))
        }
        print("    after three steps down  \(where_(reader))")
        print("    marked text      \(marked(reader)) by the end of the pass")
        print("    \(footprintMB()) MB in this process, with the conversation open and scrolled")
        await tallBlockPass(reader: reader)
        let scrubbed = path.replacingOccurrences(of: ".png", with: "-scrubbed.png")
        print("    \(scrubbed) \(capture(window, to: scrubbed))")
    }

    /// The half of `chat-search-me9.8.18`'s second cost that could be paid, driven at the only
    /// message where it is worth anything.
    ///
    /// "A drag lands on a message boundary" was stated as "on this conversation a fifth of a
    /// point of slop; on a short one with tall blocks it is the block", and the second clause is
    /// the whole complaint: a message that fits on screen is already entirely on screen once its
    /// top is, so landing on its boundary loses nothing. So this drags 60% of the way into the
    /// longest drawn message in the conversation and prints where that put the transcript. It is
    /// also the only thing that exercises the two-step path — the anchor arithmetic needs the
    /// message's height, a message the list has not laid out has none, and the request is
    /// re-issued when the row finally reports one.
    @MainActor
    private static func tallBlockPass(reader: ReaderModel) async {
        let blocks = reader.minimap.blocks
        guard let biggest = blocks.indices.filter({ blocks[$0].drawn })
            .max(by: { blocks[$0].text.utf8.count < blocks[$1].text.utf8.count })
        else { return }
        reader.scrub(to: reader.minimap.fraction(at: biggest, within: 0.6))
        try? await Task.sleep(for: .milliseconds(900))
        let height = reader.placed[blocks[biggest].id]?.height
        let tall = height.map { $0 > reader.viewport.height } ?? false
        print("    drag 60% into message \(biggest), the longest drawn at "
            + "\(blocks[biggest].text.utf8.count) bytes → "
            + (height.map { "\(fmt($0)) pt against a \(fmt(reader.viewport.height)) pt viewport, "
                + (tall ? "so the anchor is the pointer's own fraction" : "so its top is its "
                    + "whole extent and .top is the only place to land") } ?? "not laid out"))
        print("      \(where_(reader)), \(footprintMB()) MB")
        reader.endScrub()
    }

    /// What the drawer spent turning marks into `AttributedString`s, and what it did not.
    ///
    /// The sum is what a computed property on the row would have assembled in full, because that
    /// is what every one of these calls used to be — so the second number is the work this run
    /// avoided rather than an inference off a percentile (`chat-search-me9.8.29`).
    @MainActor
    private static func marked(_ reader: ReaderModel) -> String {
        let table = reader.markedText
        return "\(table.builds) messages built, \(table.reuses) of "
            + "\(table.builds + table.reuses) evaluations answered from the table"
    }

    /// Where the transcript is, measured rather than reported.
    ///
    /// The messages are printed with the fraction of each that the viewport's edge cuts through,
    /// because that is the whole of what `chat-search-me9.8.27` changed: under `onAppear` the box
    /// was the *prepared* rows, which is a superset, and its edges could only ever land on a
    /// message boundary. `.3` of message 1810 is a number the old path could not produce.
    @MainActor
    private static func where_(_ reader: ReaderModel) -> String {
        guard let span = reader.visibleMessages else {
            return "nothing has been laid out yet"
        }
        // The request is printed beside the result because they are different facts: a drag names
        // a message and an anchor, and where the list put it is the answer to that.
        let requested: String? = reader.scrollRequest?.id
        let asked = requested.flatMap { reader.minimap.position(of: $0) }
        return "messages \(fmt2(Double(span.top) + span.topWithin))–"
            + "\(fmt2(Double(span.bottom) + span.bottomWithin)) of "
            + "\(reader.minimap.blocks.count) on screen (\(reader.placed.count) rows held), "
            + "box \(pct(reader.visible?.lowerBound ?? 0))–\(pct(reader.visible?.upperBound ?? 0))"
            + (asked.map { ", last asked for \($0) at anchor "
                + "\(fmt2(Double(reader.scrollRequest?.anchor.y ?? 0)))" } ?? "")
    }

    /// A fraction of the map as a percentage with a decimal, which is the resolution the claim
    /// needs: a box that moves in message-sized jumps on this conversation moves by 0.04% at a
    /// time, and a whole number would round every one of them away.
    private static func pct(_ fraction: Double) -> String {
        String(format: "%.2f%%", fraction * 100)
    }

    /// How the box moved over a fling: how many steps of it moved the box at all, and the largest
    /// single move. Both are needed. A box that never stands still could still be jumping, and a
    /// box with a small maximum could still be standing still for most of the scroll.
    private static func continuity(_ trace: [Double]) -> String {
        let steps = zip(trace, trace.dropFirst()).map { abs($1 - $0) }
        guard !steps.isEmpty else { return "nothing to trace" }
        let moved = steps.filter { $0 > 0 }.count
        let largest = steps.max() ?? 0
        // Four places on the largest move and two on the positions, because they are different
        // sizes: the box sits at a percentage and it moves by thousandths of one. Rounding the
        // second to the first's resolution prints 0.00% for a box that in fact tracked every
        // nudge, which is the claim being made and would read as its opposite.
        return "\(pct(trace.first ?? 0)) → \(pct(trace.last ?? 0)), moved on \(moved) of "
            + "\(steps.count) steps, largest single move "
            + String(format: "%.4f%%", largest * 100)
    }

    private static func fmt2(_ value: Double) -> String { String(format: "%.2f", value) }

    /// The rightmost scroll view in the window — which pane that is depends on when you ask.
    ///
    /// With a conversation open it is the drawer, with the results list to its left; before one is
    /// opened it is the results list, with only the facet rail to its left. Both callers state
    /// which of the two they are after and both depend on their own timing to get it, which is a
    /// weaker test than asking SwiftUI — and asking SwiftUI is not on offer.
    @MainActor
    private static func rightmostScrollView(_ window: NSWindow) -> NSScrollView? {
        var found: [NSScrollView] = []
        func walk(_ view: NSView?) {
            guard let view else { return }
            if let scroll = view as? NSScrollView { found.append(scroll) }
            for sub in view.subviews { walk(sub) }
        }
        walk(window.contentView)
        return found.max { $0.convert($0.bounds, to: nil).minX < $1.convert($1.bounds, to: nil).minX }
    }

    /// Resident footprint, the same `phys_footprint` `poc/swift`'s `Metrics.swift` reports — the
    /// number macOS bills this process for, so the figure is comparable with the 5.2 MB and 65.6 MB
    /// `chat-search-me9.22` measured the three containers at.
    private static func footprintMB() -> String {
        var info = task_vm_info_data_t()
        var count = mach_msg_type_number_t(
            MemoryLayout<task_vm_info_data_t>.size / MemoryLayout<natural_t>.size)
        let result = withUnsafeMutablePointer(to: &info) {
            $0.withMemoryRebound(to: integer_t.self, capacity: Int(count)) {
                task_info(mach_task_self_, task_flavor_t(TASK_VM_INFO), $0, &count)
            }
        }
        guard result == KERN_SUCCESS else { return "?" }
        return fmt(Double(info.phys_footprint) / 1024 / 1024)
    }

    /// The window's own view hierarchy as a PNG, with no window server in it.
    @MainActor
    private static func capture(_ window: NSWindow, to path: String) -> String {
        capture(window.contentView, to: path)
    }

    /// The same, for a view held directly. The settings window is reached as a view rather than as
    /// a window because the run that photographs it also reads the appearance back off it, and
    /// those have to be the same object or the picture and the reading are about two things.
    @MainActor
    private static func capture(_ view: NSView?, to path: String) -> String {
        guard let view, let rep = view.bitmapImageRepForCachingDisplay(in: view.bounds)
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

    /// Where the cursor ended up, which is the half of the fold no picture shows: the acceptance
    /// this was built to is that folding never leaves it on a row nobody can see.
    private static func describe(_ cursor: Cursor?) -> String {
        switch cursor {
        case .row(let id): "a row (\(id))"
        case .head(let key): "a group head (\(key))"
        case nil: "nothing"
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
