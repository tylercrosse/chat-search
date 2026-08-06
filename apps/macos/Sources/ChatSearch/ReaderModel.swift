import CsKit
import CsTheme
import Foundation
import Observation

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
    private let markedText = MarkedText()

    /// The conversation as a map, laid out once per transcript rather than once per redraw: it is
    /// O(messages) to build and the view holding it is invalidated on every frame of a scroll.
    private(set) var minimap = MinimapLayout()

    /// The messages `List` currently has on screen, as its rows report themselves.
    ///
    /// **This is the whole of route 1**, and it is worth being exact about what it is. `List` is
    /// the only container `chat-search-me9.22` measured that recycles, and it gives back neither
    /// of the two numbers the prototype reads off the DOM — `scrollTop` and `scrollHeight`. What
    /// it does give is `onAppear` and `onDisappear` per row, so the position of the transcript is
    /// kept in the units this container actually speaks: which rows exist right now.
    ///
    /// The cost is that this is a *superset* of what a reader can see. `NSTableView` prepares rows
    /// beyond the visible rectangle, so the box drawn from these is larger than the viewport and
    /// larger at the ends of a fling than in the middle of one. It is a report of where you are
    /// and not a measurement of what you can see; `apps/macos/README.md` records what that
    /// measured, and route 2 is what to reach for if it ever stops being good enough.
    private(set) var onScreen: Set<String> = []

    /// Where the minimap has asked the transcript to go. Consumed by `ReaderView`, which is the
    /// only place with a `ScrollViewReader` to ask.
    ///
    /// Carrying a serial rather than an id alone: dragging away and back lands on the same message
    /// and has to scroll there again, and an `onChange` cannot see a value that did not change.
    private(set) var scrollRequest: ScrollRequest?

    struct ScrollRequest: Equatable {
        let id: String
        let serial: Int
    }

    /// The last message a drag asked for, so a drag that crosses fifty of them in one flick issues
    /// one scroll per message rather than one per frame. Cleared when the drag ends, because
    /// dragging back to a message you scrolled away from by hand is a real request.
    private var lastScrubbed: String?
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
        onScreen.removeAll()
        scrollRequest = nil
        lastScrubbed = nil
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

    /// A row appeared or went away. Called from the transcript's rows and from nowhere else.
    ///
    /// Written straight through, which was not the first version. `onAppear` fires from inside
    /// `NSTableView`'s row preparation and a fling changes twenty rows a frame, so this buffered
    /// into an unobserved set and published once per turn of the run loop — and that turned out to
    /// be a hand-rolled copy of what SwiftUI already does, since an observable write marks a view
    /// dirty and the body runs at the next frame either way. Measured against each other on the
    /// corpus's longest conversation, neither the frame tail nor the count of AppKit's reentrancy
    /// warning could tell the two apart. The simpler one ships; `apps/macos/README.md` has both
    /// sets of numbers.
    func rowAppeared(_ id: String) { onScreen.insert(id) }

    func rowDisappeared(_ id: String) { onScreen.remove(id) }

    /// A drag on the minimap, as a fraction of its height.
    func scrub(to fraction: Double) {
        guard let id = minimap.drawnId(atFraction: fraction), id != lastScrubbed else { return }
        request(id)
    }

    func endScrub() { lastScrubbed = nil }

    /// One drawn message up or down — the minimap's adjustable action, and the only way to move
    /// it without a pointer.
    ///
    /// Stepped from the message this last asked for, as long as that message is still on screen,
    /// and from the top of the box otherwise. Anchoring on the box alone looks right and is not:
    /// `onScreen` is a superset of what is visible, so a step of one message frequently does not
    /// move its top edge, and the next step would then resolve to the same message again and the
    /// keyboard would be stuck one message below where it started.
    func step(by delta: Int) {
        // Annotated, because `scrollRequest?.id.flatMap` reads as `String`'s and walks the id one
        // character at a time.
        let requested: String? = scrollRequest?.id
        let anchor = requested.flatMap { onScreen.contains($0) ? $0 : nil }
            ?? minimap.firstOnScreen(of: onScreen)
        guard let id = minimap.drawnId(steppingFrom: anchor, by: delta) else { return }
        request(id)
    }

    /// How far down the conversation the reader is, 0 to 1. What the map says out loud.
    var scrollFraction: Double {
        minimap.span(ofIds: onScreen)?.lowerBound ?? 0
    }

    private func request(_ id: String) {
        lastScrubbed = id
        scrollSerial += 1
        scrollRequest = ScrollRequest(id: id, serial: scrollSerial)
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
