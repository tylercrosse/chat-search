import Foundation

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

    /// Prints the readings and returns an exit status: 0 if every measurement holds.
    public static func run(_ theme: Theme, log: (String) -> Void = { print($0) }) -> Int32 {
        var ok = true
        log("\(theme.name)")

        for (label, palette) in [("dark", theme.dark), ("light", theme.light)] {
            log("  \(label)")

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
            ok = ok && even && onTarget
            log(
                "      kind ramp on the track           "
                    + ratios.map { format($0, 2) }.joined(separator: " ")
                    + "   steps " + steps.map { format($0, 2) + "x" }.joined(separator: " ")
                    + (even && onTarget ? "  ok" : onTarget ? "  UNEVEN" : "  OFF TARGET"))

            // The three tool sub-shades have to stay inside the tool band: they say "something
            // changed here", and the primary read — tool against prose against reasoning — has to
            // survive them.
            let acts = [ColorToken.actLook, .actRun, .actChange].map {
                palette[$0].contrast(against: track)
            }
            let inside = acts[0] < acts[1] && acts[1] < acts[2] && acts[2] < ratios[1]
            ok = ok && inside
            log(
                "      act shades inside the tool band  "
                    + acts.map { format($0, 2) }.joined(separator: " ")
                    + (inside
                        ? "       ordered, under reasoning  ok" : "       OUT OF BAND"))

            for (name, fg, bg, floor) in floors {
                let ratio = palette[fg].contrast(against: palette[bg])
                let holds = ratio >= floor
                ok = ok && holds
                log(
                    "      \(name.padding(toLength: 34, withPad: " ", startingAt: 0))"
                        + "\(pad(format(ratio, 2), 6)):1  "
                        + (holds ? "ok" : "UNDER \(floor)"))
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
            log("      the quiet tier's harder ground   \(harder)")
        }

        log(ok ? "\nthe shipped direction holds." : "\nFAILED — see the lines marked above.")
        return ok ? 0 : 1
    }

    private static func format(_ value: Double, _ places: Int) -> String {
        String(format: "%.\(places)f", value)
    }

    private static func pad(_ text: String, _ width: Int) -> String {
        text.count >= width ? text : String(repeating: " ", count: width - text.count) + text
    }
}
