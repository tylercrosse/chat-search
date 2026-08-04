import Foundation

/// One round trip through the seam ADR 14 chose: argv in, JSON on stdout, exit code as part of
/// the interface. Every field is measured because the whole point of the spike is where the
/// milliseconds go, not that there are some.
struct Timing: Sendable {
    /// `Process.run()` returning — the fork/exec itself, before `cs` has done anything.
    var launch: Double
    /// Launch to last byte of stdout with the child reaped. Contains `serverMs`.
    var process: Double
    /// `JSONDecoder` over the body. The cost a client pays for the seam being text.
    var decode: Double
    /// What `cs` says it spent inside SQLite. The floor an in-process client would reach.
    var serverMs: Double
    /// Bytes of JSON on stdout.
    var bytes: Int

    var total: Double { process + decode }
    /// Everything that is not the query: spawn, index open, serialise, pipe, parse. What a
    /// daemon (`--stdio`) or a C ABI would be buying.
    var overhead: Double { total - serverMs }
}

struct QueryResult: Sendable {
    var response: SearchResponse
    var timing: Timing
}

enum CsError: Error {
    case unhealthy(IndexHealth)
    case undecodable(String, Data)
}

/// Where `cs` is and which index it should read. Both overridable so the spike can be pointed at
/// a scratch index — the rebuild experiment in RESULTS.md §4 deletes and rebuilds one underneath
/// a running client, which is not something to do to the real one.
struct CsClient: Sendable {
    var binary: URL
    var db: URL?
    var config: URL?

    /// Resolution order: `CS_BIN`, then the release binary built beside this package, then PATH.
    /// The middle one is what makes `swift run` work straight out of a fresh checkout.
    static func locate(binary override: String? = nil) -> URL? {
        if let override { return URL(fileURLWithPath: override) }
        if let env = ProcessInfo.processInfo.environment["CS_BIN"], !env.isEmpty {
            return URL(fileURLWithPath: env)
        }
        let fm = FileManager.default
        // …/poc/swift/Sources/cs-spike/CsClient.swift → repo root
        let repo = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent().deletingLastPathComponent()
            .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
        let built = repo.appendingPathComponent("target/release/cs")
        if fm.isExecutableFile(atPath: built.path) { return built }
        for dir in (ProcessInfo.processInfo.environment["PATH"] ?? "").split(separator: ":") {
            let candidate = URL(fileURLWithPath: String(dir)).appendingPathComponent("cs")
            if fm.isExecutableFile(atPath: candidate.path) { return candidate }
        }
        return nil
    }

    func arguments(query: String, limit: Int, prefix: Bool, flat: Bool) -> [String] {
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

    /// Cancellable: a superseded keystroke terminates its process rather than waiting for it.
    /// Without this, typing eight characters leaves eight `cs` processes competing for the same
    /// 324 MB index and the last one — the only one whose answer is wanted — finishes last.
    func search(_ query: String, limit: Int = 60, prefix: Bool = true, flat: Bool = false)
        async throws -> QueryResult
    {
        let args = arguments(query: query, limit: limit, prefix: prefix, flat: flat)
        let run = try await spawn(args)

        guard run.exit == 0 else {
            throw CsError.unhealthy(
                IndexHealth.classify(
                    stderr: String(decoding: run.stderr, as: UTF8.self), exitCode: run.exit))
        }

        let t = ContinuousClock.now
        let decoder = JSONDecoder()
        decoder.keyDecodingStrategy = .convertFromSnakeCase
        let response: SearchResponse
        do {
            response = try decoder.decode(SearchResponse.self, from: run.stdout)
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

    struct Run: Sendable {
        var stdout: Data
        var stderr: Data
        var exit: Int32
        var launch: Double
        var process: Double
    }

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
                // Blocking reads on a background thread. Both pipes are drained concurrently:
                // stderr is tiny today, but a client that reads stdout to EOF first deadlocks
                // the moment the other side fills 64 KB, and that is a bug you find in
                // production rather than in a spike.
                DispatchQueue.global(qos: .userInitiated).async {
                    let t0 = ContinuousClock.now
                    do { try proc.run() } catch {
                        k.resume(throwing: CsError.unhealthy(.noBinary("\(binary.path): \(error)")))
                        return
                    }
                    let launch = t0.duration(to: .now).ms

                    let drained = Drained()
                    let outHandle = out.fileHandleForReading
                    let errHandle = err.fileHandleForReading
                    let group = DispatchGroup()
                    group.enter()
                    DispatchQueue.global(qos: .userInitiated).async {
                        drained.put(.out, outHandle.readDataToEndOfFile())
                        group.leave()
                    }
                    group.enter()
                    DispatchQueue.global(qos: .userInitiated).async {
                        drained.put(.err, errHandle.readDataToEndOfFile())
                        group.leave()
                    }
                    group.wait()
                    proc.waitUntilExit()
                    k.resume(
                        returning: Run(
                            stdout: drained.take(.out), stderr: drained.take(.err),
                            exit: proc.terminationStatus,
                            launch: launch, process: t0.duration(to: .now).ms))
                }
            }
        } onCancel: {
            if proc.isRunning { proc.terminate() }
        }
    }
}

/// Somewhere for two background reads to land. A `var` captured by two `@Sendable` closures is
/// exactly the data race Swift 6 refuses to compile, and a lock is the honest answer rather than
/// an escape hatch.
private final class Drained: @unchecked Sendable {
    enum Stream { case out, err }
    private let lock = NSLock()
    private var out = Data()
    private var err = Data()

    func put(_ stream: Stream, _ data: Data) {
        lock.lock()
        defer { lock.unlock() }
        switch stream {
        case .out: out = data
        case .err: err = data
        }
    }

    func take(_ stream: Stream) -> Data {
        lock.lock()
        defer { lock.unlock() }
        return stream == .out ? out : err
    }
}

extension Duration {
    var ms: Double {
        let (s, attos) = components
        return Double(s) * 1000 + Double(attos) / 1e15
    }
}
