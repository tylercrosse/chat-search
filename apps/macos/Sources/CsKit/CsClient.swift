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
}

/// Where `cs` is and which index it should read. Both overridable so a client can be pointed at
/// a scratch index — the rebuild experiment in `poc/swift/RESULTS.md` §4 deletes and rebuilds one
/// underneath a running client, which is not something to do to the real one.
public struct CsClient: Sendable {
    public var binary: URL
    public var db: URL?
    public var config: URL?

    public init(binary: URL, db: URL? = nil, config: URL? = nil) {
        self.binary = binary
        self.db = db
        self.config = config
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

    private func invoke<Response: Envelope>(_ args: [String]) async throws -> QueryResult<Response> {
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
        let response: Response
        do {
            response = try decoder.decode(Response.self, from: run.stdout)
        } catch {
            throw CsError.undecodable(String(describing: error), run.stdout)
        }
        let decode = t.duration(to: .now).ms

        return QueryResult(
            response: response,
            timing: Timing(
                launch: run.launch, process: run.process, decode: decode,
                serverMs: response.ms, bytes: run.stdout.count))
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
