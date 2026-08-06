import CsKit
import Foundation

/// Decodes both search envelopes and the facet rail out of a real index, and says what it found.
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
        await transcripts(client, into: &report)
        await rail(client, into: &report)

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
        census.append(("model null", rows.count { $0.model == nil }))
        // Not nullable, and counted anyway: `thread_count` above 1 is 1% of the corpus, which is
        // exactly the kind of state a hand test never reaches and a client draws a mark for.
        census.append(("forked", rows.count { $0.forks != nil }))
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

    // MARK: - The transcript

    /// `cs show --json`, the second contract, on the conversations the searches above returned.
    ///
    /// Real transcripts and not fixtures, for the reason the rest of this file is: the defects
    /// this check has found were all in rows nobody had looked at. It asks for the *top* result of
    /// each phrase because that is the conversation the reader will actually open, and because a
    /// top result is the one guaranteed to carry marks.
    private static func transcripts(_ client: CsClient, into report: inout Report) async {
        print("\n  transcripts — `cs show --json`, marked against the same query")
        var bands: Set<String> = []
        var folds: Set<String> = []
        var multibyteMarked = 0
        var unranked = 0

        // The four phrases, plus one filtered probe. Without it this check reads only Claude Code
        // conversations, which carry no `reasoning` — so two states a reader has to draw
        // differently, the reasoning band and the mark that may not claim to have ranked
        // anything, would go unexercised while the check said "ok".
        for phrase in Bench.phrases + ["reasoning agent:codex"] {
            guard let top = try? await client.search(phrase, limit: 3).response.results,
                !top.isEmpty
            else {
                report.failures.append("transcripts \"\(phrase)\": the search it follows failed")
                continue
            }
            for conv in top {
                let t: Transcript
                do {
                    t = try await client.show(conv.convId, query: phrase)
                } catch {
                    report.failures.append("show \(conv.convId): \(describe(error))")
                    continue
                }

                let drawn = t.drawnMessages
                check(t.count == t.messages.count, "show \(t.convId): count \(t.count) but "
                    + "\(t.messages.count) messages", into: &report)
                check(t.drawn == drawn.count, "show \(t.convId): drawn \(t.drawn) but "
                    + "\(drawn.count) messages say so", into: &report)
                check(t.markOffsets == .utf8Bytes, "show \(t.convId): "
                    + "\(marksLabel(t.markOffsets)) — this reader can no longer mark a message",
                    into: &report)
                report.messages += t.count

                for block in t.messages {
                    bands.insert(label(block.band))
                    folds.insert(block.fold == .expanded ? "expanded" : "collapsed")
                    if case .unrecognised(let raw) = block.markKind {
                        report.failures.append(
                            "show \(t.convId) \(block.msgId): mark kind \"\(raw)\" postdates this "
                                + "reader, which cannot say how loudly to mark it")
                    }
                    if !block.marks.isEmpty, block.markKind == .unranked { unranked += 1 }
                    if !block.marks.isEmpty, block.text.utf8.count != block.text.count {
                        multibyteMarked += 1
                    }
                    marks(block.text, block.marks, t.markOffsets,
                        at: "show \(t.convId) \(block.msgId)", into: &report)
                    collapsing(block, t.markOffsets, at: "show \(t.convId) \(block.msgId)",
                        into: &report)
                }
            }
        }

        print("    bands: \(bands.sorted().joined(separator: ", "))"
            + " · folds: \(folds.sorted().joined(separator: ", "))")
        print("    \(multibyteMarked) marked messages contain multi-byte characters, "
            + "\(unranked) carry marks that may not claim to have ranked anything")
        // Counted rather than asserted, for the reason `browse`'s census is: what a count adds is
        // whether this corpus still exercises the state. A band no transcript contains is a band
        // the reader is being asked to draw on trust, and saying so is cheaper than a failure
        // that fires the day someone deletes their Codex history.
        for band in Set(["user", "agent", "reasoning", "tool"]).subtracting(bands).sorted() {
            print("    note: nothing in this sample is a \"\(band)\" message, so that spine went "
                + "undrawn here")
        }
        if unranked == 0 {
            print("    note: every mark in this sample was entitled to claim it ranked, so the "
                + "quieter form went undrawn")
        }
    }

    /// The one-line form a collapsed message is drawn as has to keep every mark where it was.
    ///
    /// This is the whole reason `lineBreaksAsSpaces` is spelled in bytes. A transform that shifted
    /// so much as one offset would highlight the wrong word in the fold that 388 of 563 messages
    /// in a real agent session are drawn in — and it would do it silently, because a mark in the
    /// wrong place still looks like a mark.
    private static func collapsing(
        _ block: Block, _ units: MarkOffsets, at where_: String, into report: inout Report
    ) {
        let flat = block.text.lineBreaksAsSpaces
        guard flat.utf8.count == block.text.utf8.count else {
            report.failures.append("\(where_): collapsing moved \(block.text.utf8.count) bytes to "
                + "\(flat.utf8.count), so every mark after the first line break is now a lie")
            return
        }
        for span in block.marks {
            guard let was = block.text.range(of: span, in: units),
                let now = flat.range(of: span, in: units)
            else { continue }  // Unresolvable in either form is already `marks`'s complaint.
            let expected = String(block.text[was]).lineBreaksAsSpaces
            check(String(flat[now]) == expected, "\(where_): collapsed, span "
                + "\(span.start)–\(span.end) marks \"\(String(flat[now]).prefix(40))\" where the "
                + "message has \"\(expected.prefix(40))\"", into: &report)
        }
    }

    private static func label(_ band: Band) -> String {
        switch band {
        case .user: "user"
        case .agent: "agent"
        case .reasoning: "reasoning"
        case .tool: "tool"
        case .unrecognised(let raw): "\"\(raw)\" (unknown to this reader)"
        }
    }

    // MARK: - The facet rail

    /// The second client contract: `cs facets --json`, decoded, and its round trip closed.
    ///
    /// The rail is the one reply a client cannot sanity-check by eye, because what it carries is
    /// not data about the corpus but *query text a click will paste into the box*. So the check
    /// is the round trip: take the query a chip claims to produce, ask for the rail again, and
    /// see the chip in the state it promised. A rewriting rule that regressed on the Rust side
    /// would still serialize perfectly and would fail here.
    ///
    /// Free to run, like `browse`: the empty query is skipped by the query log, so nothing here
    /// appends to the archive's authored `queries.jsonl`.
    private static func rail(_ client: CsClient, into report: inout Report) async {
        let start: FacetRail
        do {
            start = try await client.facets("")
        } catch {
            report.failures.append("facets: \(describe(error))")
            return
        }

        let census = start.sources.values
        print("\n  facets: v\(start.v) · \(census.count) sources · \(start.indexState)")
        print("    " + census.map { "\($0.value) \($0.conversations)" }.joined(separator: " · "))

        check(start.sources.all.selected, "facets: the empty query does not read as All",
            into: &report)
        check(!census.isEmpty, "facets: no source at all, so the rail cannot be drawn",
            into: &report)
        check(census.allSatisfy { $0.state == .off },
            "facets: the empty query lit a chip", into: &report)
        for chip in census where chip.coverage == .unrecognised(chip.value) {
            print("    note: coverage state for \(chip.value) postdates this client")
        }

        // The round trip, on the chip with the most behind it — the one a reader is most likely
        // to click, and the one whose count makes a broken filter visible in the same run.
        guard let busiest = census.max(by: { $0.conversations < $1.conversations }) else { return }
        let filtered: FacetRail
        do {
            filtered = try await client.facets(busiest.query)
        } catch {
            report.failures.append("facets \"\(busiest.query)\": \(describe(error))")
            return
        }
        print("    round trip: \"\(busiest.query)\" → "
            + "\(filtered.sources.values.filter { $0.state == .include }.map(\.value).joined(separator: ", "))")

        let lit = filtered.sources.values.first { $0.value == busiest.value }
        check(lit?.state == .include,
            "facets: clicking \(busiest.value) produced a query that does not select it",
            into: &report)
        check(!filtered.sources.all.selected,
            "facets: a query naming a source still reads as All", into: &report)
        // And the chip that is on is the chip that turns it off, which is the whole affordance:
        // a rail with no way back to All through the chip you pressed is a filter you can enter
        // and not leave.
        check(lit?.query == start.query,
            "facets: \(busiest.value) does not toggle back off — its click gives "
                + "\"\(lit?.query ?? "")\"", into: &report)

        await spans(client, start, into: &report)
        await directories(client, start, into: &report)
    }

    /// The `date:` rail, whose click is the one that differs (`chat-search-1ld`).
    ///
    /// Two `date:` tokens intersect, so clicking a second span has to *replace* the first rather
    /// than widen — and a rail that widened would hand back `date:today date:>1mo`, a query that
    /// cannot match a conversation that has ever been indexed. That is the round trip worth
    /// making against a real corpus: click one span, then another, and see the list still there.
    private static func spans(
        _ client: CsClient, _ start: FacetRail, into report: inout Report
    ) async {
        guard let rail = start.dates else {
            print("    note: this `cs` has no `dates` section — nothing to check")
            return
        }
        print("    spans: "
            + rail.values.map { "\($0.label) \($0.conversations)" }.joined(separator: " · "))
        check(!rail.values.isEmpty, "facets: a rail with no spans in it", into: &report)
        check(rail.all.selected, "facets: the empty query reads as a date filter", into: &report)
        // All but the last nest, so each is at least as large as the one inside it — the last is
        // the complement of the one before it and is under no such obligation. A census counted
        // against two clocks, or joined to the wrong span, shows up here and nowhere else.
        for pair in zip(rail.values.dropLast(), rail.values.dropLast().dropFirst()) {
            check(pair.0.conversations <= pair.1.conversations,
                "facets: \(pair.0.label) holds more than \(pair.1.label), which nests it",
                into: &report)
        }

        guard let first = rail.values.first, let second = rail.values.dropFirst().first else {
            return
        }
        let after = try? await client.facets(first.query)
        guard let lit = after?.dates?.values.first(where: { $0.value == first.value }) else {
            report.failures.append("facets: \"\(first.query)\" lost the \(first.label) chip")
            return
        }
        check(lit.state == .include,
            "facets: clicking \(first.label) produced a query that does not select it",
            into: &report)

        // The replacement, which is the whole difference from `agent:`.
        let swapped = try? await client.facets(
            after?.dates?.values.first { $0.value == second.value }?.query ?? "")
        let spans = swapped?.dates?.values.filter { $0.state == .include } ?? []
        print("    round trip: \(first.label) → \(second.label) leaves "
            + "\(spans.map(\.label).joined(separator: ", "))")
        check(spans.count == 1 && spans.first?.value == second.value,
            "facets: a second span did not replace the first — \(spans.count) are lit",
            into: &report)
    }

    /// The `dir:` rail, whose chips are paths out of the index rather than a fixed vocabulary.
    ///
    /// The check that matters is that a click *narrows*: `dir:` is a substring match, so a rail
    /// handing back a fragment rather than a path would filter to far more than the chip claims,
    /// and a rail handing back a path no token can carry would filter to something else entirely
    /// (`chat-search-me9.8.16`).
    private static func directories(
        _ client: CsClient, _ start: FacetRail, into report: inout Report
    ) async {
        guard let rail = start.dirs else {
            print("    note: this `cs` has no `dirs` section — nothing to check")
            return
        }
        print("    directories: \(rail.values.count) of \(rail.indexed) drawn · "
            + "\(rail.undirected) conversations record none")
        check(rail.values.count <= rail.indexed,
            "facets: more chips than there are directories", into: &report)
        check(rail.all.selected, "facets: the empty query reads as a directory filter",
            into: &report)

        guard let busiest = rail.values.first else { return }
        guard let after = try? await client.facets(busiest.query), let lit = after.dirs else {
            report.failures.append("facets: \"\(busiest.query)\" did not come back")
            return
        }
        let chip = lit.values.first { $0.value == busiest.value }
        check(chip?.state == .include,
            "facets: clicking \(busiest.value) produced a query that does not select it",
            into: &report)
        check(chip?.query == start.query,
            "facets: \(busiest.value) does not toggle back off", into: &report)

        // And the filter reaches the results: a click that lit a chip and returned the same list
        // would be a rail that only looks like it filters.
        let all = try? await client.search("", limit: 1)
        let inside = try? await client.search(busiest.query, limit: 1)
        let (whole, part) = (all?.response.total ?? 0, inside?.response.total ?? 0)
        print("    \(busiest.value) → \(part) of \(whole)")
        check(part > 0 && part < whole,
            "facets: \(busiest.value) selected \(part) of \(whole), which is not a filter",
            into: &report)
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
