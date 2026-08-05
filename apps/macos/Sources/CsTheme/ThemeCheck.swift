import Foundation

/// Where a set of token values came from, which is the only thing that decides what a missed
/// measurement costs.
///
/// Provenance and not content. A direction is compiled into this binary; a user theme was read off
/// a disk at launch. Nothing *inside* a token set can claim to be either, and that is deliberate:
/// the class the project enforces has to be one a theme cannot assert about itself, or the first
/// theme that fails the fence declares itself exempt from it. `docs/DECISIONS.md` ADR 25.
public enum ThemeClass: Sendable {
    /// Shipped with the app and offered in the picker. Fenced — `--verify-theme` is the gate, and
    /// a direction that misses these measurements does not ship. `chat-search-me9.8.9`.
    case direction

    /// Loaded at launch from a file a person wrote. Measured by exactly the same code and drawn
    /// whatever the readings say, but never silently: the failures are printed and the name
    /// carries the class wherever the app names a theme. `chat-search-me9.8.10`.
    case userTheme

    /// How the check names it. Read by anything that shows a theme's name beside its class.
    public var label: String {
        switch self {
        case .direction: "direction"
        case .userTheme: "user theme"
        }
    }
}

/// Re-measures a theme against the measurements that fence it, and says so.
///
/// This is `poc/ui/palette.py --verify` on the Swift side, and it exists for the same reason:
/// solving a colour and writing it down are two events, and the stylesheet only ever checked the
/// first. `Tokens.swift` adds a third event — a generator — so what shipped is now two steps from
/// what was solved, and neither step is checked by anything that reads Swift.
///
/// It is a subcommand rather than a test because there are no tests to put it in: this package
/// builds against the Command Line Tools SDK, where neither `Testing` nor `XCTest` exists, so
/// `swift test` cannot run at all. `swift run -c release cs-spike contract` is the same shape for
/// the same reason, and it is the only thing in the repo that catches a JSON contract break.
///
/// ## The same readings, two different consequences
///
/// A missed measurement means one thing in a theme the project chose and another in a theme a
/// person wrote for themselves, so the class is an argument to every entry point here and there is
/// no default. `measure` takes the readings and decides nothing; `run` prints them and returns the
/// gate's exit status. A caller that draws an unfenced theme (`chat-search-me9.8.10`) wants the
/// first, and must not read the second's status as permission to draw.
public enum ThemeCheck {
    /// The four kinds against the ribbon track. An even ~1.8x per step is the widest even spacing
    /// four levels fit into the range, and evenness is the finding — hue is the channel that
    /// degrades fastest at the ~2px the bands are drawn at, so kind has to ride on luminance.
    /// See `poc/ui/NOTES.md` §3.
    static let ramp: [(ColorToken, Double)] = [
        (.kTool, 2.20), (.kReason, 4.00), (.kUser, 7.20), (.kAgent, 13.00),
    ]

    /// The AA floor for text at 9–13px. Every tier that carries information is measured, not only
    /// the one the brief happens to name: a direction that fixes `--ink-3` and breaks
    /// `--k-tool-ink` has held one token, not the finding.
    static let floors: [(String, ColorToken, ColorToken, Double)] = [
        ("body text on its ground", .ink, .bg, 4.5),
        ("second tier on its ground", .ink2, .bg, 4.5),
        ("quiet tier on the page", .ink3, .bg, 4.5),
        ("quiet tier on the drawer", .ink3, .panel, 4.5),
        ("tool names in the drawer", .kToolInk, .panel, 4.5),
        ("a selected row title", .sel, .selBg, 4.5),
        ("text inside a match", .ink, .hitBg, 4.5),
    ]

    /// Every reading, and the ones that did not hold.
    ///
    /// Two renderings of one measurement, because they are read in different places. `lines` is the
    /// table `--verify-theme` prints, aligned and complete, readings that passed included — the
    /// number that holds today is what says how much room the next nudge has. `failures` is what a
    /// load-time complaint puts on stderr: whole sentences, each naming its theme, its side and
    /// what it missed, because nobody who just launched an app has a table in front of them.
    public struct Report: Sendable {
        public let name: String
        public let themeClass: ThemeClass
        public let lines: [String]
        public let failures: [String]

        public var holds: Bool { failures.isEmpty }
    }

    /// Takes every reading and decides nothing.
    public static func measure(_ theme: Theme, as themeClass: ThemeClass) -> Report {
        var lines: [String] = []
        var failures: [String] = []
        // One reading: the table line always, and a sentence as well when it did not hold.
        func reading(_ line: String, missed: String? = nil) {
            lines.append(line)
            if let missed { failures.append(missed) }
        }

        reading("\(theme.name) · \(themeClass.label)")

        for (side, palette) in [("dark", theme.dark), ("light", theme.light)] {
            reading("  \(side)")

            // Both themes are complete authored sets rather than one derived from the other, and
            // this is where that is worth something: on a light track contrast is distance from
            // white, so the light ramp is reached by going *darker*. A theme computed from the
            // other would not hold these ratios, and would fail here rather than on a screen.
            let track = palette[.mapBg]
            let ratios = ramp.map { palette[$0.0].contrast(against: track) }
            let steps = zip(ratios, ratios.dropFirst()).map { $1 / $0 }
            let even = steps.allSatisfy { abs($0 - 1.8) < 0.06 }
            // Proportionally, not absolutely. The incumbent's dark values were hand-solved months
            // before `palette.py` existed and land within one part in 255 of what it now computes,
            // which reads as 12.96 against a target of 13.00 — and a fixed tolerance tight enough
            // to mean anything at 2.2 would fail that. 2% catches a wrong colour, which is wrong by
            // tens of percent, and forgives eight-bit rounding, which is not a drift.
            let onTarget = zip(ratios, ramp).allSatisfy { abs($0 - $1.1) / $1.1 < 0.02 }
            let readings = ratios.map { format($0, 2) }.joined(separator: " ")
            reading(
                "      kind ramp on the track           " + readings
                    + "   steps " + steps.map { format($0, 2) + "x" }.joined(separator: " ")
                    + (even && onTarget ? "  ok" : onTarget ? "  UNEVEN" : "  OFF TARGET"),
                missed: even && onTarget
                    ? nil
                    : "\(theme.name), \(side): the kind ramp reads \(readings) against "
                        + "2.20 4.00 7.20 13.00 — "
                        + (even
                            ? "the four kinds are evenly spaced but sit off the ramp"
                            : "the four kinds are not equally far apart, and that spacing is "
                                + "what tells them apart at the 2px the ribbon draws them at"))

            // The three tool sub-shades have to stay inside the tool band: they say "something
            // changed here", and the primary read — tool against prose against reasoning — has to
            // survive them.
            let acts = [ColorToken.actLook, .actRun, .actChange].map {
                palette[$0].contrast(against: track)
            }
            let inside = acts[0] < acts[1] && acts[1] < acts[2] && acts[2] < ratios[1]
            reading(
                "      act shades inside the tool band  "
                    + acts.map { format($0, 2) }.joined(separator: " ")
                    + (inside ? "       ordered, under reasoning  ok" : "       OUT OF BAND"),
                missed: inside
                    ? nil
                    : "\(theme.name), \(side): the act shades read "
                        + acts.map { format($0, 2) }.joined(separator: " ")
                        + " and have to climb and stay under reasoning at "
                        + "\(format(ratios[1], 2)) — a look, a run and a change are one band")

            for (name, fg, bg, floor) in floors {
                let ratio = palette[fg].contrast(against: palette[bg])
                let holds = ratio >= floor
                reading(
                    "      \(name.padding(toLength: 34, withPad: " ", startingAt: 0))"
                        + "\(pad(format(ratio, 2), 6)):1  " + (holds ? "ok" : "UNDER \(floor)"),
                    missed: holds
                        ? nil
                        : "\(theme.name), \(side): \(name) is \(format(ratio, 2)):1, under the "
                            + "\(format(floor, 1)) AA floor for text this size")
            }

            // The quiet tier lands on two grounds and only ever gets measured against one. In a
            // light theme `--panel` is the darker of the two, so dark text has less to work with
            // there than on `--bg` — and `.pv-meta` and every tool and reasoning line in the
            // transcript sit on the drawer, which is where most of that text actually is. Solving
            // against `--bg` alone left the light theme at 4.23:1 on the ground that mattered.
            // Both readings are printed above; this names which one was the hard one.
            let harder =
                palette[.ink3].contrast(against: palette[.bg])
                <= palette[.ink3].contrast(against: palette[.panel]) ? "the page" : "the drawer"
            reading("      the quiet tier's harder ground   \(harder)")
        }

        return Report(name: theme.name, themeClass: themeClass, lines: lines, failures: failures)
    }

    /// Prints the readings and returns an exit status: 0 if every measurement holds.
    ///
    /// The status reports the readings and never the policy. Somebody checking their own token set
    /// before they load it is asking the same question the gate asks — does this hold — and what
    /// the app *does* about a theme that does not is the last line rather than the exit code.
    public static func run(
        _ theme: Theme, as themeClass: ThemeClass, log: (String) -> Void = { print($0) }
    ) -> Int32 {
        let report = measure(theme, as: themeClass)
        report.lines.forEach(log)
        log("\n" + verdict(report))
        return report.holds ? 0 : 1
    }

    /// The last line, which is the only part of this that differs by class.
    private static func verdict(_ report: Report) -> String {
        switch (report.themeClass, report.holds) {
        case (.direction, true):
            "\(report.name) holds — which is what a direction has to do to ship."
        case (.direction, false):
            "FAILED — see the lines marked above. A direction that misses these does not ship: "
                + "re-solve it through poc/ui/palette.py, or load it as a user theme instead "
                + "(docs/DECISIONS.md ADR 25)."
        case (.userTheme, true):
            "\(report.name) holds — it clears the same fence a shipped direction has to."
        case (.userTheme, false):
            "UNFENCED — see the lines marked above. The app draws this anyway, because you loaded "
                + "it and it is your screen; what that costs is up there (docs/DECISIONS.md "
                + "ADR 25)."
        }
    }

    private static func format(_ value: Double, _ places: Int) -> String {
        String(format: "%.\(places)f", value)
    }

    private static func pad(_ text: String, _ width: Int) -> String {
        text.count >= width ? text : String(repeating: " ", count: width - text.count) + text
    }
}
