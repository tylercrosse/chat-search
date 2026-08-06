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

        // The second view. Empty by design and not by accident — there is no store to author into
        // (`chat-search-6eb.14`) — so what there is to check is that every shelf says which.
        model.surface = .library
        try? await Task.sleep(for: .milliseconds(400))
        let library = path.replacingOccurrences(of: ".png", with: "-library.png")
        print("  library → \(capture(window, to: library)) \(library)")
        model.surface = .search
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

    /// A key equivalent as AppKit would deliver it, minus the hardware.
    private static func press(_ character: String, code: UInt16) -> NSEvent {
        NSEvent.keyEvent(
            with: .keyDown, location: .zero, modifierFlags: .command, timestamp: 0,
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
        // and therefore both are `NSScrollView`s, so they are told apart by where they are. The
        // drawer is the right-hand pane, which makes it the rightmost of the two — a weaker test
        // than asking SwiftUI, which does not answer.
        guard let scroll = readerScrollView(window) else {
            print("    (no scroll view found in the drawer — nothing to drive)")
            return
        }
        let document = scroll.documentView?.frame.height ?? 0
        let visible = scroll.contentView.bounds.height
        print("    document         \(fmt(document)) pt over \(reader.minimap.blocks.count) "
            + "messages, \(fmt(visible)) pt of it on screen")

        // Half the document in 60 steps, which is a fling rather than a nudge: the box has to
        // survive rows arriving and leaving faster than a person could ask for them.
        frames?.reset()
        let travel = max(0, document - visible) / 2
        for step in 0...60 {
            scroll.contentView.scroll(to: NSPoint(x: 0, y: travel * Double(step) / 60))
            scroll.reflectScrolledClipView(scroll.contentView)
            try? await Task.sleep(for: .milliseconds(8))
        }
        try? await Task.sleep(for: .milliseconds(400))
        print("    after scrolling  \(where_(reader)) — \(MinimapBands.renders) canvas renders")
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
        print("    \(footprintMB()) MB in this process, with the conversation open and scrolled")
        let scrubbed = path.replacingOccurrences(of: ".png", with: "-scrubbed.png")
        print("    \(scrubbed) \(capture(window, to: scrubbed))")
    }

    /// Where the transcript is, in the only terms `List` can express it.
    @MainActor
    private static func where_(_ reader: ReaderModel) -> String {
        let positions = reader.onScreen.compactMap { reader.minimap.position(of: $0) }.sorted()
        guard let first = positions.first, let last = positions.last else {
            return "no rows have reported themselves on screen"
        }
        // The request is printed beside the result because they are different facts: `List` keeps a
        // row prepared past the edge of the viewport, so a step of one message can move the
        // transcript without moving the top of the box at all.
        let requested: String? = reader.scrollRequest?.id
        let asked = requested.flatMap { reader.minimap.position(of: $0) }
        return "messages \(first)–\(last) of \(reader.minimap.blocks.count) on screen "
            + "(\(positions.count) rows), box at \(Int((reader.scrollFraction * 100).rounded()))%"
            + (asked.map { ", last asked for \($0)" } ?? "")
    }

    /// The drawer's scroll view: the rightmost one in the window.
    @MainActor
    private static func readerScrollView(_ window: NSWindow) -> NSScrollView? {
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
