import CsKit
import CsTheme
import SwiftUI

/// Where every message sits on a map of the whole conversation, in fractions of the map's height.
///
/// Pure geometry and no view: the same numbers answer both halves of the scroll relationship —
/// where to draw the band and which message a drag at 62% of the way down is pointing at — and
/// two derivations of that would be two conversations.
///
/// **Height is `log10` of the message's length, normalised over the conversation**, which is
/// `poc/ui/app.js`'s rule and not a row per message. A 2,431-message agent session has to fit a
/// drawer, and equal-height bands make a transcript of mostly short tool calls look like a
/// transcript of mostly prose — which is the one thing the map exists to tell you at a glance.
struct MinimapLayout: Sendable {
    /// Every message on the head path, drawn or not. The map describes the conversation rather
    /// than the current view of it, which is `app.js`'s "dim rather than drop, so the scrollbar
    /// keeps describing the whole conversation": a map that omitted what the fold is hiding would
    /// be a map of what is on screen, and there is already one of those — it is the screen.
    let blocks: [Block]

    /// The top of message *i*, with a final entry at 1 so the bottom of the last message needs no
    /// special case. `blocks.count + 1` long.
    private let offsets: [Double]
    /// Positions in `blocks` of the messages the transcript actually renders — the only places a
    /// scrub can land, because they are the only rows `List` has an id for.
    private let drawnPositions: [Int]
    private let positionOfId: [String: Int]

    init(_ blocks: [Block] = []) {
        self.blocks = blocks
        let weights = blocks.map(MinimapLayout.weight)
        let total = weights.reduce(0, +)
        var offsets = [Double]()
        offsets.reserveCapacity(blocks.count + 1)
        var accumulated = 0.0
        for weight in weights {
            offsets.append(total > 0 ? accumulated / total : 0)
            accumulated += weight
        }
        offsets.append(1)
        self.offsets = offsets
        self.drawnPositions = blocks.indices.filter { blocks[$0].drawn }
        self.positionOfId = Dictionary(
            blocks.enumerated().map { ($0.element.id, $0.offset) }, uniquingKeysWith: { a, _ in a })
    }

    /// `log10` of the message's length in bytes.
    ///
    /// Bytes and not characters for two reasons: `mark_offsets` already says `utf8-bytes`, so it
    /// is the unit this contract speaks; and `String.utf8.count` is O(1) on a native string where
    /// counting graphemes would walk all 2.8 MB of the corpus's longest conversation every time
    /// the map is laid out.
    ///
    /// The `+ 1` is not cosmetic. `log10(1)` is zero and `log10(0)` is not a number, and a band
    /// with no height is a message the map has lost — the shortest message in that conversation is
    /// two bytes and it is still a place you can be.
    private static func weight(_ block: Block) -> Double {
        log10(Double(block.text.utf8.count) + 1)
    }

    var isEmpty: Bool { blocks.isEmpty }

    /// The top and bottom of message *i*, as fractions of the map.
    func top(_ index: Int) -> Double { offsets[index] }
    func bottom(_ index: Int) -> Double { offsets[index + 1] }

    /// A point *inside* message *i*'s band, as a fraction of the map.
    ///
    /// The viewport box is two of these — the top and bottom edges of the visible rectangle, each
    /// part-way through the message it cuts — which is what makes the box move by pixels rather
    /// than by messages. `chat-search-me9.8.27`.
    func fraction(at index: Int, within: Double) -> Double {
        let clamped = Swift.min(Swift.max(within, 0), 1)
        return top(index) + (bottom(index) - top(index)) * clamped
    }

    /// Where a drag at this fraction of the map is pointing: a message the transcript can scroll
    /// to, and how far into that message the pointer is.
    ///
    /// The snap to a *drawn* message is not the boundary problem. It is that 952 of the longest
    /// conversation's 2,431 messages are successful tool results the reader folds away, so they
    /// are on the map — dim rather than dropped — and have no row for a scroll to land on. A
    /// pointer over one of those is pointing at nothing the transcript can show, and the top of
    /// the nearest drawn message is the only honest answer; `within` is zero there because a
    /// fraction of a message the pointer is not over would be a fabricated one.
    func target(atFraction fraction: Double) -> (id: String, within: Double)? {
        guard !drawnPositions.isEmpty else { return nil }
        let clamped = Swift.min(Swift.max(fraction, 0), 1)
        let wanted = search(clamped)
        let position = nearestDrawn(to: wanted)
        guard position == wanted else { return (blocks[position].id, 0) }
        let extent = bottom(position) - top(position)
        guard extent > 0 else { return (blocks[position].id, 0) }
        return (blocks[position].id, Swift.min(Swift.max((clamped - top(position)) / extent, 0), 1))
    }

    /// One drawn message either side of where the reader is — the arrow keys' unit.
    ///
    /// The prototype moves 48px because the DOM scrolls in pixels. This container moves in
    /// messages, so the step is a message: a unit expressed in the container's own terms is the
    /// one that cannot land between two of them.
    func drawnId(steppingFrom id: String?, by delta: Int) -> String? {
        guard !drawnPositions.isEmpty else { return nil }
        let from = id.flatMap { positionOfId[$0] } ?? drawnPositions[0]
        let moved = Swift.min(
            Swift.max(rank(atOrAfter: from) + delta, 0), drawnPositions.count - 1)
        return blocks[drawnPositions[moved]].id
    }

    /// Where a message sits in the conversation. The reader resolves the rows it has measured
    /// through this, and `--shot` reads it to say which stretch is on screen.
    func position(of id: String) -> Int? { positionOfId[id] }

    /// The message at a position, for the reader stepping one row at a time.
    func id(at position: Int) -> String? {
        blocks.indices.contains(position) ? blocks[position].id : nil
    }

    /// Which message owns this fraction of the map. Binary search: the alternative is a linear
    /// scan of 2,431 messages per frame of a drag.
    private func search(_ fraction: Double) -> Int {
        var low = 0
        var high = blocks.count - 1
        while low < high {
            let middle = (low + high + 1) / 2
            if offsets[middle] <= fraction { low = middle } else { high = middle - 1 }
        }
        return low
    }

    /// The drawn message nearest a position, in either direction. A drag that lands on a
    /// successful tool result — 952 of the 2,431 messages in the longest conversation — has to
    /// resolve to something the transcript has a row for.
    private func nearestDrawn(to position: Int) -> Int {
        let after = rank(atOrAfter: position)
        if after == 0 { return drawnPositions[0] }
        if after == drawnPositions.count { return drawnPositions[drawnPositions.count - 1] }
        let below = drawnPositions[after - 1]
        let above = drawnPositions[after]
        return position - below <= above - position ? below : above
    }

    /// How many drawn messages come before this position. Binary search for the same reason
    /// `search` is one: this is on the path of every frame of a drag, and `firstIndex(where:)`
    /// over 1,479 drawn messages is not.
    private func rank(atOrAfter position: Int) -> Int {
        var low = 0
        var high = drawnPositions.count
        while low < high {
            let middle = (low + high) / 2
            if drawnPositions[middle] < position { low = middle + 1 } else { high = middle }
        }
        return low
    }
}

/// The conversation as a column beside the transcript, and the scrubber for it.
///
/// `poc/ui/DESIGN-BRIEF.md` puts it in the right drawer and `styles.css` gives it the width:
/// `.pv { grid-template-columns: 1fr 50px }`. What it draws is the mockup's `.mm` — a band per
/// message, a tick per match, a box for where you are — and what it does *not* draw is named in
/// `apps/macos/README.md`, because two of the prototype's marks are not on this wire.
struct Minimap: View {
    /// The mockup's 50px column. A literal for the reason the seam states: the width of a strip
    /// is not a visual direction, and a 50pt column that a theme could halve would be a map with
    /// no room for a band and a tick side by side.
    static let width: CGFloat = 50

    let layout: MinimapLayout
    let reader: ReaderModel
    @Environment(\.theme) private var theme

    /// `Math.max(14, frac * h)` — a viewport box thinner than this is not a box, it is a line.
    private static let minimumViewport: CGFloat = 14

    var body: some View {
        GeometryReader { geometry in
            let height = geometry.size.height
            ZStack(alignment: .topLeading) {
                MinimapBands(layout: layout)
                viewport(in: height)
            }
            .contentShape(Rectangle())
            // `minimumDistance: 0` so a click is a jump and not just the start of a drag, which is
            // what the prototype's `pointerdown` → `scrub` does. The whole column is the target:
            // aiming at a band a quarter of a point tall is not a gesture anyone can make.
            .gesture(
                DragGesture(minimumDistance: 0)
                    .onChanged { reader.scrub(to: $0.location.y / max(height, 1)) }
                    .onEnded { _ in reader.endScrub() }
            )
        }
        .frame(width: Self.width)
        .background(theme.color(.mapBg))
        // `.mm`'s `border-left`, drawn here rather than by the caller so the map arrives with its
        // own edge — the transcript beside it has no business knowing it has a neighbour.
        .overlay(alignment: .leading) { Rectangle().fill(theme.color(.rule)).frame(width: 1) }
        // The mockup's `role="slider"` with `aria-valuenow`, in this platform's spelling. An
        // adjustable action rather than an arrow-key binding: the arrows in this window already
        // move the cursor in the results list, and a map that stole them would be a map that
        // fought the list for the keyboard.
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("conversation minimap")
        // `Text(verbatim:)`, because a bare literal is a `LocalizedStringKey` and an `Int`
        // interpolated into one gets thousands separators — the bug that rendered the corpus's
        // longest conversation as "2,553 ms…" in the row.
        .accessibilityValue(Text(verbatim: "\(Int((reader.scrollFraction * 100).rounded()))%"))
        .accessibilityAdjustableAction { direction in
            switch direction {
            case .increment: reader.step(by: 1)
            case .decrement: reader.step(by: -1)
            @unknown default: break
            }
        }
    }

    /// Where the transcript is: the visible rectangle, mapped through the messages it cuts. See
    /// `ReaderModel.visible` — since `chat-search-me9.8.27` this is a measurement of what is on
    /// screen rather than a report of which rows exist, so it moves by pixels.
    @ViewBuilder
    private func viewport(in height: CGFloat) -> some View {
        if let span = reader.visible {
            let box = max(Self.minimumViewport, (span.upperBound - span.lowerBound) * height)
            RoundedRectangle(cornerRadius: 2)
                .fill(theme.color(.sel).opacity(0.12))
                .overlay(
                    RoundedRectangle(cornerRadius: 2)
                        .strokeBorder(theme.color(.sel), lineWidth: 1))
                .frame(height: box)
                .offset(y: min(span.lowerBound * height, max(0, height - box)))
                // The box reports; it does not take the click that was aimed past it.
                .allowsHitTesting(false)
        }
    }
}

/// Every message as a band, and a tick for every match.
///
/// A view of its own and not a property of `Minimap`, which is a real optimisation and not a
/// tidying: the viewport box changes on every frame of a scroll and this does not, so keeping the
/// two apart is what stops 2,431 rectangles being redrawn sixty times a second. SwiftUI compares
/// the stored `layout` to decide, and the arrays inside it are the same buffers until a new
/// transcript lands — which is exactly when the bands *should* be drawn again, because that is
/// when the marks moved.
///
/// A `Canvas` and not a stack of views: 2,431 `View`s with identity is the shape
/// `chat-search-me9.22` measured at 566 MB. Nothing here is hit-tested individually — the gesture
/// on the column owns every point of it — so there is nothing a view would have bought.
///
/// Not `private`, for one reason: `--shot` reads `renders` below, and a claim about how often
/// something is drawn that nothing can check is a claim that stops being true quietly.
struct MinimapBands: View {
    let layout: MinimapLayout
    @Environment(\.theme) private var theme

    /// `.mm-band`: 8px in from the left, 18px wide for a user turn and 28px for everything else.
    /// **Band width encodes role** — cheap, and the one channel in this column that survives
    /// greyscale, since the four hues are a luminance ramp and greyscale is exactly where a ramp
    /// stops being four things.
    private static let inset: CGFloat = 8
    private static let userWidth: CGFloat = 18
    private static let bandWidth: CGFloat = 28
    /// A floor rather than a true height. At 2,431 messages in a 500pt column the average message
    /// is a fifth of a point, and a band rounded to nothing is a message you cannot aim at.
    private static let minimumBand: CGFloat = 1.5
    /// `.mm-band`'s `height: hh - 0.12%`, in points. Only ever visible on a short conversation,
    /// where the bands are tall enough to need telling apart.
    private static let bandGap: CGFloat = 0.5
    private static let tickWidth: CGFloat = 8
    private static let tickHeight: CGFloat = 2.5
    private static let tickInset: CGFloat = 3

    /// How many times these bands have actually been drawn, which `--shot` prints after driving a
    /// fling. It is the only way to check the claim above: the split is worth having exactly as
    /// long as this stays at 2 over a scroll that changes the box sixty times, and the day some
    /// innocent-looking capture puts `reader` back inside this view it will read in the hundreds
    /// instead.
    @MainActor static var renders = 0

    var body: some View {
        Canvas { context, size in
            MinimapBands.renders += 1
            for pass in Pass.allCases {
                for (index, block) in layout.blocks.enumerated() where Pass(block) == pass {
                    let top = layout.top(index) * size.height
                    let extent = (layout.bottom(index) - layout.top(index)) * size.height
                    let rect = CGRect(
                        x: Self.inset, y: top,
                        width: block.band == .user ? Self.userWidth : Self.bandWidth,
                        height: max(Self.minimumBand, extent - Self.bandGap))
                    context.fill(
                        Path(roundedRect: rect, cornerRadius: 1), with: .color(colour(of: block)))
                }
            }
            for (index, block) in layout.blocks.enumerated() where !block.marks.isEmpty {
                // Over the bands rather than a recolour of them, which is the prototype's rule: a
                // hit on a tool call still has to read as a tool call. The two mark kinds differ
                // by *length* and not by hue, for the reason the transcript gives — at two and a
                // half points there is no room for a second colour, and an unranked match drawn
                // exactly like a ranked one states in the map what the transcript refuses to say.
                let width = block.markKind.claimsRanking ? Self.tickWidth : Self.tickWidth / 2
                let rect = CGRect(
                    x: size.width - Self.tickInset - width,
                    y: layout.top(index) * size.height, width: width, height: Self.tickHeight)
                context.fill(
                    Path(roundedRect: rect, cornerRadius: 1), with: .color(theme.color(.hit)))
            }
        }
    }

    private func colour(of block: Block) -> Color {
        let base = theme.color(Display.bandToken(block))
        // `.mm-band.off` and `.mm-band.dim`. Dim rather than drop, and the two are different
        // numbers because they are different statements: `off` is a branch the conversation did
        // not take, `dim` is a message this reader is not being shown. Only the second happens
        // today — `cs show` returns the head path — and the first is written because the field is
        // on the wire and the toggle that uses it is a thing docs/TUI-DESIGN.md §8 describes.
        if !block.onPath { return base.opacity(0.3) }
        return block.drawn ? base : base.opacity(0.22)
    }

    /// Paint order, and the one place this map departs from the prototype.
    ///
    /// At 2,431 messages in a 500pt column every band is over the floor and they overlap, so the
    /// last one painted owns the pixel. Painted in transcript order that hands every pixel to tool
    /// traffic — 2,043 of those 2,431 messages — and the 43 user turns and every failure vanish
    /// under it. Those are the landmarks the map exists to carry, so they go last. The width
    /// encoding is what makes it safe: a user band is 18pt against 28pt, so a turn painted over a
    /// stretch of tool calls still leaves the tool band showing beside it.
    private enum Pass: CaseIterable {
        case ordinary, failure, user

        init(_ block: Block) {
            if block.band == .user {
                self = .user
            } else if block.isError {
                self = .failure
            } else {
                self = .ordinary
            }
        }
    }
}
