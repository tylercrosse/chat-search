import AppKit
import Observation
import SwiftUI

/// How the 3,000-row list is built. The whole of question 2 is this enum: SwiftUI offers three
/// ways to put rows on screen and they are not interchangeable at corpus scale.
enum RowContainer: String, CaseIterable, Identifiable {
    /// `List` — NSTableView underneath on macOS. Lazy *and* recycling.
    case list
    /// `ScrollView { LazyVStack }` — lazy, but every row ever shown stays built.
    case lazyStack
    /// `ScrollView { VStack }` — builds all of them, immediately. The control case.
    case eagerStack

    var id: String { rawValue }
    var label: String {
        switch self {
        case .list: "List"
        case .lazyStack: "LazyVStack"
        case .eagerStack: "VStack"
        }
    }
}

@MainActor
@Observable
final class SearchModel {
    var query = ""
    var conversations: [Conversation] = []
    /// Read off the last response rather than assumed. A row cannot mark a snippet without
    /// knowing what units its offsets are in, and this client is the one that once guessed.
    var marks: MarkOffsets = .utf8Bytes
    /// How many conversations match with `--limit` ignored, and whether that number is whole.
    /// `--prefix` is on for every keystroke here, so the unsettled case is the common one.
    var total = 0
    var settled = true
    var health: IndexHealth = .ready
    var lastTiming: Timing?
    var container: RowContainer = .list
    /// Kept for the footer readout, so the number is visible while typing rather than only in a
    /// log afterwards. A latency you cannot see while you type is a latency you will argue about.
    var toData = Samples()
    var supersededCount = 0
    var queryCount = 0

    let client: CsClient
    var limit: Int
    /// Owned by the host window, because the honest end of a keystroke is a frame and only the
    /// display link knows when one happened.
    var recorder: FrameRecorder?

    private var inFlight: Task<Void, Never>?
    /// `NSEvent.timestamp` of the keystroke this query belongs to, on the same clock as
    /// `CACurrentMediaTime()` — so the measurement starts at the hardware event and not at the
    /// point SwiftUI got round to telling us about it.
    private var keystrokeAt: Double?

    init(client: CsClient, limit: Int = 60) {
        self.client = client
        self.limit = limit
    }

    func noteKeystroke(at t: Double) { keystrokeAt = t }

    /// One query per keystroke, the previous one killed. Deliberately no debounce: the bead's
    /// first question is whether a process per keystroke is fast enough, and a debounce would
    /// answer a different question by hiding the one being asked.
    func queryChanged() {
        let text = query
        if inFlight != nil { supersededCount += 1 }
        inFlight?.cancel()
        let started = keystrokeAt ?? CACurrentMediaTime()
        inFlight = Task { [weak self] in
            guard let self else { return }
            do {
                let result = try await client.search(text, limit: limit)
                guard !Task.isCancelled else { return }
                self.apply(result, keystrokeAt: started)
            } catch is CancellationError {
                return
            } catch CsError.unhealthy(let h) {
                guard !Task.isCancelled else { return }
                self.health = h
                self.conversations = []
            } catch {
                guard !Task.isCancelled else { return }
                self.health = .failed(String(describing: error))
            }
        }
    }

    private func apply(_ result: QueryResult<SearchResponse>, keystrokeAt started: Double) {
        health = .ready
        conversations = result.response.results
        marks = result.response.markOffsets
        total = result.response.total
        settled = result.response.settled
        lastTiming = result.timing
        queryCount += 1
        toData.add((CACurrentMediaTime() - started) * 1000)
        // The keystroke is not finished until a frame carries it. Hand the start time to the
        // display link, which is the only thing that knows when that was.
        recorder?.pendingSince = started
        inFlight = nil
    }

    func load(_ rows: [Conversation]) {
        conversations = rows
        health = .ready
    }
}

struct SearchView: View {
    @Bindable var model: SearchModel
    @FocusState private var focused: Bool

    var body: some View {
        VStack(spacing: 0) {
            field
            Divider()
            content
            Divider()
            footer
        }
        .frame(minWidth: 720, minHeight: 480)
        .onAppear { focused = true }
    }

    private var field: some View {
        HStack(spacing: 8) {
            Image(systemName: "magnifyingglass").foregroundStyle(.secondary)
            TextField("search 3,059 conversations", text: $model.query)
                .textFieldStyle(.plain)
                .font(.system(size: 15))
                .focused($focused)
                .onChange(of: model.query) { model.queryChanged() }
            Picker("", selection: $model.container) {
                ForEach(RowContainer.allCases) { Text($0.label).tag($0) }
            }
            .pickerStyle(.segmented)
            .frame(width: 240)
            .help("Which container builds the rows — the virtualisation question, live")
        }
        .padding(10)
    }

    @ViewBuilder
    private var content: some View {
        switch model.health {
        case .ready:
            if model.conversations.isEmpty {
                // Not an error state and not styled like one. An empty result is the most common
                // thing a search says.
                placeholder(
                    "no results", model.query.isEmpty ? "type to search" : "nothing matched \(model.query)",
                    icon: "text.magnifyingglass", tone: .secondary)
            } else {
                rows
            }
        case .noIndex(let detail):
            placeholder(
                "no index yet", "run `cs index` — this is a first-run state, not a failure\n\(detail)",
                icon: "tray", tone: .secondary)
        case .rebuilding(let detail):
            placeholder(
                "index is being built", "results arrive on their own when it finishes\n\(detail)",
                icon: "arrow.triangle.2.circlepath", tone: .secondary)
        case .stale(let detail):
            // Distinct from `noIndex` because the sentence a user needs is different: something
            // is there and cannot be read, which is not the same as nothing being there. The two
            // were one screen while the client had only prose to go on.
            placeholder(
                "index cannot be read", "run `cs index` to rebuild it\n\(detail)",
                icon: "questionmark.folder", tone: .secondary)
        case .noBinary(let detail):
            placeholder("cs not found", detail, icon: "exclamationmark.triangle", tone: .red)
        case .failed(let detail):
            placeholder("cs failed", detail, icon: "exclamationmark.triangle", tone: .red)
        }
    }

    private func placeholder(_ title: String, _ detail: String, icon: String, tone: Color) -> some View {
        VStack(spacing: 8) {
            Image(systemName: icon).font(.system(size: 28)).foregroundStyle(tone)
            Text(title).font(.headline)
            Text(detail).font(.callout).foregroundStyle(.secondary)
                .multilineTextAlignment(.center).textSelection(.enabled)
        }
        .padding(40)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    @ViewBuilder
    private var rows: some View {
        switch model.container {
        case .list:
            List(model.conversations) { ConversationRow(conv: $0, marks: model.marks) }
                .listStyle(.inset)
        case .lazyStack:
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 0) {
                    ForEach(model.conversations) {
                        ConversationRow(conv: $0, marks: model.marks).padding(.horizontal, 8)
                    }
                }
            }
        case .eagerStack:
            ScrollView {
                VStack(alignment: .leading, spacing: 0) {
                    ForEach(model.conversations) {
                        ConversationRow(conv: $0, marks: model.marks).padding(.horizontal, 8)
                    }
                }
            }
        }
    }

    private var footer: some View {
        HStack(spacing: 14) {
            // `total` is only a number when `settled`; otherwise it is a floor, and a poor one —
            // the typeahead `the` came back at 1,025 against a true 4,243 — so it is ranged
            // rather than printed. Saying "1,025 matches" when the answer is four times that is
            // the one thing this field exists to stop a client doing.
            Text("\(model.conversations.count) of \(model.settled ? "\(model.total)" : "\(model.total)+")")
            if let t = model.lastTiming {
                Text(String(format: "%.0f ms round trip", t.total))
                Text(String(format: "%.1f in sqlite", t.serverMs)).foregroundStyle(.secondary)
                Text(String(format: "%.0f KB", Double(t.bytes) / 1024)).foregroundStyle(.secondary)
            }
            Spacer()
            if let f = model.recorder, f.toFrame.count > 0 {
                Text(String(format: "keystroke→frame p50 %.0f / p95 %.0f ms", f.toFrame.p50, f.toFrame.p95))
            }
            Text("\(model.supersededCount) killed").foregroundStyle(.secondary)
        }
        .font(.system(size: 11, design: .monospaced))
        .foregroundStyle(.primary)
        .padding(.horizontal, 10).padding(.vertical, 6)
    }
}

struct ConversationRow: View {
    let conv: Conversation
    /// Handed down from the response the row came out of, because the units the marks are in are
    /// a fact about that response and not about this view.
    let marks: MarkOffsets

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(conv.title.flatMap { $0.isEmpty ? nil : $0 } ?? "untitled")
                .font(.system(size: 13, weight: .medium))
                .lineLimit(1)
            HStack(spacing: 6) {
                Text(conv.source)
                if let p = conv.project { Text("· \(p)") }
                if let d = conv.endedDate { Text("· \(d)") }
                Text("· \(conv.msgCount) msgs")
                if conv.matchCount > 0 { Text("· \(conv.matchCount) matches") }
            }
            .font(.system(size: 10))
            .foregroundStyle(.secondary)
            .lineLimit(1)
            // The best matching message, which is the first: `matches` is ranked, so truncating
            // it takes the message that won rather than an arbitrary one.
            if let match = conv.matches.first {
                Text(highlighted(match))
                    .font(.system(size: 11))
                    .lineLimit(2)
            }
        }
        .padding(.vertical, 4)
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    /// Marks the spans `cs` found, rather than re-finding them here. `cs` stems, so the client
    /// cannot reproduce the marks by substring search — `[Checker]` is marked for the query
    /// `check` and no naive client would find it.
    private func highlighted(_ match: Match) -> AttributedString {
        var s = AttributedString(match.snippet)
        for span in match.snippetSpans {
            guard let r = match.snippet.range(of: span, in: marks),
                  let lo = AttributedString.Index(r.lowerBound, within: s),
                  let hi = AttributedString.Index(r.upperBound, within: s)
            else { continue }
            s[lo..<hi].inlinePresentationIntent = .stronglyEmphasized
            s[lo..<hi].foregroundColor = .accentColor
        }
        return s
    }
}
