import Foundation

// The `cs timeline --json` body, decoded. The third client contract in docs/JSON-CONTRACT.md,
// and the one this app cannot compute for itself.
//
// **A timeline drawn from the rows in hand would be wrong, and wrong invisibly.** This window
// searches at `--limit 60` and ranking is not chronological, so the page is a *biased* sample of
// exactly the axis being drawn — the top sixty of 354 matches are not sixty of them spread
// evenly through time. The picture would still look like a picture. So the counting happens in
// `cs_core::timeline` over the whole matching set, and what crosses the wire is a fixed-size
// histogram rather than an instant per conversation: the reply is the same size whatever the
// archive does, on a path that runs per keystroke.
//
// **And nothing here assembles a `date:` token**, for the reason nothing in `Facets.swift`
// assembles an `agent:` one. A chip can be handed the query text clicking it produces; a drag is
// two instants out of a continuum and cannot be enumerated that way, so the trade is made
// backwards — `CsClient.timeline(_:drag:)` hands over two instants and gets the finished line
// back. `Window::value_in`'s rules (each edge rounds outward to a whole second, a midnight is
// written as a bare date) stay in the crate that owns the grammar.

/// The distribution of one query over time, and the window it currently names.
public struct Timeline: Decodable, Sendable {
    /// Contract version, moving under the same rule as the other two replies'.
    public let v: Int
    /// The query this is a distribution of. Echoed so a client can tell which of several replies
    /// it is holding — the search, the rail and this are three processes and can land out of
    /// order.
    public let query: String
    public let ms: Double
    public let indexState: String
    /// The axis, half-open. **The corpus's dated span, not this query's**, so the coordinate
    /// system does not move under the scrubber while somebody types.
    public let from: Int
    public let until: Int
    /// The two ends as local days, ready to label with. **Rendered on the far side**, like
    /// `Group.endedDate`, because a client deriving a local day is how the same bug got made
    /// three times — and nil rather than `1970-01-01` when there is no axis to label.
    public let fromDate: String?
    public let untilDate: String?
    /// Civil days a bucket covers. Zero when there are no buckets.
    public let bucketDays: Int
    /// Source ids in the order every `Bucket.sources` counts them.
    public let sources: [String]
    /// Oldest first, abutting, covering the whole axis. Empty exactly when the index holds no
    /// dated conversation.
    public let buckets: [Bucket]
    /// Conversations the filters keep that have no ending and are therefore in no bucket.
    public let undated: Int
    /// Of what the bars draw, how many are inside `window` — free text ignored, like the bars.
    public let inRange: Int
    /// How many conversations the query selects with `--limit` ignored. The same number the
    /// search envelope calls `total`, and always settled.
    public let total: Int
    /// What to draw over the bars, or nil when the query names no window. Also nil when the only
    /// `date:` token is negated: the complement of a window is not a rectangle.
    public let window: TimeWindow?
    /// The click that clears the selection — the same shape as a rail's All chip, because it is
    /// the same thing.
    public let all: AllChip
    /// What a drag writes. Non-nil only on the reply to a drag.
    public let drag: Drag?

    /// The axis as a span, or nil when there is none to draw.
    public var span: Range<Int>? { until > from ? from..<until : nil }

    /// Where an instant sits on the axis, 0 at the left edge and 1 at the right.
    ///
    /// Clamped, because the axis is taken when the reply is built and a pointer can be anywhere:
    /// a drag that leaves the track by four points is a drag to the end of it, not an error.
    public func fraction(of ms: Int) -> Double {
        guard let span else { return 0 }
        return min(max(Double(ms - span.lowerBound) / Double(span.count), 0), 1)
    }

    /// The instant at a fraction of the axis — `fraction(of:)` the other way about, which is what
    /// turns a pointer into the two numbers `--drag` takes.
    public func instant(atFraction f: Double) -> Int {
        guard let span else { return from }
        return span.lowerBound + Int((Double(span.count) * min(max(f, 0), 1)).rounded())
    }
}

/// One bar: a span of time and what fell in it.
public struct Bucket: Decodable, Sendable, Identifiable {
    /// Half-open `[from, until)`, so consecutive buckets tile without an instant falling in both
    /// or neither.
    public let from: Int
    public let until: Int
    /// Rows surviving every filter *but* `date:`, free text ignored — "when was I working on
    /// this". The window narrows the number beside the picture and never the picture.
    public let conversations: Int
    /// Of those, the ones a term landed in — "when did this query land". Zero throughout when
    /// nothing searchable was typed, which is not the same as a browse having no answer.
    public let matches: Int
    /// `conversations` by source, parallel to `Timeline.sources` and summing to `conversations`.
    public let sources: [Int]

    /// Stable across replies for the same axis, which is what stops a redraw animating every bar
    /// into a different one.
    public var id: Int { from }
}

/// The `date:` window in force.
public struct TimeWindow: Decodable, Sendable, Equatable {
    /// **Nil is an open edge, not the end of the axis.** `date:<7d` reaches to now and past it,
    /// and a rectangle clamped to `Timeline.until` would stop where the data stops rather than
    /// where the filter does.
    public let from: Int?
    public let until: Int?
    /// The window as a `date:` value — `2026-07-28..2026-08-02`. What a reader would have typed,
    /// which is why the header prints this rather than formatting the two instants itself: a
    /// client deriving a local day is the shape of the bug `cs_core::time` exists to have fixed.
    public let value: String?
}

/// What a drag writes into the query line.
public struct Drag: Decodable, Sendable {
    /// The `date:` value the two instants are typed as, or nil when they name no span.
    public let value: String?
    /// The whole query text after the drag. Put it in the box; do not splice a token.
    public let query: String
}
