import CsKit
import Observation
import QuartzCore

/// One line the list draws, and therefore one place the cursor can be.
///
/// Ungrouped this is the rows and nothing else, which is all the cursor has ever moved through.
/// Grouped it is a group head followed by its rows — and followed by nothing when that group is
/// folded, which is the whole of how a folded section is kept from holding the cursor. A head is a
/// place the cursor can rest because it has to be: fold everything and there is no row left to
/// stand on, and the key that opens one back up has to be pressed somewhere.
enum Cursor: Equatable {
    /// A conversation, by `Conversation.id`.
    case row(String)
    /// The head of a group, by `ConversationGroup.key`.
    case head(String)
}

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
    /// Which of the two views is on screen. Grouping is not one of these, which is the whole
    /// point of `chat-search-4ar.10`: three of the prototype's four views were `GROUP BY` over one
    /// set, and only `Library` is a different thing rather than a different cut.
    var surface: Surface = .search
    /// The axis the one list is cut along, and the groups that fall out of it.
    ///
    /// Held here rather than in the view because the keyboard reads it: grouping changes the order
    /// rows are drawn in, and a cursor that moved through the ranked order while the screen showed
    /// the grouped one would open the row above or below the one under it.
    private(set) var grouping: Grouping = .none
    private(set) var groups: [ConversationGroup] = []
    /// Whether a group nobody has touched is drawn folded.
    ///
    /// False, and that is a statement about *this* window rather than about grouping. `poc/ui`
    /// opens every axis folded and is right to: it groups a corpus-scale export, where thirteen
    /// heads carrying a count, a span and a sparkline are a project index and one open group buries
    /// the other twelve (`poc/ui/NOTES.md` §2). This groups the `--limit` window of a ranked answer
    /// — 60 rows — so folding by default would hide the answer behind its own heads on every
    /// keystroke. The trade flips when that window grows, which is why the default is one boolean
    /// here rather than an assumption spread through the view: `--folded` already flips it.
    private(set) var foldsByDefault = false
    /// The groups drawn against that default, by `ConversationGroup.key`.
    ///
    /// Not pruned to the groups that currently exist. A keystroke can narrow the list until a group
    /// has no rows in it and the next one bring it back, and a fold that forgot itself in between
    /// would be a list rearranging under a cursor that never asked it to.
    private var toggled: Set<String> = []
    /// Which line Enter would act on — a conversation to open, or a group head to fold. An id
    /// rather than an index: the list is replaced wholesale on every keystroke, and a position
    /// means a different conversation each time it is.
    var cursor: Cursor?
    /// What the last open attempt said, or nil if it opened. Cleared by the next keystroke,
    /// because a message about a conversation is stale the moment the list is not that list.
    private(set) var openFailure: String?
    /// Whether the query now in the box has been answered by opening something.
    ///
    /// A statement about *this* query rather than about the session, which is why the next
    /// keystroke clears it: finding one conversation and then going looking for a second is the
    /// ordinary way this gets used, and the second search going unanswered is precisely what
    /// docs/TUI-DESIGN.md §6 wants written down.
    private(set) var answered = false

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
    private var railInFlight: Task<Void, Never>?
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
        openFailure = nil
        // `askAgain` comes through here too, with the text unchanged, and that is the reading
        // this wants: asking the same question a second time is asking it again.
        answered = false
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
                self.refused(h)
            } catch {
                guard !Task.isCancelled else { return }
                self.refused(.failed(String(describing: error)))
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

    /// No answer, so nothing that describes one may stay on screen. `unappliedFilters` in
    /// particular: it names tokens in a query that *was* answered, and left standing it would
    /// report the last successful search's filters against a screen saying there is no index.
    private func refused(_ health: IndexHealth) {
        self.health = health
        conversations = []
        unappliedFilters = []
        cursor = nil
        regroup()
    }

    /// Cut the list a different way. The same rows, in a different arrangement — no second
    /// request, because grouping is a property of the answer in hand and not of the question.
    func group(by axis: Grouping) {
        guard axis != grouping else { return }
        grouping = axis
        // Switching axes clears the accordion rather than restoring it (`poc/ui/NOTES.md` §2):
        // groups are *ranked*, so the set you left open is rarely the set at the top when you come
        // back — and a key from the axis you left cannot name a group on the axis you arrived at
        // anyway. Clicking the axis already in force returns above, so the reset is a consequence
        // of switching and never a surprise.
        toggled.removeAll()
        regroup()
        place()
    }

    /// The conversation under the cursor, or nil when the cursor is on a group head.
    var selected: String? {
        guard case .row(let id) = cursor else { return nil }
        return id
    }

    /// Whether this group is drawn folded.
    func isFolded(_ key: String) -> Bool { foldsByDefault != toggled.contains(key) }

    /// How many groups are folded, which the footer says because a folded group and a group that
    /// is not there look the same from above it.
    var foldedGroups: Int { groups.filter { isFolded($0.key) }.count }

    /// Fold or unfold one group, and take the cursor with it.
    ///
    /// Folding the group the cursor is standing in would leave it on a row that is no longer drawn
    /// — the one state a fold must not be able to produce, because the next Enter would then open a
    /// conversation nobody can see. The head is where it goes: the line that is still on the screen,
    /// and the line that opens the group again.
    func toggleFold(_ key: String) {
        if toggled.contains(key) { toggled.remove(key) } else { toggled.insert(key) }
        guard isFolded(key), case .row(let id) = cursor else { return }
        let holdsCursor = groups.first(where: { $0.key == key })?
            .items.contains(where: { $0.id == id })
        if holdsCursor == true { cursor = .head(key) }
    }

    /// Fold or unfold everything, which is the state `--folded` opens in.
    ///
    /// It moves the default rather than folding each group in turn, so a group that arrives on a
    /// later keystroke is in the same state as the ones already on screen. An instrument that
    /// measured a folded list for the first phrase and a half-folded one for the second would not
    /// be measuring anything.
    func foldAll(_ folded: Bool) {
        foldsByDefault = folded
        toggled.removeAll()
        place()
    }

    /// Every line the list draws, in the order it draws them — which is the order an arrow key
    /// moves through.
    ///
    /// Ungrouped that is the ranking and nothing else. Grouped it is a head per group and, under
    /// each open one, the same rows gathered: `project` and `source` preserve the ranking, `run`
    /// re-orders by time because a run cannot be found any other way. **A folded group contributes
    /// its head and nothing more**, which is the single place the fold has to be honoured — there
    /// is no other arithmetic in this class that has to remember what is on the screen.
    var lines: [Cursor] {
        guard grouping != .none else { return conversations.map { .row($0.id) } }
        return groups.flatMap { group in
            isFolded(group.key)
                ? [Cursor.head(group.key)]
                : [Cursor.head(group.key)] + group.items.map { Cursor.row($0.id) }
        }
    }

    private func regroup() {
        groups = grouping.groups(of: conversations)
    }

    /// Put the cursor on a line the list actually draws.
    ///
    /// A narrowed query can take the cursor's row out of the answer, and a fold can take it off the
    /// screen; either way a cursor pointing at something the list does not draw is a cursor that
    /// opens the wrong conversation on Enter. Fall back to the first line, which is where it
    /// started.
    private func place() {
        let lines = self.lines
        if let cursor, lines.contains(cursor) { return }
        cursor = lines.first
    }

    /// Move the cursor. Wraps at neither end, so holding a key does not roll off the bottom and
    /// come back somewhere unexpected.
    func moveSelection(by delta: Int) {
        let lines = self.lines
        guard !lines.isEmpty else { return }
        let here = cursor.flatMap { lines.firstIndex(of: $0) } ?? 0
        cursor = lines[min(max(here + delta, 0), lines.count - 1)]
    }

    /// What Enter does, which is a question about what the cursor is on: a conversation opens, a
    /// group head folds or unfolds.
    ///
    /// One key rather than the prototype's `→`, and not because a second binding would be
    /// expensive. The only focused view in this window is the query box, so a key this app binds is
    /// a key the box stops getting — and left and right are how a caret moves through a query that
    /// is a grammar (`agent:codex`, a quoted `dir:` with a space in it). Enter is already the
    /// list's rather than the field's, so the fold costs nothing that was being used.
    func activate() {
        switch cursor {
        case .row: openSelected()
        case .head(let key): toggleFold(key)
        case nil: break
        }
    }

    /// What Enter would do where the cursor is, when that is not what it has always done.
    ///
    /// `poc/ui/NOTES.md` §5's complaint about the prototype is a footer that drew a key nobody had
    /// wired. A fold the keyboard can reach and no footer mentions is the same defect from the
    /// other side, so the one line that is new — the cursor standing on a group head — says so.
    var cursorHint: String? {
        guard case .head(let key) = cursor else { return nil }
        return isFolded(key) ? "⏎ opens this group" : "⏎ folds this group"
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

    /// Open a conversation. `destination` names one of its own, for the menu that offers a
    /// choice; `nil` takes the best, which is what Enter and a double-click mean.
    func open(_ conv: Conversation, at destination: Destination? = nil) {
        // The finished query, which is what is recorded. Read now rather than inside the task:
        // typing continues while this runs, and the pick belongs to the query it was made from.
        let text = query
        // Set here rather than when the task returns, for the same reason `Opener` records on
        // every path it takes: a conversation that was wanted and could not be reached is as
        // much of a judgement as one that opened, so this query has had its answer either way.
        answered = true
        Task { [weak self] in
            guard let self else { return }
            self.openFailure = await Opener.open(
                conv, at: destination, query: text, limit: self.limit, client: self.client)
        }
    }

    /// The query this window should record as abandoned, or nil if there is nothing to record.
    ///
    /// docs/TUI-DESIGN.md §6's third row: quit with a non-blank query and no pick, and the log
    /// gets one `Search`. The empty box is skipped here only to save spawning a process for
    /// nothing — what counts as a query with a need behind it is `cs abandon`'s to say, and it
    /// drops more than this could (whitespace, `"??"`, a filter with no text beside it).
    var abandonedQuery: String? {
        guard !answered, !query.isEmpty else { return nil }
        return query
    }

    /// Write the abandonment down, at the one moment there is one to write.
    ///
    /// Blocking, on the way out. `cs abandon` re-ranks the finished query so that the list it
    /// records is the one a `Pick` would have been placed in, which costs roughly what a search
    /// costs — and quit is the one place in this app where there is no frame left to drop.
    func recordAbandonment() {
        guard let text = abandonedQuery else { return }
        client.abandon(query: text, limit: limit)
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
        // Before the cursor is placed: grouped, the first row on the screen is the first row of
        // the first group, which is not `conversations.first` on the one axis that re-orders.
        regroup()
        place()
        // The keystroke is not finished until a frame carries it. Hand the start time to the
        // display link, which is the only thing that knows when that was.
        frames?.pendingSince = started
        inFlight = nil
    }
}
