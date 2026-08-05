import Foundation

/// One round trip through the seam ADR 14 chose: argv in, JSON on stdout, exit code as part of
/// the interface. Every field is measured because the question the spike asked — where do the
/// milliseconds go — is one the app has to keep being able to answer after it moves.
public struct Timing: Sendable {
    /// `Process.run()` returning — the fork/exec itself, before `cs` has done anything.
    public var launch: Double
    /// Launch to last byte of stdout with the child reaped. Contains `serverMs`.
    public var process: Double
    /// `JSONDecoder` over the body. The cost a client pays for the seam being text.
    public var decode: Double
    /// What `cs` says it spent inside SQLite. The floor an in-process client would reach.
    public var serverMs: Double
    /// Bytes of JSON on stdout.
    public var bytes: Int

    public var total: Double { process + decode }
    /// Everything that is not the query: spawn, index open, serialise, pipe, parse. What a
    /// daemon (`--stdio`) or a C ABI would be buying.
    public var overhead: Double { total - serverMs }
}

/// Generic over the envelope, because `--flat` answers a different question and has a different
/// shape — but the same round trip and the same clock around it.
public struct QueryResult<Response: Envelope>: Sendable {
    public var response: Response
    public var timing: Timing
}

public enum CsError: Error {
    case unhealthy(IndexHealth)
    case undecodable(String, Data)
    /// `cs pick` refused: no such conversation, or a source that cannot open in the kind it was
    /// asked for. Separate from `unhealthy` because neither is a statement about the index, and
    /// classifying it as one would put "no index yet" on screen for a mistyped id.
    case pickFailed(String)
}

/// Where `cs` is and which index it should read. Both overridable so a client can be pointed at
/// a scratch index — the rebuild experiment in `poc/swift/RESULTS.md` §4 deletes and rebuilds one
/// underneath a running client, which is not something to do to the real one.
public struct CsClient: Sendable {
    public var binary: URL
    public var db: URL?
    public var config: URL?
    /// Whether this client's traffic is machine-driven — a benchmark, a scripted capture —
    /// rather than somebody looking for something.
    ///
    /// Nothing about a search says which it was, and no rule over the log can recover the
    /// difference afterwards: a query typed to measure latency is ordinary text and it goes
    /// unpicked, which is exactly what an abandoned one looks like. So the run says so at the
    /// time, by switching the log off for every `cs` it spawns. `cs-archive`'s config calls
    /// `CS_LOG_QUERIES` "a convenience rather than the mechanism" because a person has to
    /// remember to export it; a flag set from the flag that made the run scripted cannot forget.
    public var driven: Bool

    public init(binary: URL, db: URL? = nil, config: URL? = nil, driven: Bool = false) {
        self.binary = binary
        self.db = db
        self.config = config
        self.driven = driven
    }

    /// The environment every `cs` this client spawns runs in, or nil to leave the child with the
    /// one this process has.
    ///
    /// **Assign it only when it is non-nil.** `Process.environment` is documented as nil meaning
    /// "inherit", and it is — right up until something assigns nil to it, which hands the child
    /// an *empty* environment instead. A `cs` with no `HOME` cannot expand `~`, so every command
    /// fails with "no such file: ~/.config/chat-search/config.toml", which reads as a machine
    /// that was never `cs init`-ed rather than as a client that erased the environment. Measured
    /// on this Foundation, after `cs-spike contract` failed exactly that way.
    ///
    /// It is built by merging rather than replacing for the same reason: `cs` resolves `HOME`,
    /// `PATH` and the user's timezone out of it, and a driven run is still a run on this machine.
    private var childEnvironment: [String: String]? {
        guard driven else { return nil }
        return ProcessInfo.processInfo.environment.merging(["CS_LOG_QUERIES": "0"]) { _, new in
            new
        }
    }

    /// Resolution order: `CS_BIN`, then the release binary in the checkout this was built from,
    /// then PATH. The middle one is what makes a fresh checkout work with no configuration.
    public static func locate(binary override: String? = nil) -> URL? {
        if let override { return URL(fileURLWithPath: override) }
        if let env = ProcessInfo.processInfo.environment["CS_BIN"], !env.isEmpty {
            return URL(fileURLWithPath: env)
        }
        let fm = FileManager.default
        // …/apps/macos/Sources/CsKit/CsClient.swift → the repo root that built `cs`. Walked
        // rather than counted: this file has already moved once, out of `poc/swift`, and the
        // component count happened to survive — which is not a thing to rely on a second time.
        var dir = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
        while dir.path != "/" {
            let built = dir.appendingPathComponent("target/release/cs")
            if fm.isExecutableFile(atPath: built.path) { return built }
            dir = dir.deletingLastPathComponent()
        }
        for entry in (ProcessInfo.processInfo.environment["PATH"] ?? "").split(separator: ":") {
            let candidate = URL(fileURLWithPath: String(entry)).appendingPathComponent("cs")
            if fm.isExecutableFile(atPath: candidate.path) { return candidate }
        }
        return nil
    }

    public func arguments(query: String, limit: Int, prefix: Bool, flat: Bool) -> [String] {
        var args = ["search", query, "--json", "--limit", String(limit)]
        // `--prefix` is the typeahead affordance: the last word is treated as incomplete, so
        // "borrow che" still matches. Without it every keystroke inside a word returns nothing
        // and the latency question never even gets asked.
        if prefix { args.append("--prefix") }
        if flat { args.append("--flat") }
        if let db { args += ["--db", db.path] }
        if let config { args += ["--config", config.path] }
        return args
    }

    /// Matching conversations. Cancellable: a superseded keystroke terminates its process rather
    /// than waiting for it. Without this, typing eight characters leaves eight `cs` processes
    /// competing for the same 324 MB index and the last one — the only one whose answer is
    /// wanted — finishes last.
    public func search(_ query: String, limit: Int = 60, prefix: Bool = true) async throws
        -> QueryResult<SearchResponse>
    {
        try await invoke(arguments(query: query, limit: limit, prefix: prefix, flat: false))
    }

    /// Matching messages, ungrouped. A separate call and not a flag on the one above, because it
    /// comes back as a different envelope — see `FlatResponse`.
    public func searchFlat(_ query: String, limit: Int = 60, prefix: Bool = true) async throws
        -> QueryResult<FlatResponse>
    {
        try await invoke(arguments(query: query, limit: limit, prefix: prefix, flat: true))
    }

    /// `cs show <id> <query> --json`: one conversation, whole.
    ///
    /// The query is passed rather than dropped so that a highlight in the transcript means what
    /// it means in the results list — core resolves it through the same grammar and marks the
    /// same terms. An empty one marks nothing, which is the honest state for a conversation
    /// opened without a search behind it.
    public func show(_ convId: String, query: String = "") async throws -> Transcript {
        try await decoded(arguments(show: convId, query: query)).value
    }

    public func arguments(show convId: String, query: String) -> [String] {
        // The query is positional and defaulted, so it is passed even when empty rather than
        // omitted: `cs show <id> --json` would parse `--json` into the query slot.
        var args = ["show", convId, query, "--json"]
        if let db { args += ["--db", db.path] }
        if let config { args += ["--config", config.path] }
        return args
    }

    /// The facet rail for a query — every source, what the query says about it, and the query
    /// text clicking it produces (docs/JSON-CONTRACT.md).
    ///
    /// A second process beside the search rather than a field on its reply, for the reason that
    /// contract gives: the census stats the source directories, so it would have to be a key
    /// that is sometimes present, and a sometimes-absent key is a second type to a decoder.
    /// Measured at ~9 ms, which a client can afford once per keystroke.
    ///
    /// It has no refusal path. `cs facets` answers with every configured source at zero rather
    /// than failing when there is no index, because that is exactly the first-run state a rail
    /// should be able to draw.
    public func facets(_ query: String) async throws -> FacetRail {
        var args = ["facets", query, "--json"]
        if let db { args += ["--db", db.path] }
        if let config { args += ["--config", config.path] }
        return try await decoded(args).value
    }

    /// Record that a search ended in opening `convID`, and get back the line that reopens it.
    ///
    /// One call, because the open and the log event are one moment (docs/TUI-DESIGN.md §6). The
    /// rank is recomputed on the far side against the *finished* query rather than taken from
    /// the row's position in a typeahead list, which is what `chat-search-6eb.21` reads.
    ///
    /// `kind` names the destination already chosen from `Group.destinations`, so a source that
    /// cannot offer it fails loudly instead of silently reopening somewhere else. `nil` records
    /// the pick and prints nothing, which is the path for a destination this process opens
    /// itself and for a conversation that has none.
    public func pick(_ convID: String, query: String, limit: Int, kind: String?) async throws
        -> String
    {
        var args = ["pick", convID, "--query", query, "--limit", String(limit)]
        switch kind {
        case let kind?: args += ["--in", kind]
        case nil: args.append("--quiet")
        }
        if let db { args += ["--db", db.path] }
        if let config { args += ["--config", config.path] }

        let run = try await spawn(args)
        guard run.exit == 0 else {
            // Not an index-health question: `cs pick` fails when the id is not in the index or
            // when the source cannot open in the kind that was asked for, and both are worth
            // showing verbatim rather than classified.
            let why = String(decoding: run.stderr, as: UTF8.self).trimmed
            throw CsError.pickFailed(why.isEmpty ? "cs pick exited \(run.exit)" : why)
        }
        return String(decoding: run.stdout, as: UTF8.self).trimmed
    }

    public func arguments(abandon query: String, limit: Int) -> [String] {
        var args = ["abandon", query, "--limit", String(limit)]
        if let db { args += ["--db", db.path] }
        if let config { args += ["--config", config.path] }
        return args
    }

    /// Record that a search ended in nothing being opened — the other half of `pick`.
    ///
    /// A `Search` with no `Pick` after it is the abandonment signal, and it is the only thing
    /// the query log ever learns that is not a success (docs/TUI-DESIGN.md §6). There is no way
    /// to get one out of `cs search`: that path logs on the non-`--prefix` route only, which is
    /// every route a typeahead client does not take.
    ///
    /// **Whether the query is worth recording is not decided here.** `cs abandon` drops the
    /// ones with no need behind them — whitespace, `"??"`, a filter with no text beside it —
    /// and a client that re-derived that rule would be the second place it lived.
    ///
    /// Synchronous, which nothing else in this file is, and for a reason that does not
    /// generalise: the only caller is `applicationWillTerminate`, where there is no interface
    /// left to keep responsive and the process is about to stop existing. An `async` call there
    /// would be a continuation nobody is alive to resume. Waiting is also what makes the line
    /// observable — a spawn left to outlive its parent may or may not have written by the time
    /// anything goes looking, and "exactly one event" has to be a checkable claim.
    ///
    /// Best effort, like every other write to that log: `cs` missing or refusing costs a data
    /// point, and there is no window left to report it in.
    public func abandon(query: String, limit: Int) {
        let proc = Process()
        proc.executableURL = binary
        proc.arguments = arguments(abandon: query, limit: limit)
        if let childEnvironment { proc.environment = childEnvironment }
        // Nothing reads either stream, and an inherited stdout would print into whatever
        // launched this app, after the window it belonged to has gone.
        proc.standardOutput = FileHandle.nullDevice
        proc.standardError = FileHandle.nullDevice
        do { try proc.run() } catch { return }
        proc.waitUntilExit()
    }

    private func invoke<Response: Envelope>(_ args: [String]) async throws -> QueryResult<Response> {
        let (response, run, decode): (Response, Run, Double) = try await decoded(args)
        return QueryResult(
            response: response,
            timing: Timing(
                launch: run.launch, process: run.process, decode: decode,
                serverMs: response.ms, bytes: run.stdout.count))
    }

    /// One round trip whose stdout is JSON: spawn, classify a refusal, decode, time the decode.
    ///
    /// Shared by the two replies this app reads, because the refusal contract and the snake-case
    /// key strategy are the seam's rules and not one command's — a second copy is a second place
    /// for a client to start guessing at index health from prose, which is the defect
    /// `IndexHealth.classify` exists to have removed.
    private func decoded<Body: Decodable & Sendable>(_ args: [String]) async throws
        -> (value: Body, run: Run, decodeMs: Double)
    {
        let run = try await spawn(args)

        // The refusal is on stdout with the exit status, so both are needed to say what happened.
        guard run.exit == 0 else {
            throw CsError.unhealthy(
                IndexHealth.classify(
                    stdout: run.stdout, stderr: String(decoding: run.stderr, as: UTF8.self),
                    exitCode: run.exit))
        }

        let t = ContinuousClock.now
        let decoder = JSONDecoder()
        decoder.keyDecodingStrategy = .convertFromSnakeCase
        let body: Body
        do {
            body = try decoder.decode(Body.self, from: run.stdout)
        } catch {
            throw CsError.undecodable(String(describing: error), run.stdout)
        }
        return (body, run, t.duration(to: .now).ms)
    }

    public struct Run: Sendable {
        public var stdout: Data
        public var stderr: Data
        public var exit: Int32
        public var launch: Double
        public var process: Double
    }

    /// Nothing here blocks a thread, and that is the whole design.
    ///
    /// The obvious spelling — `readDataToEndOfFile()` on a background queue, then
    /// `waitUntilExit()` — costs three blocked threads per invocation. `DispatchQueue.global()`
    /// is not overcommit, so its width is the core count: eight here. Three keystrokes in flight
    /// exhaust it, the fourth waits for a thread rather than for `cs`, and the measured keystroke
    /// latency stops being a measurement of the transport at all. It read as ~80 ms of transport
    /// cost that did not exist. See `poc/swift/RESULTS.md` §2.
    ///
    /// So: readability handlers for both pipes, a termination handler for the exit status, and
    /// the continuation resumes when all three have reported. Foundation runs those on its own
    /// queues and no thread of ours is ever parked.
    private func spawn(_ args: [String]) async throws -> Run {
        let proc = Process()
        proc.executableURL = binary
        proc.arguments = args
        if let childEnvironment { proc.environment = childEnvironment }
        let out = Pipe()
        let err = Pipe()
        proc.standardOutput = out
        proc.standardError = err

        return try await withTaskCancellationHandler {
            try await withCheckedThrowingContinuation { (k: CheckedContinuation<Run, Error>) in
                let state = SpawnState(continuation: k)
                let t0 = ContinuousClock.now

                for (handle, stream) in [
                    (out.fileHandleForReading, SpawnState.Stream.out),
                    (err.fileHandleForReading, SpawnState.Stream.err),
                ] {
                    handle.readabilityHandler = { h in
                        let chunk = h.availableData
                        // Empty read is EOF. Clearing the handler is not optional: Foundation
                        // keeps polling a closed descriptor otherwise.
                        if chunk.isEmpty {
                            h.readabilityHandler = nil
                            state.close(stream, at: t0)
                        } else {
                            state.append(stream, chunk)
                        }
                    }
                }
                proc.terminationHandler = { p in state.exited(p.terminationStatus, at: t0) }

                do { try proc.run() } catch {
                    state.failed(CsError.unhealthy(.noBinary("\(binary.path): \(error)")))
                    return
                }
                state.launched(after: t0.duration(to: .now).ms)
            }
        } onCancel: {
            if proc.isRunning { proc.terminate() }
        }
    }
}

/// Three callbacks from three queues assembling one answer, and exactly one resume of the
/// continuation. A `var` shared by `@Sendable` closures is the data race Swift 6 refuses to
/// compile; a lock is the honest answer rather than an escape hatch.
private final class SpawnState: @unchecked Sendable {
    enum Stream { case out, err }

    private let lock = NSLock()
    private var k: CheckedContinuation<CsClient.Run, Error>?
    private var out = Data()
    private var err = Data()
    private var outClosed = false
    private var errClosed = false
    private var exit: Int32?
    private var launch: Double = 0
    private var elapsed: Double = 0

    init(continuation: CheckedContinuation<CsClient.Run, Error>) { k = continuation }

    func append(_ stream: Stream, _ data: Data) {
        lock.lock()
        defer { lock.unlock() }
        switch stream {
        case .out: out.append(data)
        case .err: err.append(data)
        }
    }

    func close(_ stream: Stream, at t0: ContinuousClock.Instant) {
        lock.lock()
        switch stream {
        case .out: outClosed = true
        case .err: errClosed = true
        }
        elapsed = t0.duration(to: .now).ms
        let ready = finish()
        lock.unlock()
        ready?()
    }

    func exited(_ status: Int32, at t0: ContinuousClock.Instant) {
        lock.lock()
        exit = status
        elapsed = t0.duration(to: .now).ms
        let ready = finish()
        lock.unlock()
        ready?()
    }

    func launched(after ms: Double) {
        lock.lock()
        launch = ms
        lock.unlock()
    }

    func failed(_ error: Error) {
        lock.lock()
        let k = self.k
        self.k = nil
        lock.unlock()
        k?.resume(throwing: error)
    }

    /// Called with the lock held; returns the resume to run without it, because resuming a
    /// continuation inside a lock is how a deadlock gets built.
    private func finish() -> (() -> Void)? {
        guard outClosed, errClosed, let exit, let k else { return nil }
        self.k = nil
        let run = CsClient.Run(
            stdout: out, stderr: err, exit: exit, launch: launch, process: elapsed)
        return { k.resume(returning: run) }
    }
}

extension Duration {
    public var ms: Double {
        let (s, attos) = components
        return Double(s) * 1000 + Double(attos) / 1e15
    }
}
