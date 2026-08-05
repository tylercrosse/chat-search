import CsKit
import Observation
import QuartzCore

/// What the window knows: a query, the answer to it, and what the index said about itself while
/// answering.
///
/// One query per keystroke and the previous one killed, with no debounce. `chat-search-me9.22`
/// measured that arrangement at 29–40 ms keystroke→frame p50 with a 0.3 ms fork/exec, so a
/// debounce here would be latency spent to save a cost that was measured and found small.
@MainActor
@Observable
final class SearchModel {
    var query = ""
    private(set) var conversations: [Conversation] = []
    /// Read off the last response rather than assumed. A row cannot mark a snippet without
    /// knowing what units its offsets are in, and this client is the one that once guessed.
    private(set) var marks: MarkOffsets = .utf8Bytes
    /// How many conversations match with `--limit` ignored, and whether that number is whole.
    /// `--prefix` is on for every keystroke here, so the unsettled case is the common one.
    private(set) var total = 0
    private(set) var settled = true
    /// Where the index stands, from whichever half of the contract said so — the `index_state`
    /// on an answered envelope or the `error.code` on a refusal. Both paths land here, which is
    /// what lets one `switch` in the view cover all four states.
    private(set) var health: IndexHealth = .ready
    private(set) var lastTiming: Timing?
    /// Filter tokens whose value selects nothing. Non-empty is not an error and the exit status
    /// stays 0, so a client that ignores this shows unfiltered results for a filtered query and
    /// it looks like it worked (`chat-search-6eb.11`). Kept here so exactly one view has to draw
    /// it and nobody has to remember to look.
    private(set) var unappliedFilters: [String] = []
    /// The facet rail, from `cs facets`. Nil until the first reply lands, and left standing when
    /// a later one fails — a rail that blinked out on a hiccup would look like a corpus that had
    /// lost its sources.
    private(set) var rail: FacetRail?
    /// Which row Enter would open. An id rather than an index: the list is replaced wholesale on
    /// every keystroke, and a position means a different conversation each time it is.
    var selected: String?
    /// What the last open attempt said, or nil if it opened. Cleared by the next keystroke,
    /// because a message about a conversation is stale the moment the list is not that list.
    private(set) var openFailure: String?

    let client: CsClient
    let limit: Int
    /// Set only by `--measure`. The honest end of a keystroke is a frame and only the display
    /// link knows when one happened, so the measurement needs a hook here — but a display link
    /// firing every vsync is not free, and an ordinary run does not have one.
    var frames: FrameClock?

    private var inFlight: Task<Void, Never>?
    private var railInFlight: Task<Void, Never>?
    /// When the keystroke that this query belongs to happened, on `CACurrentMediaTime`'s clock.
    private var keystrokeAt: Double?

    init(client: CsClient, limit: Int = 60) {
        self.client = client
        self.limit = limit
    }

    func noteKeystroke(at t: Double) { keystrokeAt = t }

    func queryChanged() {
        let text = query
        openFailure = nil
        // Killing the superseded process rather than waiting for it. Without this, typing eight
        // characters leaves eight `cs` processes competing for the same index and the last one —
        // the only one whose answer is wanted — finishes last.
        inFlight?.cancel()
        let started = keystrokeAt ?? CACurrentMediaTime()
        inFlight = Task { [weak self] in
            guard let self else { return }
            do {
                let result = try await client.search(text, limit: limit)
                guard !Task.isCancelled else { return }
                self.apply(result, keystrokeAt: started)
            } catch is CancellationError {
                return
            } catch CsError.unhealthy(let h) {
                guard !Task.isCancelled else { return }
                self.health = h
                self.conversations = []
            } catch {
                guard !Task.isCancelled else { return }
                self.health = .failed(String(describing: error))
                self.conversations = []
            }
        }
        refreshRail(for: text)
    }

    /// Re-project the rail onto the query, in a process of its own.
    ///
    /// A second spawn per keystroke and not on the critical path: it is measured at ~9 ms, the
    /// search is the answer being waited for, and a rail that lands a frame late is a rail. Same
    /// cancellation discipline as the search, so typing does not queue up eight of these either.
    ///
    /// **A failure leaves the previous rail standing.** The chips are a census of the corpus, not
    /// a property of this query, so blanking them on a hiccup would draw a machine that had lost
    /// its sources. What a stale rail can be wrong about is which chips are lit — for one
    /// keystroke, and the query box beside it is right the whole time.
    private func refreshRail(for text: String) {
        railInFlight?.cancel()
        railInFlight = Task { [weak self] in
            guard let self else { return }
            guard let rail = try? await client.facets(text), !Task.isCancelled else { return }
            self.rail = rail
        }
    }

    /// Put a chip's query text in the box, which is the only thing a chip click does.
    ///
    /// Every rewriting rule — widen an existing `agent:`, drop a standing exclusion, leave the
    /// free text where it is — happened on the far side of the seam, in the crate that owns the
    /// grammar. This holds no filter state of its own, so there is nothing here that could
    /// disagree with what the box says (docs/TUI-DESIGN.md §5).
    func apply(chip query: String) {
        keystrokeAt = CACurrentMediaTime()
        // Setting the text *is* the whole action: the field's `onChange` runs the query, exactly
        // as it does for a typed character. A chip click is a keystroke.
        self.query = query
    }

    /// Move the cursor. Wraps at neither end, so holding a key does not roll off the bottom and
    /// come back somewhere unexpected.
    func moveSelection(by delta: Int) {
        guard !conversations.isEmpty else { return }
        let here = conversations.firstIndex { $0.id == selected } ?? 0
        selected = conversations[min(max(here + delta, 0), conversations.count - 1)].id
    }

    /// Open the selected conversation where it lives.
    ///
    /// Fire-and-forget rather than awaited: `cs pick` recomputes the ranking to place the pick
    /// honestly, which costs a few milliseconds, and a window that froze for them would be
    /// paying interface latency for a log line.
    func openSelected() {
        guard let selected, let conv = conversations.first(where: { $0.id == selected }) else {
            return
        }
        open(conv)
    }

    func open(_ conv: Conversation) {
        // The finished query, which is what is recorded. Read now rather than inside the task:
        // typing continues while this runs, and the pick belongs to the query it was made from.
        let text = query
        Task { [weak self] in
            guard let self else { return }
            self.openFailure = await Opener.open(
                conv, query: text, limit: self.limit, client: self.client)
        }
    }

    /// Ask the same question again. Only offered while the index is `rebuilding`, where it means
    /// something specific: these results are complete but one build old, and the newer build may
    /// have landed since they were fetched.
    func askAgain() {
        keystrokeAt = CACurrentMediaTime()
        queryChanged()
    }

    private func apply(_ result: QueryResult<SearchResponse>, keystrokeAt started: Double) {
        // The success path's own reading of the index, which this client used to decode and then
        // throw away — `health = .ready` unconditionally, so `rebuilding` looked like `ready` and
        // nothing on screen could say a newer answer was coming (`chat-search-me9.8.1`).
        health = .answering(result.response.indexState)
        conversations = result.response.results
        marks = result.response.markOffsets
        total = result.response.total
        settled = result.response.settled
        unappliedFilters = result.response.unappliedFilters
        lastTiming = result.timing
        // A filter that narrows the list can take the cursor's row out of it, and a cursor
        // pointing at something the list no longer contains is a cursor that opens the wrong
        // conversation on Enter. Fall back to the best row, which is the one it started on.
        if selected == nil || !conversations.contains(where: { $0.id == selected }) {
            selected = conversations.first?.id
        }
        // The keystroke is not finished until a frame carries it. Hand the start time to the
        // display link, which is the only thing that knows when that was.
        frames?.pendingSince = started
        inFlight = nil
    }
}
