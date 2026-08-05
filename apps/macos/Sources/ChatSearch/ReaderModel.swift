import CsKit
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
