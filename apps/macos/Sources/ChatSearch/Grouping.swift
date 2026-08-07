import CsKit
import CsTheme
import Foundation

/// The axis one list is cut along — and it is one list, not four views.
///
/// `poc/ui` had Search, Projects and Sittings drawing the same rows, from the same filters, into
/// the same drawer: two of the three were `GROUP BY` wearing the costume of a place, and nothing
/// was in one and not the others (`chat-search-4ar.10`, `poc/ui/NOTES.md` §2). Collapsing them
/// into a setting on the list freed a second tab for the only surface that is *not* derived from
/// the index — and `chat-search-me9.8.30` then scrapped that tab too, so there is one view and no
/// switch above it. `LibraryView` says what became of the other one.
///
/// Each axis states what it costs, because the corpus is the argument for every one of them and
/// none of those numbers is guessable from the screen. `note` is what the key line above the list
/// prints.
enum Grouping: String, CaseIterable, Identifiable, Sendable {
    case none
    case project
    case run
    case source

    var id: String { rawValue }
    var label: String { rawValue }

    /// The measurement behind the axis, from `poc/ui/NOTES.md` §1. On screen rather than in a
    /// comment: an axis that can place a third of the corpus and one that can place all of it
    /// produce screens that look alike, and that difference is the whole reason there is a
    /// residue group.
    var note: String {
        switch self {
        case .none: "ranked, ungrouped"
        case .project: "cwd · 34% of conversations, 92% of messages"
        // The widest signal there is, and not 100%: `ended_at` is nullable on the wire and null on
        // 2 conversations. Printing "covers 100%" beside a residue group counting the ones it did
        // not cover is the exact screen this axis's residue exists to produce.
        case .run: "ended_at · every conversation but the 2 with no last message"
        case .source: "archetype is ~87% predicted by it"
        }
    }

    /// What the rows this axis cannot place are called, and why there are any.
    ///
    /// **The residue is information, not an edge case.** `cwd` covers 34% of the corpus — codex
    /// and claude-code 100%, chatgpt and gemini-cli 0% — so a project grouping that quietly kept
    /// only the rows it could place would drop two thirds of the list and look complete doing it.
    /// The prototype makes exactly that mistake on `project` and gets it right on `topic`; this is
    /// the prototype's own rule applied to every axis (`chat-search-me9.8.4`).
    var residue: (label: String, why: String)? {
        switch self {
        case .none:
            nil
        case .project:
            (
                "no working directory",
                "codex and claude-code carry one, chatgpt and gemini-cli never do"
            )
        case .run:
            ("no last message", "nothing to place them in time — 2 conversations in the corpus")
        // A conversation always names the source it was imported from, so this axis has no
        // residue at all. Absent because there is none, not because it is unhandled.
        case .source:
            nil
        }
    }

    /// The rows gathered along this axis, in the order they are drawn.
    ///
    /// **Nothing is re-ranked.** The rows arrive best first and that order *is* the answer, so
    /// `project` and `source` gather in the order each key first appears: the group holding the
    /// top row leads, and inside a group the rows keep the order they came in. `run` is the one
    /// axis that re-orders, because a cluster in time cannot be found in rank order — an axis
    /// that says so on its face.
    ///
    /// The residue group is last wherever there is one. It makes no rank claim: two thirds of a
    /// list can land in it, and leading with it would bury the axis you asked for.
    ///
    /// The census is what the corpus holds for these keys, where anything does — see `Census`. It
    /// arrives after the gather rather than during it because it is a fact about the axis and the
    /// reply beside this one, not about the rows.
    func groups(of rows: [Conversation], against census: Census?) -> [ConversationGroup] {
        let cut: [ConversationGroup] = switch self {
        case .none:
            []
        case .project:
            // The `cwd` column verbatim, not a project derived from it. `chat-search-6eb.26`
            // measured what deriving one costs — basenames collide, and the nearest `.git`
            // ancestor reads the live filesystem and so breaks ADR 1 — and closed saying the
            // derivation has to be captured at import time or not at all. So this axis groups the
            // one thing that is on the wire, which is also the thing `dir:` already filters on.
            // The cost is measured in `poc/ui/NOTES.md` §2 and stands: the worktrees of one repo
            // are separate groups (chat-search was 11 directories) and each of Codex's
            // per-conversation scratch directories is a group of one.
            Self.gathered(
                rows, by: { $0.cwd.flatMap { $0.isEmpty ? nil : $0 } },
                label: { Display.dir($0) })
        case .source:
            Self.gathered(
                rows, by: { $0.source }, label: { Display.sourceLabel($0) },
                // The row's own mapping, read rather than copied: a second source-to-hue table
                // here is the duplication `chat-search-g6u` exists to prevent.
                hue: { Display.sourceHue($0) })
        case .run:
            Self.runs(rows)
        }
        return Self.placed(cut, against: census?.totals(for: self))
    }

    /// Attach the corpus count to every head, or to none of them.
    ///
    /// **Whole or not at all**, the same rule the head's date range is under and for the same
    /// reason: a screen where one head reads `56 of 458` and the head under it reads `39` is two
    /// numbers of different kinds side by side, and nothing on the screen says which is which.
    ///
    /// A residue group takes the whole screen back on its own. Those are the rows the axis could
    /// not place, so their key names nothing the corpus could be counted by — `no working
    /// directory` is an absence rather than a directory. The `dir:` rail does carry `undirected`,
    /// which is that absence counted corpus-wide, and it is deliberately not read here: it would
    /// be one corpus-true head under a screenful of heads counting the rows in hand.
    ///
    /// The guard is the last line of defence rather than the rule. Which axes have a corpus count
    /// is `Census.totals(for:)`'s answer and it is all-or-nothing by construction today; a census
    /// that could place *some* of an axis would leave this flickering between keystrokes as the
    /// answer moved, which is a reason to complete such a census rather than to ship it partial.
    private static func placed(
        _ groups: [ConversationGroup], against totals: [String: Int]?
    ) -> [ConversationGroup] {
        guard let totals, groups.allSatisfy({ !$0.isResidue && totals[$0.key] != nil }) else {
            return groups
        }
        return groups.map { group in
            var placed = group
            placed.corpus = totals[group.key]
            return placed
        }
    }

    /// Gather by a key each row either carries or does not, preserving arrival order.
    private static func gathered(
        _ rows: [Conversation],
        by key: (Conversation) -> String?,
        label: (String) -> String,
        hue: (String) -> ColorToken? = { _ in nil }
    ) -> [ConversationGroup] {
        var order: [String] = []
        var members: [String: [Conversation]] = [:]
        var rest: [Conversation] = []
        for row in rows {
            guard let key = key(row) else {
                rest.append(row)
                continue
            }
            if members[key] == nil {
                members[key] = []
                order.append(key)
            }
            members[key]?.append(row)
        }
        var groups = order.map {
            ConversationGroup(key: $0, label: label($0), items: members[$0] ?? [], hue: hue($0))
        }
        if !rest.isEmpty { groups.append(ConversationGroup(residue: rest)) }
        return groups
    }

    /// A run: everything that happened with no gap of `runGap` in it, newest first.
    ///
    /// A run and the old Sittings view were one primitive at two parameterisations — both cluster
    /// `ended_at`, one as a divider inside a project and one as a third of the navigation — so
    /// there is one renderer and one word (`poc/ui/NOTES.md` §2). The cross-tool requirement
    /// sittings carried went with them: it was a display filter posing as a definition and threw
    /// most runs away, so every run is shown and the ones that crossed tools are tagged.
    private static func runs(_ rows: [Conversation]) -> [ConversationGroup] {
        let dated = rows.filter { $0.endedAt != nil }
            .sorted { ($0.endedAt ?? 0) > ($1.endedAt ?? 0) }
        var groups: [ConversationGroup] = []
        var current: [Conversation] = []

        func flush() {
            guard !current.isEmpty else { return }
            groups.append(ConversationGroup(run: current))
            current = []
        }

        for row in dated {
            if let last = current.last, (last.endedAt ?? 0) - (row.endedAt ?? 0) >= runGap {
                flush()
            }
            current.append(row)
        }
        flush()

        // `ended_at` is the widest signal the corpus has and it is still nullable: 2 conversations
        // never had a last message. They are in the answer, so they are on the screen.
        let undated = rows.filter { $0.endedAt == nil }
        if !undated.isEmpty { groups.append(ConversationGroup(residue: undated)) }
        return groups
    }

    /// Where one run ends and the next begins, measured rather than picked. `poc/ui/NOTES.md` §1:
    /// the 3-day rule left chat-search, meety-local and dev/career each as one undivided run,
    /// while 12h gives 1–15 runs and tracks the day count exactly — chat-search 6 of 6,
    /// personal-site 4 of 4, ga 7 of 7.
    ///
    /// A duration and not a time of day, which is what keeps it clear of the local-date rule: the
    /// gap between two instants has no timezone in it, so it can be computed here where a
    /// calendar day could not (see `Display.day`).
    static let runGap = 12 * 60 * 60 * 1000
}

/// One group of the list: what it is called, what is in it, and how to read the count.
///
/// Membership is over **the rows in hand** — the `--limit` window of the ranked answer, not the
/// corpus. The prototype's project rows can print a corpus-true count (`114 · 30 here`) because it
/// groups an export of the whole index; on this wire only `corpus` below carries such a number,
/// and only on the axis `Census` can reach. Every head says which of the two it is holding, so
/// `39` never appears on its own (`chat-search-me9.8.23`).
struct ConversationGroup: Identifiable {
    /// Stable across a re-group, so a `List` section keeps its identity while you type.
    let key: String
    let label: String
    let items: [Conversation]
    /// The source hue, on the one axis whose groups are sources. Nil everywhere else, and nil for
    /// a source the palette has no hue for — never a colour chosen here.
    let hue: ColorToken?
    /// The rows this axis could not place. Drawn with a tag, counted in the open, and last.
    let isResidue: Bool
    /// A run that used more than one tool, which is the fact the old cross-tool filter threw runs
    /// away to isolate. Always false off the run axis.
    let crossTool: Bool
    /// How many conversations the corpus holds for this group, or nil when this screen has only
    /// the rows in hand to count. Written by `Grouping.placed` after the gather and never by a
    /// gather, because whether such a number exists is a fact about the axis and the rail beside
    /// the answer rather than about these rows.
    fileprivate(set) var corpus: Int?

    init(
        key: String, label: String, items: [Conversation], hue: ColorToken? = nil,
        isResidue: Bool = false, crossTool: Bool = false
    ) {
        self.key = key
        self.label = label
        self.items = items
        self.hue = hue
        self.isResidue = isResidue
        self.crossTool = crossTool
    }

    /// The residue bucket. Keyed on a string no `cwd`, source id or run can produce, so a re-group
    /// cannot collide it with a real group.
    init(residue items: [Conversation]) {
        self.init(key: "\u{0}residue", label: "", items: items, isResidue: true)
    }

    /// One run, newest first.
    init(run items: [Conversation]) {
        let days = items.compactMap(\.endedDate)
        self.init(
            // The newest instant in the run: unique among runs and stable while typing.
            key: "run:\(items.first?.endedAt ?? 0)",
            label: Display.span(from: days.min(), to: days.max()),
            items: items,
            crossTool: Set(items.map(\.source)).count > 1)
    }

    var id: String { key }

    /// The days this group spans, from the local date the core rendered. ISO dates sort as
    /// strings, so the ends of the span are `min` and `max` and no calendar is involved.
    var days: String {
        let days = items.compactMap(\.endedDate)
        return Display.span(from: days.min(), to: days.max())
    }

    /// How far apart the newest and oldest conversation in this group are, or nil under a minute.
    ///
    /// A duration between two `ended_at` instants, so there is no calendar and no timezone in it.
    /// It is not time spent working: it measures last message to last message, which is the only
    /// span this wire supports.
    var elapsed: String? {
        let stamps = items.compactMap(\.endedAt)
        guard let newest = stamps.max(), let oldest = stamps.min() else { return nil }
        return Display.elapsed(ms: newest - oldest)
    }
}
