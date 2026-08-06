import CsKit
import CsTheme
import Foundation

/// Turning a contract fact into a string a person reads — in one place, for the same reason
/// `ended_date` is rendered by the core: the local-date bug happened because three clients each
/// derived the day themselves, and every rule below is one a second view would otherwise derive
/// again slightly differently.
///
/// Each of these already exists in Rust, in `cs-tui/src/text.rs` and `cs-tui/src/theme.rs`. This
/// is a second implementation rather than a shared one, because the seam between the app and the
/// core is JSON over argv (ADR 14) and a Swift process cannot call into that crate. So the rules
/// are reproduced deliberately and named after their originals: what the terminal calls a source
/// and what the window calls it have to be the same word, or the two surfaces disagree in front
/// of somebody comparing them.
enum Display {
    /// The label for a source id. Mirrors `cs_tui::theme::source_badge`, shortening exactly the
    /// four ids that read as an import path rather than as a product — `chatgpt-export` is the
    /// directory an export was unpacked into, and `claude-ai` beside `claude-code` reads as a
    /// second flavour of the same thing at a glance.
    ///
    /// An unknown id is returned verbatim, which is that function's rule and its reasoning:
    /// inventing an abbreviation for a source nobody has seen makes the one thing carrying the
    /// identity less recognisable. `google-takeout` is the live case — 1,280 conversations, and
    /// no shortening in either surface.
    static func sourceLabel(_ source: String) -> String {
        switch source {
        case "gemini-cli": "gemini"
        case "chatgpt-export": "chatgpt"
        case "claude-ai": "claude.ai"
        case "claude-desktop": "claude.app"
        default: source
        }
    }

    /// The hue a source is drawn in, or nil for one the palette has no hue for.
    ///
    /// The mapping is the palette's own: `--src-claude-code` is named after the source id it is
    /// for, and the only place the two spellings part company is where `sourceLabel` already
    /// does. Nil rather than a fallback chosen here — the caller reads the quiet tier off the
    /// theme, because a view that invented a sixth source hue would be authoring a token.
    ///
    /// Five hues against six sources in the live index: `google-takeout` has none, which is
    /// `chat-search-me9.8.13`.
    static func sourceHue(_ source: String) -> ColorToken? {
        switch source {
        case "claude-code": .srcClaudeCode
        case "codex": .srcCodex
        case "chatgpt-export": .srcChatgpt
        case "claude-ai": .srcClaudeAi
        case "gemini-cli": .srcGemini
        default: nil
        }
    }

    /// The token a message is drawn in wherever its *band* is the thing being drawn: the spine
    /// beside it in the transcript, and its band on the minimap next to that.
    ///
    /// One function rather than two switches, because those are the same conversation at two
    /// resolutions. A message whose spine is `--err` and whose band a centimetre away is
    /// `--k-tool` would be two answers to one question on one screen, which is the shape of the
    /// local-date bug this repository already paid for once.
    ///
    /// The error test comes first for the reason the transcript gives: a failed result is the one
    /// result that survives the fold, and it is the failure rather than the tool that earned it.
    /// A band this build has no reading for takes the quiet tier rather than borrowing one of the
    /// four, because a stripe is a claim about what a conversation is made of.
    static func bandToken(_ block: Block) -> ColorToken {
        if block.isError { return .err }
        return bandToken(block.band)
    }

    /// The same answer for a band with no message under it — what the fidelity chip's dot is drawn
    /// in, since a control over a band has to be the colour of the band it controls.
    static func bandToken(_ band: Band) -> ColorToken {
        switch band {
        case .user: .kUser
        case .agent: .kAgent
        case .reasoning: .kReason
        case .tool: .kTool
        case .unrecognised: .ink3
        }
    }

    /// `$HOME` collapsed to `~`, and `—` for a conversation that has no working directory.
    ///
    /// Mirrors `cs_tui::text::display_dir`, including the trap in it: a plain prefix test turns
    /// `/Users/tylercrosse` under home `/Users/tyler` into `~crosse`, which reads as a real path
    /// and is not one. The dash is not an empty cell — two thirds of the corpus is ChatGPT and
    /// has no such thing as a working directory, which is a fact about the source rather than
    /// data that went missing.
    static func dir(_ cwd: String?, home: String = NSHomeDirectory()) -> String {
        guard let cwd, !cwd.isEmpty else { return "—" }
        var home = home
        while home.hasSuffix("/") { home.removeLast() }
        guard !home.isEmpty else { return cwd }
        if cwd == home { return "~" }
        guard cwd.hasPrefix(home + "/") else { return cwd }
        return "~" + cwd.dropFirst(home.count)
    }

    private static let minuteMs = 60_000
    private static let hourMs = 60 * minuteMs
    private static let dayMs = 24 * hourMs
    /// The mean Gregorian month, 365.2425 / 12 days, as `cs_tui::text::age` uses it. It buys what
    /// a round 30 days does not: `12 * monthMs == yearMs`, so the last month bucket is `11mo` and
    /// no `12mo` ever renders one cell away from a `1y`.
    private static let monthMs = 2_629_746_000
    private static let yearMs = 12 * monthMs

    /// Coarse age for the row's last cell: `now`, `5m`, `3h`, `12d`, `4mo`, `2y` — and `—` for
    /// the two conversations in the corpus with no last message.
    ///
    /// Relative rather than absolute, in the same buckets `cs_tui::text::age` renders. A duration
    /// has no timezone, so this cannot reproduce the UTC bug `ended_date` exists to prevent; the
    /// absolute local day is on the wire already, rendered once by the core, for the surface that
    /// wants to show it.
    static func age(endedAt: Int?, now: Date = .now) -> String {
        guard let endedAt else { return "—" }
        let ms = Int(now.timeIntervalSince1970 * 1000) - endedAt
        return switch ms {
        // A clock that moved backwards lands here rather than printing `-3m`.
        case ..<minuteMs: "now"
        case ..<hourMs: "\(ms / minuteMs)m"
        case ..<dayMs: "\(ms / hourMs)h"
        case ..<monthMs: "\(ms / dayMs)d"
        case ..<yearMs: "\(ms / monthMs)mo"
        default: "\(ms / yearMs)y"
        }
    }

    /// One local day, formatted the way `poc/ui`'s `dayLabel` formats it: `Aug 5`, and
    /// `Oct 25 ’25` once the year is not this one.
    ///
    /// **This reformats the day the core rendered and never decides which day it is.**
    /// `ended_date` arrives as a local `YYYY-MM-DD` rendered once in `cs-core`, precisely so that
    /// three clients do not each get their own chance to derive the day in UTC — the bug
    /// `AGENTS.md` names as the reason a rule lives in exactly one place. Every field below is
    /// read out of that string.
    ///
    /// The one thing it does ask a calendar is which year is *this* year, to decide whether the
    /// year is worth printing — and it asks a **Gregorian** one by name. `Calendar.current` honours
    /// the region's calendar setting, so on a Japanese, Buddhist, Hebrew or Islamic calendar it
    /// answers 8, 2569, 5786 or 1448, none of which ever equals the `YYYY` on the wire: every
    /// label in the app would carry the year suffix this line exists to suppress. A client
    /// deciding a calendar question for itself is the shape of the original bug, so the calendar
    /// is named rather than inherited.
    ///
    /// A value that is not `YYYY-MM-DD` is shown exactly as it arrived, down to the field widths.
    /// The alternative is a client that invents a day for a shape it does not recognise, which is
    /// the same failure one step further along.
    static func day(_ endedDate: String?, now: Date = .now) -> String {
        guard let endedDate else { return "—" }
        let parts = endedDate.split(separator: "-")
        guard parts.count == 3, parts[0].count == 4, parts[1].count == 2, parts[2].count == 2,
            let month = Int(parts[1]), (1...12).contains(month),
            let dayOfMonth = Int(parts[2]), (1...31).contains(dayOfMonth)
        else { return endedDate }
        // The year only when it is not this one. Eleven months of corpus means most spans sit
        // inside one year, and printing it on every label is noise in a cell that is already
        // narrow.
        let suffix = parts[0] == String(gregorian.component(.year, from: now))
            ? "" : " ’\(parts[0].suffix(2))"
        return "\(months[month - 1]) \(dayOfMonth)\(suffix)"
    }

    /// A Gregorian calendar in the machine's own zone: the zone is what makes "today" the user's
    /// today, and the identifier is what stops a region setting renumbering the year underneath it.
    private static let gregorian: Calendar = {
        var calendar = Calendar(identifier: .gregorian)
        calendar.timeZone = .current
        return calendar
    }()

    private static let months = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ]

    /// The days a group of conversations spans, both ends being local days the core rendered. One
    /// day when the two ends are the same day, which is the common case for a run.
    static func span(from earliest: String?, to latest: String?) -> String {
        guard let earliest, let latest else { return "—" }
        let start = day(earliest)
        let end = day(latest)
        return start == end ? start : "\(start) – \(end)"
    }

    /// A duration, coarse: `38m`, `6h 12m`, `3d`. Nil below a minute, where there is nothing worth
    /// saying — a group of one conversation spans no time at all.
    ///
    /// A duration and not a clock, which is what makes it computable here: the gap between two
    /// instants has no timezone in it, where `09:14 – 23:40` would be a second local-time rule in
    /// a second language (see `day` above).
    static func elapsed(ms: Int) -> String? {
        switch ms {
        case ..<minuteMs: nil
        case ..<hourMs: "\(ms / minuteMs)m"
        case ..<dayMs:
            (ms % hourMs) / minuteMs == 0
                ? "\(ms / hourMs)h" : "\(ms / hourMs)h \((ms % hourMs) / minuteMs)m"
        default: "\(ms / dayMs)d"
        }
    }

    /// The literal prefix `cs` puts on a snippet whose match it could not locate in the text.
    ///
    /// `docs/JSON-CONTRACT.md` calls it a label rather than content, so the row draws it as one:
    /// inside the quotes it would read as something the conversation said.
    static let unlocated = "⟨no match⟩ "

    /// What line 3 leads with: who or what produced the message the snippet came from.
    ///
    /// The mockup's `⚙ Bash(cargo test) ›` frame without the tool's name, which is not on the
    /// wire. Today the tool arms are also unreachable: `cs search` matches prose unless `--tools`
    /// is passed and this client never passes it, so every match arrives `user` or `assistant`
    /// prose. Written anyway because the arm costs a line, and a `tool_call` landing in a row
    /// with no frame is exactly the "naked log spew" `poc/ui/NOTES.md` §2 says it prevents.
    static func signal(role: String, kind: String) -> String {
        switch kind {
        case "tool_call", "tool_result": "⚙"
        default: role == "user" ? "▸" : "◂"
        }
    }
}
