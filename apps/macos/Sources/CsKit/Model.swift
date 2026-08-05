import Foundation

// The `cs search --json` body, decoded. ADR 12 makes these field names a contract and ADR 14
// makes that contract load-bearing for every non-Rust surface, so this file is the honest test
// of it: a second implementation reading the same bytes, written against `docs/JSON-CONTRACT.md`
// rather than against the Rust structs that produce them.
//
// It is `public` because it is written once. The app renders these types and `poc/swift`'s
// contract check decodes with them, and a second copy of this file would mean every field added
// to the contract lands twice — which is the duplication `chat-search-me9.36` was sequenced ahead
// of the app to prevent.
//
// Everything here is a value type and therefore `Sendable` for free, which is what lets a
// decode happen off the main actor and land on it without a copy or a lock.

/// What both envelopes have in common, which is only enough to time a round trip, read a snippet
/// and say what was at the index path. They are otherwise separate shapes, and this deliberately
/// does not paper over that.
public protocol Envelope: Decodable, Sendable {
    /// Server-side query time. `cs` measures this itself, so the gap between it and the wall
    /// clock around the spawn is exactly what a daemon or an FFI seam would buy back.
    var ms: Double { get }
    /// How to read every `snippetSpans` below. On the envelope rather than beside the offsets
    /// because one answer describes all of them.
    var markOffsets: MarkOffsets { get }
    /// `ready` or `rebuilding` — see `SearchResponse.indexState`. Here rather than on one shape
    /// because a client that reads it off the grouped envelope only would tell the truth about
    /// the index on one path and say nothing on the other.
    var indexState: String { get }
}

/// The default envelope: matching **conversations**, each carrying its best matching messages.
public struct SearchResponse: Envelope {
    /// Contract version. Moves only when a field changes meaning — an addition is silent and a
    /// rename fails loudly at the missing key — so an unexpected value means this reply was
    /// written for somebody else.
    public let v: Int
    public let query: String
    public let ms: Double
    /// `results.count`. Not how many conversations match: `--limit` truncates before this.
    public let count: Int
    /// How many conversations the query selects with `--limit` ignored. Read it beside
    /// `settled`: a hundred rows out of a hundred and a hundred out of two thousand are the same
    /// hundred rows, and this is what says which.
    public let total: Int
    /// False means `total` is a floor and a poor one, to be ranged rather than displayed. Only
    /// `--prefix` can produce it — which is exactly the mode this client types in.
    public let settled: Bool
    /// Filter tokens whose value selects nothing. Non-empty is not an error and the exit status
    /// stays 0, so a client that ignores this shows unfiltered results for a filtered query.
    public let unappliedFilters: [String]
    /// `ready` or `rebuilding`, and **both mean these results are complete** — a rebuild
    /// assembles a sibling and swaps it in whole. Branch on the name, never on prose. The other
    /// two index states cannot appear here: with `no_index` or `building` there is no readable
    /// index to answer out of, so they arrive as a refusal instead. `IndexHealth.answering` is
    /// the one place this name is read.
    public let indexState: String
    public let markOffsets: MarkOffsets
    public let results: [Conversation]
}

/// `cs search --flat --json`: matching **messages**, ungrouped, each naming its own conversation.
///
/// A separate type and not a variant of the one above, because the wire is two shapes on purpose
/// (`docs/JSON-CONTRACT.md`). One envelope covering both would need a discriminator every decoder
/// has to model for a case it never asks for — which is what `grouped` was, and this file went on
/// believing in it for a release after `chat-search-me9.36` stopped emitting it.
///
/// No `total` or `settled`: the number worth settling counts conversations, and this envelope is
/// not about conversations.
public struct FlatResponse: Envelope {
    public let v: Int
    public let query: String
    public let ms: Double
    /// `hits.count`.
    public let count: Int
    public let unappliedFilters: [String]
    public let indexState: String
    public let markOffsets: MarkOffsets
    public let hits: [Hit]
}

/// The units `snippetSpans` are in, read off the wire instead of assumed.
///
/// The assumption is what `chat-search-me9.33` is about: this client read the offsets as Swift
/// `Character` positions and mis-highlighted every snippet containing an em-dash. A wrong offset
/// highlights the wrong word rather than failing, so the encoding has to travel with the offsets
/// it describes — and the client that guessed is the one that has to branch on it.
public enum MarkOffsets: Decodable, Sendable, Equatable {
    /// UTF-8 byte offsets, half-open. The only sane choice coming out of Rust, and not what a
    /// Swift, Python or JavaScript client assumes.
    case utf8Bytes
    /// An encoding this client has no reading for. Draw the snippet unmarked: no marks is a
    /// visible loss, marks in the wrong place is an invisible lie.
    case unknown(String)

    public init(from decoder: any Decoder) throws {
        let raw = try decoder.singleValueContainer().decode(String.self)
        self = raw == "utf8-bytes" ? .utf8Bytes : .unknown(raw)
    }
}

/// One conversation — the contract's `Group`. Its identity is stated here once and never
/// repeated inside `matches`.
public struct Conversation: Decodable, Sendable, Identifiable {
    public let convId: String
    public let source: String
    public let nativeId: String
    /// Every way to reopen this, best first. **May be empty**, which means this source has no
    /// known way to reopen a conversation — all 12 `gemini-cli` rows today — rather than that
    /// building one failed. Empty disables the affordance; it never invents a fallback.
    public let destinations: [Destination]
    /// Nullable, and nothing said so until this client threw on it. 11 of 3,059 conversations
    /// have no title, and none of them is in the first 10 rows of anything — so a client that
    /// reads this as `String` works on every hand test and then throws on a real result set.
    /// See RESULTS.md §5 and `chat-search-me9.27`.
    public let title: String?
    /// Epoch milliseconds of the last message. Null for the 2 conversations that never had one.
    public let endedAt: Int?
    /// `endedAt` as a **local** `YYYY-MM-DD`, rendered by the core so that three clients do not
    /// each get their own chance to derive it in UTC. Null exactly when `endedAt` is.
    public let endedDate: String?
    public let userTurns: Int
    public let msgCount: Int
    public let proseCount: Int
    public let cwd: String?
    /// Opaque ordering. The rows arrive best first and that order *is* the ranking, so this is
    /// carried to be shown and never to re-sort on.
    public let score: Double
    /// Total matching messages, including any beyond the `matches` returned.
    public let matchCount: Int
    /// Where the matches sit, as 0-based positions into `msgCount` — which is *not* the
    /// coordinate space `kindRuns` counts in (`chat-search-me9.25`).
    public let matchSeqs: [Int]
    /// What the conversation is made of, in reading order, run-length encoded.
    public let kindRuns: [Run]
    /// Gone from its source and kept here (ADR 9).
    public let deletedUpstream: Bool
    /// The best matching messages, best first, up to `--nested`. Truncate rather than sample:
    /// the first entry is the message that won the ranking. Empty for an empty query, which
    /// returns conversations without matching anything.
    public let matches: [Match]

    public var id: String { convId }

    /// The last path component of `cwd`, which is how a project reads in a row. Absent for the
    /// 69% of the corpus that is ChatGPT, so it is a hint and never a column.
    public var project: String? {
        guard let cwd, !cwd.isEmpty else { return nil }
        return URL(fileURLWithPath: cwd).lastPathComponent
    }
}

/// One matching message under a `Conversation`. **Message fields only** — `convId`, `source`,
/// `nativeId`, `title`, `destinations` and `deletedUpstream` are stated on the group that holds
/// it rather than repeated under every nested row.
public struct Match: Decodable, Sendable, Identifiable {
    public let msgId: String
    public let role: String
    public let kind: String
    /// Never null on the current corpus and nullable nonetheless: every importer reads it from
    /// an optional field. The field most likely to be typed non-optional on the evidence of a
    /// `SELECT`, and the one where that would be a bet on the corpus rather than on the contract.
    public let ts: Int?
    public let score: Double
    /// A window of the message around what matched, whitespace flattened. May begin with the
    /// literal `⟨no match⟩ ` prefix, which is a label rather than content.
    public let snippet: String
    /// Where to highlight, in the units the envelope's `markOffsets` names. May be empty, which
    /// is an ordinary outcome and not a failure — draw the snippet unmarked rather than
    /// re-tokenizing it.
    public let snippetSpans: [Span]
    /// False when the message sits on a branch that was edited away. Only ever false when
    /// `--include-off-path` was passed.
    public let onHeadPath: Bool
    /// From a subagent thread, which runs parallel to the main one rather than being an
    /// abandoned branch.
    public let isSidechain: Bool
    /// Which thread within the conversation. Opaque — never parsed.
    public let threadKey: String

    public var id: String { msgId }
}

/// One message under `--flat`, and the one place a message row names its own parent. That is
/// right in a list with no parent rows in it and wrong the moment there are, which is why this
/// shape appears under `--flat` and nowhere else.
public struct Hit: Decodable, Sendable, Identifiable {
    public let convId: String
    public let msgId: String
    public let source: String
    /// The **conversation's** native id, not the message's.
    public let nativeId: String
    public let destinations: [Destination]
    /// The conversation's title, carried so a flat row renders without a second lookup. Null
    /// under exactly the same conditions as `Conversation.title`.
    public let title: String?
    public let role: String
    public let kind: String
    public let ts: Int?
    public let score: Double
    public let snippet: String
    public let snippetSpans: [Span]
    public let onHeadPath: Bool
    public let isSidechain: Bool
    public let threadKey: String
    public let deletedUpstream: Bool

    public var id: String { msgId }
}

/// A way to reopen a conversation, tagged on `kind` with a payload that differs by variant — a
/// URL is not a command, and this type exists to delete the `if cmd.starts_with("http")` guess
/// that stood in for the distinction.
public enum Destination: Decodable, Sendable, Equatable {
    /// Already split into words. Run it with no shell in between, in the conversation's `cwd`.
    case terminal(argv: [String])
    /// For the platform opener (`open`, `xdg-open`). Not a program name.
    case web(url: String)
    /// A kind that postdates this client. `kind` is an open set and the contract says to decode
    /// an unknown variant as "cannot open this" rather than as an error — the alternative makes
    /// every addition a coordinated release.
    case unsupported(kind: String)

    private enum Key: String, CodingKey { case kind, argv, url }

    public init(from decoder: any Decoder) throws {
        let c = try decoder.container(keyedBy: Key.self)
        switch try c.decode(String.self, forKey: .kind) {
        case "terminal": self = .terminal(argv: try c.decode([String].self, forKey: .argv))
        case "web": self = .web(url: try c.decode(String.self, forKey: .url))
        case let other: self = .unsupported(kind: other)
        }
    }

    public var kind: String {
        switch self {
        case .terminal: "terminal"
        case .web: "web"
        case .unsupported(let k): k
        }
    }
}

/// A stretch of the conversation that is all one thing, run-length encoded: `["tool", 12]`.
///
/// A two-element array on the wire rather than an object, because there are a great many of these
/// and `{"band":"tool","n":12}` is four times the bytes for the same two facts. `band` is an open
/// set — decode an unknown one as a run this client cannot colour, not as an error.
public struct Run: Decodable, Sendable, Equatable {
    public let band: String
    public let length: Int

    public init(from decoder: any Decoder) throws {
        var c = try decoder.unkeyedContainer()
        band = try c.decode(String.self)
        length = try c.decode(Int.self)
    }
}

/// Half-open offsets into `snippet`, in the units the envelope's `markOffsets` names. They index
/// the *returned string*, ellipsis and `⟨no match⟩ ` prefix included, and nothing downstream can
/// re-derive them: the window has been cut out of the message, its whitespace flattened, and the
/// term that matched need not appear in the query — `commits` marks `Commit`.
public struct Span: Decodable, Sendable, Equatable {
    public let start: Int
    public let end: Int
}

extension String {
    /// The `Range<Index>` a span names, or nil if this client cannot honour it.
    ///
    /// Two ways that happens, and they are not the same thing. The envelope may name an encoding
    /// this client has no reading for, in which case guessing would silently highlight the wrong
    /// word. Or the offsets may not land on a character boundary, which means the two sides
    /// disagree about the encoding they both named — worth failing visibly rather than clamping.
    public func range(of span: Span, in units: MarkOffsets) -> Range<Index>? {
        guard units == .utf8Bytes else { return nil }
        let utf8 = self.utf8
        guard span.start >= 0, span.end >= span.start, span.end <= utf8.count else { return nil }
        guard let lo = utf8.index(utf8.startIndex, offsetBy: span.start, limitedBy: utf8.endIndex),
              let hi = utf8.index(utf8.startIndex, offsetBy: span.end, limitedBy: utf8.endIndex),
              let loC = lo.samePosition(in: self), let hiC = hi.samePosition(in: self)
        else { return nil }
        return loC..<hiC
    }
}

/// The body `cs` puts on stdout in place of an envelope when it refuses to search. The nonzero
/// exit status says a search was not answered; this says why.
struct Refusal: Decodable, Sendable {
    struct Body: Decodable, Sendable {
        /// The contract. Switch on it.
        let code: String
        /// Free to be reworded, and therefore never parsed — only shown.
        let message: String
    }

    let error: Body
}

/// What a client is allowed to conclude about the index, from either half of the contract.
///
/// The four index states arrive on two paths and this is where they are put back together.
/// `ready` and `rebuilding` are a field on an answered envelope; `no_index` and `building` are an
/// `error.code` on a refusal, because in both there is nothing to answer out of. A client that
/// models only the refusal half can say "there is no index" and cannot say "this answer is one
/// build behind" — which is the thing `chat-search-me9.28` made sayable by swapping a finished
/// index in whole, and the thing this client threw away by setting `ready` on every success.
///
/// ADR 14 requires "no index yet" and "index being rebuilt" to be first-class states rather than
/// transport errors. This client is why the codes exist at all — the first non-Rust reader of the
/// contract had one English sentence on stderr and classified index health by substring-matching
/// another program's prose (RESULTS.md §4). That classifier is gone; this reads the names.
public enum IndexHealth: Equatable, Sendable {
    /// Answered out of a complete index, with nothing being built beside it.
    case ready
    /// Answered out of a complete index **in full**, while a newer one is built beside it. Not a
    /// degraded state and not a loading state: the results are whole and one build old, and a
    /// client that knows the difference can offer to ask again instead of presenting them as the
    /// last word.
    case rebuilding
    /// Answered, and the envelope named an index state this build has no reading for. The
    /// results are in hand, so it is not a failure — but folding it into `ready` would file an
    /// added state under "nothing to say", and an index state nobody notices is the one that goes
    /// wrong quietly.
    case unrecognised(state: String)
    /// There is no index at the path and nothing building one. `cs index` is the fix, and it is
    /// the user's move.
    case noIndex(String)
    /// A build is in progress and there is nothing readable yet. Resolves by waiting, which is
    /// what distinguishes it from `noIndex`. A *rebuild* over an existing index never reaches
    /// here: since `chat-search-me9.28` it assembles a sibling and swaps it in whole, so queries
    /// go on being answered out of the old one — as `rebuilding` above — until it is complete.
    case building(String)
    /// Something is at the path that this build of `cs` cannot read: an older schema, or bytes
    /// that are not an index at all. The fix is the same command as for `noIndex`, but saying
    /// which is what stops a first run being reported as a corrupt index and the reverse.
    case stale(String)
    /// `cs` could not be found or could not be launched. The one failure that is genuinely
    /// transport and not corpus state.
    case noBinary(String)
    /// A nonzero exit this client has no reading for. Shown verbatim, never swallowed.
    case failed(String)

    /// Whether this state arrived with results in hand. Three of the eight do, and they are the
    /// three a client must not draw as an empty screen.
    public var isAnswered: Bool {
        switch self {
        case .ready, .rebuilding, .unrecognised: true
        default: false
        }
    }

    /// What a **successful** envelope's `index_state` says. An unrecognised name is carried
    /// rather than folded into `ready`, for the same reason `Destination` has an `unsupported`
    /// arm: an added value should cost a line of prose on screen, not a coordinated release.
    public static func answering(_ indexState: String) -> IndexHealth {
        switch indexState {
        case "ready": .ready
        case "rebuilding": .rebuilding
        case let other: .unrecognised(state: other)
        }
    }

    /// Reads the refusal rather than the prose beside it. An unrecognised `code` becomes
    /// `failed` carrying the code: guessing at an unknown state is how the substring-matching
    /// this replaced came to be written.
    public static func classify(stdout: Data, stderr: String, exitCode: Int32) -> IndexHealth {
        guard let refusal = try? JSONDecoder().decode(Refusal.self, from: stdout) else {
            return .failed(stderr.isEmpty ? "cs exited \(exitCode) with no message" : stderr.trimmed)
        }
        switch refusal.error.code {
        case "no_index": return .noIndex(refusal.error.message)
        case "building": return .building(refusal.error.message)
        case "index_stale": return .stale(refusal.error.message)
        case let code: return .failed("\(code): \(refusal.error.message)")
        }
    }
}

extension String {
    var trimmed: String { trimmingCharacters(in: .whitespacesAndNewlines) }
}
