import AppKit
import CsKit
import Foundation

/// Getting back to the conversation, which is the action the whole tool exists for.
///
/// Three rules, all of them the seam's rather than this app's.
///
/// **The destination is data, not a string to sniff.** `Group.destinations` arrives already
/// split into `terminal(argv:)` and `web(url:)`, best first, and that type exists to delete the
/// `if cmd.starts_with("http")` guess that stood in for the distinction. A GUI offering "open in
/// browser" must not be grepping for `https://` (docs/JSON-CONTRACT.md).
///
/// **An empty list is a fact, not a failure.** Some sources have no reopen path at all — Gemini
/// CLI writes no resumable session, and a Claude desktop conversation lives inside an app that
/// takes no argument. That must be reported, because a client that cannot tell "this cannot be
/// reopened from here" from "something broke" will show the second and send its reader looking
/// for a bug.
///
/// **The open and the log event are one moment** (docs/TUI-DESIGN.md §6). Every path below goes
/// through `cs pick`, including the ones that open nothing: a conversation that was wanted and
/// could not be reached is exactly as much of a relevance judgement as one that was, and picks
/// are the only judgements `chat-search-6eb.21` has to learn a ranking from.
@MainActor
enum Opener {
    /// What happened, said the way it should appear on screen. `nil` means it opened.
    ///
    /// `destination` names one of the conversation's own; `nil` takes the front of the list,
    /// which is docs/TUI-DESIGN.md §6's sticky default — the table is ordered best first and
    /// that order *is* the answer when nobody is asked.
    static func open(
        _ conv: Conversation, at destination: Destination? = nil, query: String, limit: Int,
        client: CsClient
    ) async -> String? {
        guard let destination = destination ?? conv.destinations.first else {
            // Recorded anyway, then said out loud. `destinations` is best-first and empty means
            // the source offers nothing, which is a property of the source and not of this row.
            await record(conv, query: query, limit: limit, client: client)
            return "\(conv.source) conversations have no known way to reopen — nothing to open"
        }

        switch destination {
        case .web(let url):
            guard let url = URL(string: url) else {
                return "the destination is not a URL this build can open: \(url)"
            }
            // Best effort, and deliberately not a guard: losing a log line costs a data point,
            // failing the open costs the thing that was asked for.
            await record(conv, query: query, limit: limit, client: client)
            // The platform opener, which is what `web` means. Not `open(2)` on a command line —
            // a URL is not a program name.
            guard NSWorkspace.shared.open(url) else { return "nothing on this machine opens \(url)" }
            return nil

        case .terminal:
            do {
                // The line comes back from `cs pick`, which is also where the pick is recorded.
                // **This app does not compose it.** Quoting a directory and an id into one shell
                // line is a rule with exactly one home — `cs pick`, `cs tui` and the fzf script
                // before them each grew a version of it, and a rule spread across renderers is
                // the shape the local-date bug already took.
                let line = try await client.pick(
                    conv.convId, query: query, limit: limit, kind: destination.kind)
                guard !line.isEmpty else {
                    return "\(conv.source) offered a terminal destination and no line to run"
                }
                return handOff(line, from: conv)
            } catch {
                return String(describing: error)
            }

        case .unsupported(let kind):
            // A destination kind that postdates this build. The contract calls `kind` an open
            // set and says to decode an unknown one as "cannot open this" — so this is that,
            // with the name in it, rather than a coordinated release.
            await record(conv, query: query, limit: limit, client: client)
            return "this build has no reading for a “\(kind)” destination"
        }
    }

    /// Record the pick and print nothing. For the paths this process opens itself, and for the
    /// ones that open nothing.
    private static func record(
        _ conv: Conversation, query: String, limit: Int, client: CsClient
    ) async {
        _ = try? await client.pick(conv.convId, query: query, limit: limit, kind: nil)
    }

    /// Hand a shell line to whatever this machine opens a `.command` file with.
    ///
    /// A resume is `claude --resume <id>` in a directory, and a GUI has no terminal to hand it.
    /// Writing the line to an executable file and letting Launch Services open it is the one
    /// route that needs no automation permission and respects the user's own choice of terminal
    /// — `osascript … tell app "Terminal"` names one emulator and asks for a TCC grant, and its
    /// AppleScript escaping would be a *second* quoting rule this app owned.
    ///
    /// No shebang on purpose: the shell that runs it is the one Terminal starts, so the line is
    /// resolved against the `PATH` the user actually has. A `#!/bin/sh` would run it against the
    /// one a GUI process inherits, where `claude` and `codex` are not.
    private static func handOff(_ line: String, from conv: Conversation) -> String? {
        let file = FileManager.default.temporaryDirectory
            .appendingPathComponent("cs-resume-\(conv.convId.hashValue.magnitude).command")
        do {
            try line.appending("\n").write(to: file, atomically: true, encoding: .utf8)
            // Owner only. The line names a working directory and a session id, which is as much
            // as a transcript path gives away, and /tmp is shared.
            try FileManager.default.setAttributes(
                [.posixPermissions: 0o700], ofItemAtPath: file.path)
        } catch {
            return "could not write the resume script: \(error)"
        }
        guard NSWorkspace.shared.open(file) else {
            return "nothing on this machine opens a .command file — run it yourself:\n\(line)"
        }
        return nil
    }
}
