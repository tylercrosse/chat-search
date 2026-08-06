import CsKit
import CsTheme
import Foundation
import Observation
// For `ScrollGeometry` and `UnitPoint`. The scroll relationship is arithmetic over what the scroll
// view publishes and what its rows measure, and both arrive in this framework's own types — a
// second set of them here would be a translation layer with nothing on the other side of it.
import SwiftUI

/// What the drawer knows: which conversation is open, the transcript of it, and which messages
/// the reader has opened or closed by hand.
///
/// The row is held rather than the id, so the drawer survives a query that no longer returns it.
/// Typing on with something open is an ordinary thing to do — you have found the conversation and
/// are now looking for the next one — and a drawer that emptied itself mid-sentence would make
/// the search field and the thing being read fight for the same screen.
@MainActor
@Observable
final class ReaderModel {
    /// The conversation the drawer is showing, or nil when there is no drawer.
    private(set) var conv: Conversation?
    private(set) var transcript: Transcript?
    /// Why there is no transcript, when there is a reason worth printing. Never a substitute for
    /// one: a failure here replaces the messages and says so, rather than showing an empty pane.
    private(set) var failure: String?
    private(set) var loading = false

    /// Folds the reader set by hand. They beat the wire's default and are dropped with the
    /// conversation, because which messages someone has opened is session state — which is
    /// exactly why `cs_core::blocks` answers the default and refuses to model this.
    private var overrides: [String: Fold] = [:]

    /// Every drawn message as an `AttributedString`, built once instead of once per body
    /// evaluation. Held here rather than on the view for the reason the folds are: a computed
    /// property on a `View` is rebuilt whenever SwiftUI feels like asking, and the fold that
    /// decides which string is marked already lives on this side of the line.
    ///
    /// Not private, for the same reason `MinimapBands.renders` is not: `--shot` prints its two
    /// counters, which is the only check there is on the claim it exists to make.
    let markedText = MarkedText()

    /// The conversation as a map, laid out once per transcript rather than once per redraw: it is
    /// O(messages) to build and the view holding it is invalidated on every frame of a scroll.
    private(set) var minimap = MinimapLayout()

    /// The rectangle the transcript is drawn in, and how much document is behind it.
    ///
    /// **This is what the floor raise bought** (`chat-search-me9.8.27`). Until macOS 15 a `List`
    /// gave back neither of the two numbers the prototype reads off the DOM — `scrollTop` and
    /// `scrollHeight` — so the position of the transcript was kept as the set of rows that had
    /// reported themselves through `onAppear`. That is a superset of what a reader can see, since
    /// `NSTableView` prepares rows past both edges, and it changes only when a whole row crosses
    /// one — which is why the box was too tall and moved a message at a time.
    ///
    /// `frame` is the visible rectangle in the window's coordinates and is what a row's own
    /// rectangle is tested against; `document` and `offset` come from `onScrollGeometryChange`
    /// and are what `--shot` checks against AppKit's numbers for the same scroll view.
    private(set) var viewport = Viewport()

    struct Viewport: Equatable {
        var frame: CGRect = .zero
        var offset: CGFloat = 0
        var document: CGFloat = 0
        var height: CGFloat { frame.height }
    }

    /// Where each row the list is holding actually sits, in the same coordinates as
    /// `viewport.frame`.
    ///
    /// Still reported by the rows, and deliberately so — this container has no other way to say
    /// where a row is. What changed is *what* they report: `onAppear` said a row exists, and
    /// `onGeometryChange` says where it is in points, so a row half off the top of the screen can
    /// say which half.
    private(set) var placed: [String: CGRect] = [:]

    /// Where the minimap has asked the transcript to go. Consumed by `ReaderView`, which is the
    /// only place with a `ScrollViewReader` to ask.
    ///
    /// Carrying a serial rather than an id alone: dragging away and back lands on the same message
    /// and has to scroll there again, and an `onChange` cannot see a value that did not change.
    private(set) var scrollRequest: ScrollRequest?

    struct ScrollRequest: Equatable {
        let id: String
        /// Where in the viewport to put the message, which is how a drag lands *inside* one. See
        /// `anchor(for:within:)`.
        let anchor: UnitPoint
        let serial: Int
    }

    /// The last thing a drag asked for, so a drag that crosses fifty messages in one flick issues
    /// one scroll per message rather than one per frame — and, inside a message tall enough to
    /// scroll through, one per anchor it actually moves to. Cleared when the drag ends, because
    /// dragging back to a message you scrolled away from by hand is a real request.
    private var lastScrubbed: ScrollRequest?
    /// A drag can name a message the list has not laid out, and the anchor arithmetic needs that
    /// message's height. The request is re-issued once, the moment the row says where it landed.
    private var unrefined: (id: String, within: Double)?
    private var scrollSerial = 0

    let client: CsClient
    private var inFlight: Task<Void, Never>?

    init(client: CsClient) {
        self.client = client
    }

    /// Open a conversation, or close the drawer with nil.
    ///
    /// Re-opening the one already open is a no-op rather than a reload, so clicking the selected
    /// row does not throw away every fold the reader has set on it.
    func open(_ conv: Conversation?, query: String) {
        guard conv?.id != self.conv?.id else { return }
        self.conv = conv
        transcript = nil
        failure = nil
        overrides.removeAll()
        // The map and the position go with the conversation. Rows of the old one never report
        // themselves gone — `List` is handed a new array rather than emptied — so a set that
        // survived would draw a viewport box over a conversation those messages are not in.
        minimap = MinimapLayout()
        placed.removeAll()
        viewport = Viewport()
        scrollRequest = nil
        lastScrubbed = nil
        unrefined = nil
        // The marked text goes too. It invalidates itself against whatever transcript is asking
        // (see `MarkedText`), so this is not about staleness — it is that a shut drawer asks
        // nothing, and a table nobody consults is a megabyte of a conversation nobody is reading.
        markedText.forget()
        load(query: query)
    }

    /// Shut the drawer. The transcript goes with it rather than being kept warm: re-reading it
    /// costs one process, and holding a megabyte of a conversation nobody is looking at is the
    /// kind of cache that is only ever noticed when it is wrong.
    func close() { open(nil, query: "") }

    /// The query moved. Re-read, so the marks in the drawer and the marks in the list can never
    /// be answers to two different questions.
    ///
    /// A whole transcript per keystroke, which sounds worse than it is: the median conversation
    /// is ~10 KB and the corpus's longest measured 50–90 ms end to end, against the ~50 ms the
    /// search beside it already costs. The superseded process is killed rather than waited for,
    /// the same arrangement `SearchModel` uses and for the same reason.
    func queryChanged(_ query: String) {
        guard conv != nil else { return }
        load(query: query)
    }

    /// The wire's default for this message unless the reader has said otherwise.
    func fold(of block: Block) -> Fold { overrides[block.id] ?? block.fold }

    func toggle(_ block: Block) { overrides[block.id] = fold(of: block).toggled }

    /// The message as the drawer draws it: marked, and folded the way this reader has it.
    ///
    /// The transcript is passed in rather than read off `self` because the caller is drawing one,
    /// and it carries the other half of what a marked message is an answer to — see `MarkedText`.
    func marked(_ block: Block, in transcript: Transcript, theme: Theme) -> AttributedString {
        markedText.of(block, fold: fold(of: block), in: transcript, theme: theme)
    }

    // MARK: - The scroll relationship

    /// The list was laid out somewhere. Called from the transcript and from nowhere else.
    func viewportMoved(to frame: CGRect) { viewport.frame = frame }

    /// The list scrolled.
    func scrolled(_ geometry: ScrollGeometry) {
        viewport.offset = geometry.contentOffset.y
        viewport.document = geometry.contentSize.height
    }

    /// A row moved. Called for every row the list is holding, on every frame of a scroll.
    ///
    /// Written straight through, which was not the first version of the old `onAppear` path
    /// either. That one buffered into an unobserved set and published once per turn of the run
    /// loop, and it turned out to be a hand-rolled copy of what SwiftUI already does, since an
    /// observable write marks a view dirty and the body runs at the next frame regardless. The
    /// same argument holds here and the simpler one ships; `apps/macos/README.md` has the numbers
    /// for both the old pair and this.
    func rowMoved(_ id: String, to rect: CGRect) {
        placed[id] = rect
        // The message a drag asked for has just said how tall it is, so the anchor that lands the
        // pointer inside it can finally be worked out. Once only: the scroll this issues moves the
        // row again, and a second pass would be a loop with a scroll view on both ends of it.
        guard let pending = unrefined, pending.id == id else { return }
        unrefined = nil
        let refined = anchor(for: id, within: pending.within)
        guard refined != scrollRequest?.anchor else { return }
        issue(id, anchor: refined)
    }

    /// A row went away. The rectangle goes with it, because the last place a row was seen is not
    /// where it is now — a stale one sitting at the top edge would keep claiming to be on screen
    /// for the rest of the conversation.
    func rowLeft(_ id: String) { placed.removeValue(forKey: id) }

    /// A drag on the minimap, as a fraction of its height.
    func scrub(to fraction: Double) {
        guard let target = minimap.target(atFraction: fraction) else { return }
        request(target.id, within: target.within)
    }

    func endScrub() {
        lastScrubbed = nil
        unrefined = nil
    }

    /// One drawn message up or down — the minimap's adjustable action, and the only way to move
    /// it without a pointer.
    ///
    /// Stepped from the first message whose *start* is on screen, which is not the same as the
    /// top of the box and is the thing that makes a step advance. Scrolling to a message puts it
    /// 10 pt down — `contentMargins` — so the tail of the message above it stays visible and owns
    /// the top edge of the box. Anchored on that one, every step would resolve to the message
    /// just scrolled to and the keyboard would stand still; `chat-search-me9.8.18` worked around
    /// the same shape by anchoring on the message last asked for, which this replaces with a fact
    /// about the screen rather than about the request.
    func step(by delta: Int) {
        let from = stepAnchor ?? visibleMessages.flatMap { minimap.id(at: $0.top) }
        guard let id = minimap.drawnId(steppingFrom: from, by: delta) else { return }
        request(id, within: 0)
    }

    /// The topmost message the reader can see the beginning of.
    private var stepAnchor: String? {
        let edge = viewport.frame
        guard edge.height > 0 else { return nil }
        var best: Int?
        for (id, rect) in placed {
            guard rect.minY >= edge.minY, rect.minY < edge.maxY,
                let position = minimap.position(of: id)
            else { continue }
            if position < best ?? Int.max { best = position }
        }
        return best.flatMap { minimap.id(at: $0) }
    }

    /// The stretch of the map the reader can actually see, or nil before anything has been laid
    /// out. What the viewport box is drawn from.
    var visible: ClosedRange<Double>? {
        guard let span = visibleMessages else { return nil }
        let lower = minimap.fraction(at: span.top, within: span.topWithin)
        let upper = minimap.fraction(at: span.bottom, within: span.bottomWithin)
        return lower...Swift.max(lower, upper)
    }

    /// How far down the conversation the reader is, 0 to 1. What the map says out loud.
    var scrollFraction: Double { visible?.lowerBound ?? 0 }

    /// The visible rectangle, in messages and in fractions of the two the edges cut through.
    ///
    /// A row counts when its rectangle overlaps the viewport at all, which is the whole of the
    /// difference from the old set: `NSTableView` hands out rows above and below the edges, and
    /// those have negative or past-the-bottom rectangles and are excluded by arithmetic rather
    /// than hoped about.
    var visibleMessages: (top: Int, topWithin: Double, bottom: Int, bottomWithin: Double)? {
        let edge = viewport.frame
        guard edge.height > 0 else { return nil }
        var top = Int.max
        var bottom = Int.min
        var topWithin = 0.0
        var bottomWithin = 1.0
        for (id, rect) in placed {
            guard rect.height > 0, rect.maxY > edge.minY, rect.minY < edge.maxY,
                let position = minimap.position(of: id)
            else { continue }
            if position < top {
                top = position
                topWithin = Double(Swift.max(0, edge.minY - rect.minY) / rect.height)
            }
            if position > bottom {
                bottom = position
                bottomWithin = Double(Swift.min(rect.height, edge.maxY - rect.minY) / rect.height)
            }
        }
        guard top <= bottom else { return nil }
        return (top, topWithin, bottom, bottomWithin)
    }

    /// Where in the viewport to put a message so that the point the pointer is over ends up at the
    /// top of the screen.
    ///
    /// `scrollTo(id:anchor:)` aligns the row's anchor point with the viewport's, so a row of
    /// height *h* in a viewport of height *H* lands at document offset `rowTop + a(h - H)`.
    /// Landing fraction *f* of the row at the top therefore wants `a = f·h / (h - H)`, which is
    /// inside `0...1` exactly when the message is taller than the viewport — and that is the only
    /// case where a message boundary is somewhere a reader can see the difference from, because a
    /// message that fits on screen is already entirely on screen once its top is. So this is the
    /// documented anchor semantic and not an extrapolation of it: `me9.8.18`'s "on a short
    /// conversation with tall blocks it is the block" was the whole of that cost, and it is the
    /// half that is fixed.
    private func anchor(for id: String, within: Double) -> UnitPoint {
        guard within > 0, let height = placed[id]?.height, height > viewport.height else {
            return .top
        }
        let fraction = within * Double(height) / Double(height - viewport.height)
        return UnitPoint(x: 0.5, y: Swift.min(Swift.max(fraction, 0), 1))
    }

    private func request(_ id: String, within: Double) {
        // A message the list has not laid out has no height, so the anchor cannot be worked out
        // yet and the top of it is where this lands. `rowMoved` finishes the job.
        unrefined = within > 0 && placed[id] == nil ? (id, within) : nil
        issue(id, anchor: anchor(for: id, within: within))
    }

    private func issue(_ id: String, anchor: UnitPoint) {
        guard id != lastScrubbed?.id || anchor != lastScrubbed?.anchor else { return }
        scrollSerial += 1
        let request = ScrollRequest(id: id, anchor: anchor, serial: scrollSerial)
        lastScrubbed = request
        scrollRequest = request
    }

    private func load(query: String) {
        inFlight?.cancel()
        guard let id = conv?.id else {
            loading = false
            return
        }
        loading = true
        inFlight = Task { [weak self] in
            guard let self else { return }
            do {
                let transcript = try await client.show(id, query: query)
                // The id is checked again as well as the cancellation flag: a task cancelled
                // between the last suspension and here has already returned its answer, and it
                // would be an answer about whatever was open before.
                guard !Task.isCancelled, self.conv?.id == id else { return }
                self.transcript = transcript
                self.minimap = MinimapLayout(transcript.messages)
                self.failure = nil
            } catch is CancellationError {
                return
            } catch {
                guard !Task.isCancelled, self.conv?.id == id else { return }
                self.transcript = nil
                self.failure = ReaderModel.describe(error)
            }
            self.loading = false
            self.inFlight = nil
        }
    }

    /// What went wrong, in one line a reader can act on.
    ///
    /// Shorter than the shell's four index states on purpose. The drawer only ever opens on a row
    /// the search just returned, so the states that mean "there is no index" cannot arrive here
    /// without something having changed underneath a running app — which is worth printing
    /// verbatim rather than dressing up as one of them.
    private static func describe(_ error: Error) -> String {
        switch error {
        case CsError.undecodable(let why, _):
            "this build could not read the transcript — \(why)"
        case CsError.unhealthy(let health):
            switch health {
            case .noIndex(let detail), .building(let detail), .stale(let detail),
                .noBinary(let detail), .failed(let detail):
                detail
            case .ready, .rebuilding, .unrecognised:
                // Unreachable: these three describe an answer, and this path has none.
                "cs answered without a transcript"
            }
        default:
            String(describing: error)
        }
    }
}
