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

    let client: CsClient
    /// The drawer beside the list. Held here rather than in the view because the query and what
    /// the transcript was marked against have to be one fact: a drawer marked against a query the
    /// list has moved past would highlight words the rows no longer claim to have matched.
    let reader: ReaderModel
    let limit: Int
    /// Set only by `--measure`. The honest end of a keystroke is a frame and only the display
    /// link knows when one happened, so the measurement needs a hook here — but a display link
    /// firing every vsync is not free, and an ordinary run does not have one.
    var frames: FrameClock?

    private var inFlight: Task<Void, Never>?
    /// When the keystroke that this query belongs to happened, on `CACurrentMediaTime`'s clock.
    private var keystrokeAt: Double?

    init(client: CsClient, limit: Int = 60) {
        self.client = client
        self.reader = ReaderModel(client: client)
        self.limit = limit
    }

    func noteKeystroke(at t: Double) { keystrokeAt = t }

    func queryChanged() {
        let text = query
        // The open conversation is re-marked against the same query, on the same keystroke. Two
        // processes rather than one, both cancellable; a drawer that lagged the list would be
        // showing highlights for a question nobody is asking any more.
        reader.queryChanged(text)
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
        lastTiming = result.timing
        // The keystroke is not finished until a frame carries it. Hand the start time to the
        // display link, which is the only thing that knows when that was.
        frames?.pendingSince = started
        inFlight = nil
    }
}
