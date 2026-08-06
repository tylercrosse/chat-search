import CsKit
import CsTheme
import Observation
import SwiftUI

/// The bottom drawer's half of the window: when this query's answers happened, and the drag that
/// narrows to a stretch of it.
///
/// `poc/ui/DESIGN-BRIEF.md:59` — "a timeline of whatever survives the current filters, with a
/// scrubber". Held beside `SearchModel` rather than inside the view because a drag has to reach
/// the query box: the selection is *derived from the query text* and never kept here, which is
/// the defect `poc/ui/NOTES.md` §17 records this prototype committing — a range in a tuple that
/// never appeared in the query line, with two rounds of another view written against it before
/// anyone noticed.
///
/// **A third process per keystroke, undebounced**, which is a measurement rather than a
/// preference. `chat-search-me9.8.20` measured `cs timeline` at 1.2× what settling the same
/// query's total costs and, through a spawn, at 140–240 ms for the worst broad prefix against
/// 280–340 ms for the `cs search` beside it. A debounce here would be staleness bought to save a
/// cost smaller than the one this window already pays undebounced — the same trade `SearchModel`
/// refuses in its own comment. Hiding the drawer stops it entirely, which is the knob that
/// matters.
@MainActor
@Observable
final class TimelineModel {
    /// The last distribution that landed, or nil before the first one.
    ///
    /// **Left standing when a later reply fails**, the same rule the facet rail follows: the axis
    /// is a fact about the archive, and a drawer that blanked on a hiccup would read as a corpus
    /// that had lost its history.
    private(set) var drawn: Timeline?
    /// Why the last attempt failed, when it did. Drawn in place of the track rather than beside
    /// it, because a picture that is one keystroke stale looks exactly like a current one.
    private(set) var failure: String?
    /// Whether the drawer is open. Session state, not a preference — the same reading as
    /// `--group` and `--folded`, and for the same reason: it is a thing somebody does to this
    /// window while looking at something, not a way they want the app to be.
    var shown: Bool

    private let client: CsClient
    private var inFlight: Task<Void, Never>?

    init(client: CsClient, shown: Bool = true) {
        self.client = client
        self.shown = shown
    }

    /// Ask again for the query now in the box.
    ///
    /// The superseded process is killed rather than waited for, the same arrangement the search
    /// and the rail use: without it, typing eight characters leaves eight `cs` processes
    /// competing for one index and the only answer anyone wants finishes last.
    ///
    /// A hidden drawer asks for nothing. That is the whole of what hiding buys, and it is why
    /// closing it cannot change the result set — nothing here has ever narrowed anything.
    func queryChanged(_ text: String) {
        inFlight?.cancel()
        guard shown else { return }
        inFlight = Task { [weak self] in
            guard let self else { return }
            do {
                let drawn = try await client.timeline(text)
                guard !Task.isCancelled else { return }
                self.drawn = drawn
                self.failure = nil
            } catch is CancellationError {
                return
            } catch {
                guard !Task.isCancelled else { return }
                self.failure = String(describing: error)
            }
        }
    }

    /// Open or shut the drawer. Opening asks for the picture it is about to draw.
    func toggle(query: String) {
        shown.toggle()
        if shown { queryChanged(query) }
    }

    /// What a drag across the track writes into the query line, as a fraction of the axis at each
    /// end.
    ///
    /// **The two instants are snapped to bucket edges**, which is this side's whole contribution
    /// to the gesture. The picture is bars, so a selection finer than a bar is a selection nobody
    /// can see — and snapping is what makes `poc/ui`'s "a drag under 1% of the span clears it"
    /// fall out of the grammar instead of out of a magic fraction: a drag that never leaves the
    /// bar it started in names no span, and `cs_core` answers a span that names nothing by
    /// clearing the filter.
    ///
    /// Everything after the snap is `cs`'s. This does not spell `date:`, does not order the two
    /// ends, and does not decide what happens when the drag lands on the window already in force.
    func text(forDrag a: Double, to b: Double) async -> String? {
        guard let drawn, !drawn.buckets.isEmpty else { return nil }
        let edges = (min(a, b), max(a, b))
        let from = drawn.buckets[bar(at: edges.0, in: drawn)].from
        let until = drawn.buckets[bar(at: edges.1, in: drawn)].from
        return try? await client.timeline(drawn.query, drag: (from, until)).drag?.query
    }

    /// Which bar a fraction of the track points at.
    private func bar(at fraction: Double, in drawn: Timeline) -> Int {
        let at = drawn.instant(atFraction: fraction)
        let after = drawn.buckets.partitioningIndex { $0.from <= at }
        return min(max(after - 1, 0), drawn.buckets.count - 1)
    }
}

extension Array {
    /// How many leading elements satisfy `belongs` — a binary search, since a drag asks this on
    /// every frame and 180 bars is not a thing to scan.
    ///
    /// Assumes the predicate is true for a prefix and false after it, which is what makes bucket
    /// edges searchable: they are sorted and they abut.
    fileprivate func partitioningIndex(_ belongs: (Element) -> Bool) -> Int {
        var low = 0
        var high = count
        while low < high {
            let middle = (low + high) / 2
            if belongs(self[middle]) { low = middle + 1 } else { high = middle }
        }
        return low
    }
}

/// The drawer: a header, a track, and an axis under it.
///
/// Three departures from the prototype, each because a histogram is not a scatter of marks.
///
/// - **A bar per bucket, not a mark per conversation.** The prototype had all 3,059 in memory;
///   this window holds sixty ranked rows, and a timeline over those is a biased sample of the
///   axis it draws (`docs/JSON-CONTRACT.md`, "The timeline").
/// - **Each half of the track is scaled to its own tallest bar.** Hits are a subset of the bars
///   below them and usually a small one — 58 matches under 3,617 conversations — so one shared
///   scale would draw the answer to "when did this query land" as a row of nothing. The absolute
///   numbers are in the header, where a scale cannot mislead about them.
/// - **No 30d/90d/1y presets.** They are the rail's job: `Rail`'s When section already offers
///   `today`/`week`/`month`/`>1mo`, and a second row of relative spans in the same window would
///   be one vocabulary written twice. Clicking one of those lights the selection here, because
///   the selection is read out of the query text like everything else.
struct TimelineDrawer: View {
    @Bindable var model: SearchModel
    @Environment(\.theme) private var theme

    /// `.tld-track`'s 42px. A literal for the reason `Minimap.width` is one: the height of a
    /// strip is not a visual direction, and a theme that halved it would leave no room for a bar
    /// either side of the baseline.
    private static let trackHeight: CGFloat = 42
    /// The gap between bars, in points. Bars narrower than this plus a hair are drawn solid,
    /// which at 180 bars in a 720 pt window is the common case.
    private static let gap: CGFloat = 1

    var body: some View {
        VStack(spacing: 0) {
            Rectangle().fill(theme.color(.rule)).frame(height: 1)
            if model.timeline.shown {
                open
            } else {
                shut
            }
        }
        .background(theme.color(.panel))
    }

    /// The closed drawer: one strip with the way back into it. Drawn rather than absent, because
    /// a region that vanishes takes the knowledge that it exists with it.
    private var shut: some View {
        HStack(spacing: 10) {
            Spacer(minLength: 0)
            button("show timeline") { model.toggleTimeline() }
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 5)
    }

    private var open: some View {
        VStack(spacing: 0) {
            header
            track
            axis
        }
        .padding(.horizontal, 12)
        .padding(.top, 7)
        .padding(.bottom, 9)
    }

    /// What the picture is of, in numbers the picture itself cannot state.
    ///
    /// `inRange` is the drawer's own question — how much was going on then, free text ignored —
    /// and `total` is the list's, repeated here because the two sit either side of the same
    /// window and reading them apart is the whole point of the drawer being open.
    @ViewBuilder
    private var header: some View {
        HStack(spacing: 10) {
            if let drawn = model.timeline.drawn {
                Text(verbatim: "\(drawn.inRange)").foregroundStyle(theme.color(.ink2))
                Text("in range")
                Text(verbatim: "· \(drawn.total)").foregroundStyle(theme.color(.ink2))
                Text("the query selects")
                // The window as a reader would have typed it, which is `cs_core`'s rendering and
                // not this app's: deriving a local day in a client is the shape of the bug
                // `cs_core::time` exists to have fixed once.
                Text(verbatim: "· \(drawn.window?.value ?? "all time")")
                if drawn.undated > 0 {
                    Text(verbatim: "· \(drawn.undated) undated")
                        .help("no message in them ever landed, so no bar can hold them")
                }
            } else if model.timeline.failure != nil {
                Text("the timeline could not be drawn")
            } else {
                Text("…")
            }
            Spacer(minLength: 0)
            button("hide") { model.toggleTimeline() }
        }
        .font(theme.font(.micro, .mono))
        .foregroundStyle(theme.color(.ink3))
        .padding(.bottom, 6)
    }

    @ViewBuilder
    private var track: some View {
        if let failure = model.timeline.failure, model.timeline.drawn == nil {
            Text(failure)
                .font(theme.font(.micro, .mono))
                .foregroundStyle(theme.color(.err))
                .frame(maxWidth: .infinity, alignment: .leading)
                .frame(height: Self.trackHeight)
        } else {
            GeometryReader { geometry in
                ZStack(alignment: .topLeading) {
                    TimelineBars(
                        drawn: model.timeline.drawn, gap: Self.gap,
                        hues: model.timeline.drawn.map(hues) ?? [])
                    selection(in: geometry.size.width)
                }
                .contentShape(Rectangle())
                // `minimumDistance: 0`, so a click is a drag of zero width — which is a span
                // naming nothing, which clears the filter. The prototype spells that as a
                // separate 1%-of-the-span rule; here it is the same gesture arriving at the same
                // place through the grammar.
                .gesture(
                    DragGesture(minimumDistance: 0)
                        .onEnded { g in
                            let width = max(geometry.size.width, 1)
                            model.scrub(
                                from: g.startLocation.x / width, to: g.location.x / width)
                        }
                )
            }
            .frame(height: Self.trackHeight)
            .background(theme.color(.mapBg))
            .overlay(
                RoundedRectangle(cornerRadius: theme.metric(.r3))
                    .strokeBorder(theme.color(.rule)))
            .clipShape(RoundedRectangle(cornerRadius: theme.metric(.r3)))
        }
    }

    /// `.tld-sel`: the `date:` window, drawn *over* bars that were counted without it.
    ///
    /// An open edge runs to the edge of the track and is not clamped to the data: `date:<7d`
    /// reaches past the newest conversation, and a rectangle that stopped where the bars stop
    /// would say the filter does too.
    @ViewBuilder
    private func selection(in width: CGFloat) -> some View {
        if let drawn = model.timeline.drawn, let window = drawn.window {
            let left = window.from.map { drawn.fraction(of: $0) } ?? 0
            let right = window.until.map { drawn.fraction(of: $0) } ?? 1
            let span = max(right - left, 0)
            Rectangle()
                .fill(theme.color(.sel).opacity(0.13))
                .overlay(alignment: .leading) {
                    Rectangle().fill(theme.color(.sel)).frame(width: 1)
                }
                .overlay(alignment: .trailing) {
                    Rectangle().fill(theme.color(.sel)).frame(width: 1)
                }
                .frame(width: max(span * width, 1))
                .offset(x: left * width)
                // The rectangle reports; it does not take the drag that was aimed through it.
                .allowsHitTesting(false)
        }
    }

    /// `.tld-ax`: the two ends of the axis, and what the two halves of the track mean.
    ///
    /// The dates come off the wire rather than out of a formatter here. That is `ended_date`'s
    /// rule and not fussiness: the local-date bug happened because three clients each worked out
    /// the day themselves, and a fourth is a fourth.
    private var axis: some View {
        HStack(spacing: 8) {
            Text(verbatim: model.timeline.drawn?.fromDate ?? "")
            Spacer(minLength: 0)
            Text("hits above the line · sources below")
            Spacer(minLength: 0)
            Text(verbatim: model.timeline.drawn?.untilDate ?? "")
        }
        .font(theme.font(.micro, .mono))
        .foregroundStyle(theme.color(.ink3))
        .padding(.top, 3)
    }

    /// The stack's colours, in the wire's own source order.
    ///
    /// **`Display.sourceHue` and no second mapping** — `chat-search-g6u` settled that a source id
    /// becomes a `--src-*` token in exactly one place, and the row and the rail already read it.
    /// A source the palette has no hue for is drawn in `--ink-3` rather than given a wrong one,
    /// which is that bead's other half.
    private func hues(_ drawn: Timeline) -> [Color] {
        drawn.sources.map { theme.color(Display.sourceHue($0) ?? .ink3) }
    }

    /// `.tld-btn`: monospaced micro in a hairline box.
    private func button(_ label: String, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Text(label)
                .font(theme.font(.micro, .mono))
                .foregroundStyle(theme.color(.ink3))
                .padding(.horizontal, 7)
                .padding(.vertical, 2)
                .overlay(
                    RoundedRectangle(cornerRadius: theme.metric(.r2))
                        .strokeBorder(theme.color(.rule2)))
                .contentShape(Rectangle())
        }
        // `.plain`, because a stock button paints itself in the system accent — a colour chosen
        // somewhere this app cannot see.
        .buttonStyle(.plain)
    }
}

/// Every bucket as a pair of bars either side of a baseline.
///
/// A view of its own for the reason `MinimapBands` is one: the selection rectangle moves whenever
/// the query's `date:` does and this does not, so keeping them apart is what stops 180 bars being
/// redrawn to move a rectangle. A `Canvas` and not a stack of views, for the reason
/// `chat-search-me9.22` gives — 180 identified `View`s is a shape that was measured and rejected.
struct TimelineBars: View {
    let drawn: Timeline?
    let gap: CGFloat
    /// One colour per `Timeline.sources`, resolved by the caller so this holds no palette rule.
    let hues: [Color]
    @Environment(\.theme) private var theme

    /// A non-empty bucket is never shorter than this. At 180 bars over a 42 pt track a single
    /// conversation rounds to a tenth of a point, and a bar rounded away is a week the picture
    /// says nothing happened in.
    private static let minimumBar: CGFloat = 1

    var body: some View {
        Canvas { context, size in
            guard let drawn, !drawn.buckets.isEmpty else { return }
            let middle = size.height / 2
            // Each half against its own tallest bar — see `TimelineDrawer`'s note. `max(1)` so an
            // axis with nothing on it divides by something.
            let tallest = drawn.buckets.map(\.conversations).max() ?? 0
            let loudest = drawn.buckets.map(\.matches).max() ?? 0
            let step = size.width / CGFloat(drawn.buckets.count)
            let width = max(step - gap, 0.5)

            context.stroke(
                Path { $0.move(to: CGPoint(x: 0, y: middle))
                        $0.addLine(to: CGPoint(x: size.width, y: middle)) },
                with: .color(theme.color(.rule2)))

            for (i, bucket) in drawn.buckets.enumerated() {
                let x = CGFloat(i) * step
                // Below the line: the filtered corpus, stacked by source. Painted from the
                // baseline down in the wire's order, so a stack does not reshuffle between
                // keystrokes even as a source empties out of it.
                var y = middle
                let scale = height(bucket.conversations, of: tallest, in: middle)
                for (at, count) in bucket.sources.enumerated() where count > 0 {
                    let share = scale * CGFloat(count) / CGFloat(max(bucket.conversations, 1))
                    context.fill(
                        Path(CGRect(x: x, y: y, width: width, height: share)),
                        with: .color(hues.indices.contains(at) ? hues[at] : theme.color(.ink3)))
                    y += share
                }
                // Above it: where this query landed, in one colour. The two are different
                // questions rather than a series and its highlight, so the hit bar is not a
                // recolouring of the bar below it.
                if bucket.matches > 0 {
                    let tall = height(bucket.matches, of: loudest, in: middle)
                    context.fill(
                        Path(CGRect(x: x, y: middle - tall, width: width, height: tall)),
                        with: .color(theme.color(.hit)))
                }
            }
        }
    }

    /// A count as a height, `log10` against the tallest bar in its own half.
    ///
    /// `MinimapLayout`'s rule and `poc/ui`'s, arrived at here by measurement rather than by
    /// copying. Linear was the first version and it is what the numbers rule out: over this
    /// archive the tallest bucket holds 222 conversations and the median non-empty one holds 16,
    /// so linear puts the median at 1.5 pt of a 21 pt half and draws three years of history as a
    /// flat line under two months of spike. That is `poc/ui/NOTES.md` §14's complaint exactly — a
    /// picture of the axis rather than of the corpus.
    ///
    /// The cost is stated rather than hidden: **a log axis understates magnitude**, and 222
    /// against 16 reads as roughly two to one. Which is why the absolute numbers are in the
    /// header, where a scale cannot mislead about them, and why the axis is drawn as a shape to
    /// aim at rather than a chart to read values off.
    ///
    /// The `+ 1` is not cosmetic: `log10(1)` is zero, and a bucket holding one conversation is a
    /// week the picture would otherwise say nothing happened in.
    private func height(_ count: Int, of top: Int, in half: CGFloat) -> CGFloat {
        guard count > 0, top > 0 else { return 0 }
        let share = log10(Double(count) + 1) / log10(Double(top) + 1)
        return max(Self.minimumBar, half * CGFloat(share))
    }
}
