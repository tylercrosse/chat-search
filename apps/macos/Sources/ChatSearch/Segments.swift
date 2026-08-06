import CsKit
import CsTheme

/// A steer and the run it caused, summarised in one line.
///
/// **This is temporary and is meant to be deleted.** `chat-search-me9.45` moves the run rule into
/// `cs-core`, where it belongs for the reason every other rule in this file's neighbourhood is
/// there: the row's ribbon and this transcript draw the same conversation, and two derivations of
/// "where does a run start" is exactly the shape of the local-date bug this repository already paid
/// for once. It is computed here anyway so the fidelity model can ship against the prototype's
/// design rather than waiting for the wire — but the day core answers this, the whole file goes and
/// `ReaderModel` reads the answer instead.
///
/// Why a segment is the fold unit at all: `poc/ui/NOTES.md` — "per-message is 211 toggles on a
/// 211-message conversation". Hiding the tool band without putting anything in its place would
/// leave a conversation that visibly did forty things looking like two sentences.
struct Segment: Identifiable, Sendable {
    /// Position in the conversation, which is also the identity a reader's open/shut state is
    /// keyed on. Not a message id: a segment is not a message, and the summary line is a row the
    /// conversation does not contain.
    let index: Int
    /// The user turn that opened it, if there was one. Nil for whatever came before the first
    /// steer, and for a subagent strand, which has no steer of its own.
    let steer: Block?
    /// Everything between that steer and the next one.
    let items: [Block]
    /// Calls made, which is every drawn tool-band message that is not a failure.
    ///
    /// Counted off `band` rather than off `kind`, which is not a detour: a successful `tool_result`
    /// is not drawn at all (`cs_core::blocks` — "the result is a blob whose existence the call
    /// already implies"), so the drawn tool traffic is the calls plus the failed results, and
    /// removing the failures leaves the calls. That keeps this client's one rule about `kind`
    /// intact — it never switches on it, because `band` and `fold` are the two questions it would
    /// have been asking.
    let calls: Int
    /// Results that reported a failure, from the source's own signal rather than from their text.
    let failures: Int
    /// Times the agent ended a turn on a question, which is a stand-in and says so below.
    let questions: Int
    /// Whether the query matched anywhere inside — the summary's one dot, so a closed segment can
    /// still say that what you searched for is behind it.
    let marked: Bool

    var id: Int { index }

    init(index: Int, steer: Block?, items: [Block]) {
        self.index = index
        self.steer = steer
        self.items = items
        calls = items.filter { $0.band == .tool && !$0.isError }.count
        failures = items.filter(\.isError).count
        questions = items.filter(Segment.asksSomething).count
        marked = (steer.map { !$0.marks.isEmpty } ?? false)
            || items.contains { !$0.marks.isEmpty }
    }

    /// Did the agent stop and ask?
    ///
    /// Prose from the agent ending in a question mark, which is the prototype's test and is a
    /// heuristic rather than a fact on the wire. It is worth the line anyway: 4.6% of assistant
    /// prose in this corpus ends in `?`, and "it asked you something and waited" is the one event
    /// in a forty-call run that a reader most needs to find. `chat-search-me9.45` is where it stops
    /// being a guess made in a view layer.
    ///
    /// `band == .agent` and not `kind == "prose"` for the reason `calls` gives: reasoning and tool
    /// traffic have bands of their own, so the agent band *is* the agent's prose.
    private static func asksSomething(_ block: Block) -> Bool {
        block.band == .agent && block.text.reversed().first { !$0.isWhitespace } == "?"
    }

    /// `→ 12 calls · 2 failed · asked you 1×`, as parts rather than as a string, so the two that
    /// are drawn in a colour do not have to be found again by a renderer.
    ///
    /// Pluralised, where the prototype is not. That is not tidying: `poc/ui/NOTES.md` names
    /// `→ 1 messages` after every turn of a real ChatGPT thread as the defect that made the whole
    /// mode wrong for conversational archetypes, and half of that sentence is the grammar.
    var summary: [Part] {
        var parts: [Part] = []
        if calls > 0 { parts.append(.calls(calls)) }
        if failures > 0 { parts.append(.failures(failures)) }
        if questions > 0 { parts.append(.questions(questions)) }
        // A run that ran nothing is still a run — it wrote, or it thought — and a summary with
        // nothing in it would read as a fold over nothing.
        if parts.isEmpty { parts.append(.messages(items.count)) }
        return parts
    }

    enum Part: Equatable, Sendable {
        case calls(Int)
        case failures(Int)
        case questions(Int)
        case messages(Int)

        var text: String {
            switch self {
            case .calls(let n): "\(n) call\(n == 1 ? "" : "s")"
            case .failures(let n): "\(n) failed"
            case .questions(let n): "asked you \(n)×"
            case .messages(let n): "\(n) message\(n == 1 ? "" : "s")"
            }
        }

        /// Two of the four are loud, and the departure from the prototype's stylesheet is
        /// deliberate: it draws failures in `--hit`, which in *this* window means "the query
        /// matched here" and sits on the same line as the match dot. A failure count in the match
        /// colour would state something false in the one place a reader has gone to check, which
        /// is the argument `MarkKind` already makes one layer down. `--err` is what a failure is
        /// drawn in everywhere else here, and it is what it is drawn in on this line.
        var tone: ColorToken {
            switch self {
            case .calls, .messages: .ink3
            case .failures: .err
            case .questions: .sel
            }
        }
    }

    /// The conversation cut into segments, in reading order.
    ///
    /// A new segment opens on a user turn, and also on a change of thread. The second is not in the
    /// prototype and has to be: reading order is main strand first and each strand contiguous
    /// (ADR 4), so without it every subagent in a conversation lands inside whatever segment
    /// happened to be open when the main thread ended — and where sidechains appear at all they
    /// average 52% of the conversation.
    static func over(_ blocks: [Block]) -> [Segment] {
        var out: [Segment] = []
        var steer: Block?
        var items: [Block] = []
        var thread: String?

        func close() {
            guard steer != nil || !items.isEmpty else { return }
            out.append(Segment(index: out.count, steer: steer, items: items))
            steer = nil
            items = []
        }

        for block in blocks {
            if block.band == .user || block.threadKey != thread {
                close()
                thread = block.threadKey
                if block.band == .user {
                    steer = block
                    continue
                }
            }
            items.append(block)
        }
        close()
        return out
    }
}
