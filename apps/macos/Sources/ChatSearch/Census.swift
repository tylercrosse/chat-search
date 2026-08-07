import CsKit

/// What the corpus holds for the groups of one axis — the number a head cannot count for itself.
///
/// A group head counts **the rows in hand**: the `--limit` window of a ranked answer, 60 rows, and
/// not the corpus (`chat-search-me9.8.15`). `poc/ui` prints `114 · 30 here` on a project row
/// because it groups an export of the whole index; nothing on `cs search --json` carries the size
/// of a `cwd` or of a run, so a head here reading 39 cannot tell a project with 39 conversations
/// from one with 400 that this query touched 39 of.
///
/// `cs facets` is the one reply that counts the corpus rather than the answer, which makes it the
/// only place a corpus-true head can come from without a new field on the search envelope. **It
/// reaches one of the three axes**, and that was measured rather than assumed — on the live index
/// (4,426 conversations, 128 directories), 2026-08-06:
///
/// | axis | groups it could place | the rail placed |
/// | --- | --- | --- |
/// | `project`, `borrow checker` | 11 directories | 1 |
/// | `project`, `swiftui` | 4 directories | 2 |
/// | `source`, both queries | every source | every source |
///
/// The residue head is out of that first column on both project rows — the rows with no `cwd` are
/// an absence rather than a directory, and no chip could place them however long the rail got.
///
/// The `dir:` rail is the twelve busiest directories out of a hundred and twenty-eight and it is a
/// rail on purpose (`chat-search-6eb.26`): the tail of that distribution is worktrees and
/// per-conversation scratch directories, which is most of what a grouped answer turns out to be
/// made of. Sources are config ∪ index, so every source an answer contains has a chip by
/// construction and that axis is corpus-true for free.
///
/// So `project` waits for a count put on the wire on purpose (`chat-search-me9.8.43`) and says
/// which number it is meanwhile, and `run` says which number it is forever.
///
/// **A run has no corpus-wide identity to count.** Its boundaries are gaps in the `ended_at` of
/// *the rows that matched*, so it is a shape of this answer and of no other. `cs timeline` can say
/// how many conversations ended inside one run's span — it counts the corpus rather than the page
/// — but that is a different set wearing the same dates: the run would have run on past its own
/// edges had those conversations been in it. A number under `of` claims membership, and that one
/// would be claiming it falsely.
struct Census: Sendable {
    /// Conversations per source id, corpus-wide and query-blind: `SourceChip.conversations` is
    /// what the index holds for a source, not what this query matched in it.
    private let bySource: [String: Int]

    init(_ rail: FacetRail) {
        // Two chips for one source id would be a broken census rather than a state worth
        // modelling, and `Dictionary(uniqueKeysWithValues:)` traps on one. The first wins: a count
        // beside a name is not worth losing the window over.
        bySource = Dictionary(
            rail.sources.values.map { ($0.value, $0.conversations) },
            uniquingKeysWith: { first, _ in first })
    }

    /// The corpus count for each group key on this axis, or nil where there is no such number.
    ///
    /// Nil twice over and for two different reasons, which is why this is a `switch` on the axis
    /// rather than a lookup that comes back empty: `project`'s number exists and is not on this
    /// wire, `run`'s does not exist at all. A head cannot tell those apart and does not need to —
    /// both mean the same thing on screen — but the day the first one arrives, only one of these
    /// cases moves.
    func totals(for axis: Grouping) -> [String: Int]? {
        switch axis {
        case .source: bySource
        case .project, .run, .none: nil
        }
    }
}
