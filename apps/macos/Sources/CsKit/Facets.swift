import Foundation

// The `cs facets --json` body, decoded. The second client contract in docs/JSON-CONTRACT.md,
// and the one this app could not do without: docs/TUI-DESIGN.md §5 requires a facet bar to be a
// projection of the query text rather than a selection kept beside it, and the rules that
// rewrite that text live in `cs_core::query` with the grammar. This process cannot call them, so
// every chip arrives carrying the whole query text clicking it produces.
//
// **Nothing here assembles an `agent:` token.** That is the point. A client that built its own
// would be the second, partial parser §5 costs out at six reconciliation methods in the tool it
// was lifted from, and the two parsers would agree until the day the grammar moved.
//
// Three sections, and the differences between them are all on the far side of the seam: a second
// source widens the `agent:` token, a second span *replaces* the `date:` one because two dates
// intersect, and one `dir:` fragment lights every directory beneath it. None of that is visible
// here. Every chip carries a finished query, so this file is three shapes and no rules.

/// The rail for one query.
public struct FacetRail: Decodable, Sendable {
    /// Contract version, as on the search envelope and moving under the same rule.
    public let v: Int
    /// The query this is a projection of. Echoed so a client can tell which of several replies
    /// it is holding — the rail and the results are two processes and can land out of order.
    public let query: String
    /// What is at the index path. **This reply is never a refusal**, unlike a search: on a first
    /// run the true rail is every configured source at zero, which is the state a client most
    /// needs to draw and the one an error would hide. This is what says the counts are
    /// provisional.
    public let indexState: String
    public let sources: SourceFacet
    /// The `date:` and `dir:` rails, which arrived after `sources` did (`chat-search-1ld`).
    ///
    /// Optional because this app finds its own `cs` on disk and can be pointed at an older one: a
    /// section that build does not emit is one this window simply does not draw, where a required
    /// key would cost the whole rail — sources included — on a decode error. The same reading as
    /// `ChipState.unrecognised`, one direction on.
    public let dates: DateFacet?
    public let dirs: DirFacet?
}

/// The `date:` rail: the All chip, then the spans, newest first.
public struct DateFacet: Decodable, Sendable {
    /// `date:` — the token these chips write.
    public let keyword: String
    public let all: AllChip
    public let values: [DateChip]
}

/// One span: how far back it reaches, what is in it, and the click.
public struct DateChip: Decodable, Sendable, Identifiable {
    /// The `date:` value the query text will carry — `today`, `>1mo`.
    public let value: String
    /// The same span in words. Carried rather than mapped here because `>1mo` is syntax, and a
    /// table of four English phrases in this file would be the rail's rules written twice.
    public let label: String
    public let state: ChipState
    /// What the index holds inside this span. The spans nest, so these do not sum to the corpus:
    /// each answers "how many if I go back this far".
    public let conversations: Int
    /// The whole query text after clicking this chip. It **replaces** whatever `date:` was there
    /// — two date tokens intersect, so a rail that added beside would narrow to the overlap — and
    /// that rule is the grammar's, which is why this arrives finished.
    public let query: String

    public var id: String { value }
}

/// The `dir:` rail: the All chip, then the busiest directories the index holds.
public struct DirFacet: Decodable, Sendable {
    /// `dir:` — the token these chips write.
    public let keyword: String
    public let all: AllChip
    /// Busiest first. Not every directory: see `indexed`.
    public let values: [DirChip]
    /// Distinct directories in the index, which may be more than there are chips.
    public let indexed: Int
    /// Conversations that record no directory at all, which no `dir:` token can reach. Most of
    /// this corpus — only the agent sources have a working directory — so a rail that did not say
    /// so would read as a filter over everything (`chat-search-6eb.26`).
    public let undirected: Int
}

/// One directory: the census fact, the query fact, and the click.
public struct DirChip: Decodable, Sendable, Identifiable {
    /// The `cwd` as recorded, which is what `dir:` selects on. A path and not a project name:
    /// deriving one collapsed seven unrelated directories onto a single label, and the
    /// alternative read the live filesystem (`chat-search-6eb.26`). Shortening it for display is
    /// this side's business; naming it is nobody's.
    public let value: String
    public let state: ChipState
    public let conversations: Int
    /// The whole query text after clicking this chip.
    public let query: String

    public var id: String { value }
}

/// The `agent:` rail: the All chip, then one chip per source the machine knows about.
public struct SourceFacet: Decodable, Sendable {
    /// `agent:` — the token these chips write. Carried so a section can be labelled without this
    /// app hard-coding the grammar it is a rail for.
    public let keyword: String
    public let all: AllChip
    /// Ordered by source id, so the rail does not reshuffle between runs.
    public let values: [SourceChip]
}

/// The All chip: no `agent:` token at all.
public struct AllChip: Decodable, Sendable {
    /// True exactly when the query names no source, which is the only state in which every
    /// source is in the answer.
    public let selected: Bool
    /// The query text with every `agent:` token gone, exclusions included.
    public let query: String
}

/// One source chip: the census fact, the query fact, and the click.
public struct SourceChip: Decodable, Sendable, Identifiable {
    /// The source id. Permanent, and what `agent:` selects on (ADR 16).
    public let value: String
    public let state: ChipState
    public let coverage: Coverage
    /// What the index holds for it. Zero is an answer, not an absence — read beside `coverage`.
    public let conversations: Int
    /// The whole query text after clicking this chip, not a token to splice in.
    public let query: String

    public var id: String { value }
}

/// What the query says about one chip.
///
/// Three states rather than a `Bool`, because the query has three things to say about a source
/// and an excluded one drawn like an untouched one is filtering the reader cannot see.
public enum ChipState: Decodable, Sendable, Equatable {
    /// `agent:codex` — the query keeps this source.
    case include
    /// `-agent:codex` or `agent:!codex` — the query drops it.
    case exclude
    /// Neither. Not the same as "everything is included": that is every chip `off` *and*
    /// `SourceFacet.all.selected`.
    case off
    /// A state this build has no reading for. Drawn as `off`, never guessed at.
    case unrecognised(String)

    public init(from decoder: any Decoder) throws {
        switch try decoder.singleValueContainer().decode(String.self) {
        case "include": self = .include
        case "exclude": self = .exclude
        case "off": self = .off
        case let other: self = .unrecognised(other)
        }
    }
}

/// Where a source stands with the config and this disk, as `cs status` reports it.
///
/// The rail is the only place this is visible, and that is what it is for: a configured source
/// holding nothing is a broken importer or an archive run that never happened, and a source
/// nobody configured is a tool nobody uses. Drawn from the index alone the two are one absence —
/// you search, get nothing, and conclude you used a different tool (`chat-search-a7k.29`).
public enum Coverage: Decodable, Sendable, Equatable {
    /// Configured and its directory is here. Whether capture has *worked* is the count beside it.
    case live
    /// Configured, but the directory is gone. What was archived is safe; nothing new arrives.
    case missing
    /// Its directory is here and no `[[sources]]` entry claims it, so conversations are
    /// accruing uncaptured right now.
    case unconfigured
    /// Neither configured nor on this disk, and the index holds rows for it. Searchable, and
    /// never growing again.
    case retired
    /// A cell this build has no reading for. An added state should cost a line of prose on
    /// screen, not a coordinated release.
    case unrecognised(String)

    public init(from decoder: any Decoder) throws {
        switch try decoder.singleValueContainer().decode(String.self) {
        case "live": self = .live
        case "missing": self = .missing
        case "unconfigured": self = .unconfigured
        case "retired": self = .retired
        case let other: self = .unrecognised(other)
        }
    }
}
