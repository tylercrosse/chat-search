import CsKit
import Foundation

/// Decodes both envelopes out of a real index and says what it found.
///
/// Not a measurement — a check, and until now the only kind of check this contract had from
/// outside Rust. `poc/` sits outside the cargo workspace on purpose, so `cargo test --workspace`
/// stayed green for a whole release while the repo's only non-Rust decoder could not read the
/// first conversation of the first response: `Group.hits` became `Group.matches` in
/// `chat-search-me9.36` and nothing anywhere failed (`chat-search-me9.8.7`).
///
/// So this is the hand-run that would have caught it, written down. It exits nonzero, because a
/// check nobody can put in a pipeline is a check that gets skipped.
///
/// It deliberately asks for *real* output rather than a fixture. A fixture is a copy of what the
/// contract said on the day it was copied, and every defect this spike has found — the nullable
/// `title` at `results[54]`, the UTF-8 spans — was found by decoding rows nobody had looked at.
enum Contract {
    static func check(client: CsClient, limit: Int) async -> Bool {
        print("contract — decoding `cs search --json` on both paths\n")
        var report = Report()

        for phrase in Bench.phrases {
            await grouped(client, phrase, limit: limit, into: &report)
            await flat(client, phrase, limit: limit, into: &report)
        }
        await browse(client, into: &report)

        print("")
        for failure in report.failures { print("  FAIL  \(failure)") }
        if report.failures.isEmpty {
            print("  ok — \(report.groups) conversations and \(report.messages) messages decoded, "
                + "\(report.spans) spans resolved")
        }
        return report.failures.isEmpty
    }

    private struct Report {
        var failures: [String] = []
        var groups = 0
        var messages = 0
        var spans = 0
    }

    // MARK: - The grouped envelope

    private static func grouped(
        _ client: CsClient, _ query: String, limit: Int, into report: inout Report
    ) async {
        let r: QueryResult<SearchResponse>
        do {
            r = try await client.search(query, limit: limit)
        } catch {
            report.failures.append("grouped \"\(query)\": \(describe(error))")
            return
        }
        let body = r.response

        print("  \"\(query)\" → v\(body.v) · \(body.count) of "
            + "\(body.settled ? "\(body.total)" : "\(body.total)+ (unsettled)") conversations · "
            + "\(body.indexState) · \(fmt(body.ms)) ms · \(marksLabel(body.markOffsets))")

        check(body.count == body.results.count, "grouped \"\(query)\": count \(body.count) "
            + "but \(body.results.count) results", into: &report)
        check(body.total >= body.count, "grouped \"\(query)\": total \(body.total) below "
            + "count \(body.count)", into: &report)
        check(body.markOffsets == .utf8Bytes, "grouped \"\(query)\": "
            + "\(marksLabel(body.markOffsets)) — this client can no longer mark a snippet",
            into: &report)

        report.groups += body.results.count
        for conv in body.results {
            report.messages += conv.matches.count
            for match in conv.matches {
                marks(match.snippet, match.snippetSpans, body.markOffsets,
                    at: "grouped \"\(query)\" \(match.msgId)", into: &report)
            }
        }
    }

    // MARK: - The --flat envelope

    private static func flat(
        _ client: CsClient, _ query: String, limit: Int, into report: inout Report
    ) async {
        let r: QueryResult<FlatResponse>
        do {
            r = try await client.searchFlat(query, limit: limit)
        } catch {
            report.failures.append("flat \"\(query)\": \(describe(error))")
            return
        }
        let body = r.response

        print("      --flat → v\(body.v) · \(body.count) messages · \(fmt(body.ms)) ms")

        check(body.count == body.hits.count, "flat \"\(query)\": count \(body.count) but "
            + "\(body.hits.count) hits", into: &report)

        report.messages += body.hits.count
        for hit in body.hits {
            marks(hit.snippet, hit.snippetSpans, body.markOffsets,
                at: "flat \"\(query)\" \(hit.msgId)", into: &report)
        }
    }

    // MARK: - The whole corpus

    /// The empty query, which answers with every conversation by recency. Two reasons it is the
    /// interesting one: it is the only way to decode rows nobody has ever looked at, which is
    /// where the nullable `title` was hiding, and it is free — `cs` skips the query log for an
    /// empty query, so this cannot append to the archive's authored `queries.jsonl`.
    private static func browse(_ client: CsClient, into report: inout Report) async {
        let r: QueryResult<SearchResponse>
        do {
            r = try await client.search("", limit: 100_000, prefix: false)
        } catch {
            report.failures.append("browse: \(describe(error))")
            return
        }
        let rows = r.response.results
        report.groups += rows.count

        // The nullable and the routinely-empty, counted rather than asserted. The contract says
        // which fields these are; what a count adds is whether the corpus still exercises them,
        // because a nullable field no row ever nulls is a state a client is being asked to
        // handle on trust.
        var census: [(String, Int)] = []
        census.append(("title null", rows.count { $0.title == nil }))
        census.append(("ended_at null", rows.count { $0.endedAt == nil }))
        census.append(("ended_date null", rows.count { $0.endedDate == nil }))
        census.append(("cwd null", rows.count { $0.cwd == nil }))
        census.append(("destinations []", rows.count { $0.destinations.isEmpty }))
        census.append(("kind_runs []", rows.count { $0.kindRuns.isEmpty }))
        print("\n  browse: \(rows.count) conversations in \(fmt(r.timing.total)) ms "
            + "(\(fmt(Double(r.timing.bytes) / 1024 / 1024)) MB, \(fmt(r.timing.decode)) ms decode)")
        print("    " + census.map { "\($0.0) \($0.1)" }.joined(separator: " · "))

        check(rows.allSatisfy { $0.endedAt == nil ? $0.endedDate == nil : $0.endedDate != nil },
            "browse: ended_at and ended_date disagree about being null", into: &report)
        check(rows.allSatisfy { $0.matches.isEmpty },
            "browse: the empty query matched something", into: &report)

        // Both open sets. An unknown member is not a failure — the contract says to decode one as
        // a thing this client cannot act on — but it is worth naming, because the client that
        // silently cannot open a conversation looks identical to the one that never could.
        let kinds = Set(rows.flatMap { $0.destinations.map(\.kind) })
        let bands = Set(rows.flatMap { $0.kindRuns.map(\.band) })
        print("    destination kinds: \(kinds.sorted().joined(separator: ", "))")
        print("    kind_runs bands:   \(bands.sorted().joined(separator: ", "))")
        for kind in kinds.subtracting(["terminal", "web"]).sorted() {
            print("    note: destination kind \"\(kind)\" postdates this client — it cannot open those")
        }
        for band in bands.subtracting(["user", "agent", "reasoning", "tool"]).sorted() {
            print("    note: run band \"\(band)\" postdates this client — it cannot colour those")
        }
    }

    // MARK: -

    /// Every span has to land on a character boundary in the units the envelope named. One that
    /// does not means the two sides disagree about the encoding they both wrote down, which is a
    /// defect that highlights the wrong word rather than throwing — the failure mode
    /// `mark_offsets` exists to make impossible, checked here rather than trusted.
    private static func marks(
        _ snippet: String, _ spans: [Span], _ units: MarkOffsets, at where_: String,
        into report: inout Report
    ) {
        for span in spans {
            guard snippet.range(of: span, in: units) != nil else {
                report.failures.append(
                    "\(where_): span \(span.start)–\(span.end) does not land on a character "
                        + "boundary of \"\(snippet.prefix(60))…\"")
                continue
            }
            report.spans += 1
        }
    }

    private static func check(_ ok: Bool, _ complaint: String, into report: inout Report) {
        if !ok { report.failures.append(complaint) }
    }

    private static func marksLabel(_ m: MarkOffsets) -> String {
        switch m {
        case .utf8Bytes: "utf8-bytes"
        case .unknown(let raw): "marks in \"\(raw)\", which this client cannot read"
        }
    }

    /// `CsError.undecodable` carries the bytes on purpose: the whole value of this check is
    /// saying *what* did not decode, and `keyNotFound(matches)` with no body beside it is a
    /// sentence somebody then has to reproduce by hand.
    private static func describe(_ error: Error) -> String {
        switch error {
        case CsError.undecodable(let why, let data):
            "did not decode — \(why)\n        first 300 bytes: "
                + String(decoding: data.prefix(300), as: UTF8.self)
        case CsError.unhealthy(let health):
            Bench.describe(health)
        default:
            "\(error)"
        }
    }
}
