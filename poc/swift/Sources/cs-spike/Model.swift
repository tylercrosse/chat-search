import Foundation

// The `cs search --json` body, decoded. ADR 12 makes these field names a contract and ADR 14
// makes that contract load-bearing for every non-Rust surface, so this file is the honest test
// of it: a second implementation reading the same bytes, written without looking at the Rust
// structs that produced them.
//
// Everything here is a value type and therefore `Sendable` for free, which is what lets a
// decode happen off the main actor and land on it without a copy or a lock.

struct SearchResponse: Decodable, Sendable {
    let query: String
    let count: Int
    /// Server-side query time. `cs` measures this itself, so the gap between it and the wall
    /// clock around the spawn is exactly what a daemon or an FFI seam would buy back.
    let ms: Double
    let grouped: Bool?
    let unappliedFilters: [String]
    let results: [Conversation]
}

struct Conversation: Decodable, Sendable, Identifiable {
    let convId: String
    /// Nullable, and nothing says so. 11 of 3,059 conversations have no title, and none of them
    /// is in the first 10 rows of anything — so a client that reads this as `String` works on
    /// every hand test and then throws on a real result set. See RESULTS.md §5.
    let title: String?
    let source: String
    /// Null for 2 of 3,059: a conversation whose messages carry no timestamp has no last day.
    let endedDate: String?
    let msgCount: Int
    let proseCount: Int
    let userTurns: Int
    let matchCount: Int
    let cwd: String?
    let hits: [Hit]

    var id: String { convId }

    /// The last path component of `cwd`, which is how a project reads in a row. Absent for the
    /// 69% of the corpus that is ChatGPT, so it is a hint and never a column.
    var project: String? {
        guard let cwd, !cwd.isEmpty else { return nil }
        return URL(fileURLWithPath: cwd).lastPathComponent
    }
}

struct Hit: Decodable, Sendable, Identifiable {
    let msgId: String
    let kind: String
    let role: String
    let snippet: String
    let snippetSpans: [Span]
    let isSidechain: Bool

    var id: String { msgId }
}

/// Byte offsets into `snippet`, not character offsets — they come from Rust, where a string is
/// indexed in UTF-8. Reading them as Swift `Character` offsets silently mis-highlights every
/// snippet containing an em-dash, and this corpus is full of them.
struct Span: Decodable, Sendable {
    let start: Int
    let end: Int
}

extension String {
    /// The `Range<Index>` a Rust byte span names, or nil if it does not land on a character
    /// boundary — which would mean the two sides disagree about the encoding, not that the
    /// span is unusable, so it is worth failing visibly rather than clamping.
    func range(ofByteSpan span: Span) -> Range<Index>? {
        let utf8 = self.utf8
        guard span.start >= 0, span.end >= span.start, span.end <= utf8.count else { return nil }
        guard let lo = utf8.index(utf8.startIndex, offsetBy: span.start, limitedBy: utf8.endIndex),
              let hi = utf8.index(utf8.startIndex, offsetBy: span.end, limitedBy: utf8.endIndex),
              let loC = lo.samePosition(in: self), let hiC = hi.samePosition(in: self)
        else { return nil }
        return loC..<hiC
    }
}

/// What the client is allowed to conclude from a failed invocation.
///
/// ADR 14 requires "no index yet" and "index being rebuilt" to be first-class states rather than
/// transport errors. Classifying them is the client's job because the contract does not carry
/// them: a failure arrives as a prose line on stderr and exit 1, with no code and no JSON.
enum IndexHealth: Equatable, Sendable {
    /// Queries are being answered.
    case ready
    /// There is no usable index at the path. `cs index` is the fix, and it is the user's move.
    case noIndex(String)
    /// The index is mid-rebuild: the file exists but the tables it needs do not, or SQLite is
    /// refusing a reader. Distinguished from `noIndex` only because it resolves by waiting.
    case rebuilding(String)
    /// `cs` could not be found or could not be launched. The one failure that is genuinely
    /// transport and not corpus state.
    case noBinary(String)
    /// A nonzero exit this client has no reading for. Shown verbatim, never swallowed.
    case failed(String)

    var isReady: Bool { self == .ready }

    /// The whole classifier. Text matching against another program's prose is exactly as fragile
    /// as it looks — see RESULTS.md §4; it is reported as a finding, not defended as a design.
    static func classify(stderr: String, exitCode: Int32) -> IndexHealth {
        let s = stderr.lowercased()
        if s.contains("no importer version") || s.contains("no such table") {
            return .noIndex(stderr.trimmed)
        }
        if s.contains("database is locked") || s.contains("database disk image is malformed")
            || s.contains("attempt to write a readonly database") {
            return .rebuilding(stderr.trimmed)
        }
        if s.contains("unable to open database") || s.contains("no such file") {
            return .noIndex(stderr.trimmed)
        }
        return .failed(stderr.isEmpty ? "cs exited \(exitCode) with no message" : stderr.trimmed)
    }
}

extension String {
    var trimmed: String { trimmingCharacters(in: .whitespacesAndNewlines) }
}
