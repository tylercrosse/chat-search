import CsKit
import SwiftUI

/// A search field, a list, and a line saying what the index is doing. That is the whole shell —
/// the row's anatomy, the reader, grouping and facets are `chat-search-me9.8.2` onward, and
/// taking any of them here is what re-blocks them.
struct SearchView: View {
    @Bindable var model: SearchModel
    @FocusState private var focused: Bool

    var body: some View {
        VStack(spacing: 0) {
            field
            Divider()
            indexBanner
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
            TextField("search your conversations", text: $model.query)
                .textFieldStyle(.plain)
                .font(.system(size: 15))
                .focused($focused)
                .onChange(of: model.query) { model.queryChanged() }
        }
        .padding(10)
    }

    /// The half of the index states that arrive *with* an answer, and therefore cannot be drawn
    /// as a screen of their own without hiding the results they came with.
    ///
    /// Nothing here is styled as an error, because none of it is one. `rebuilding` means the rows
    /// below are complete and one build old — `chat-search-me9.28` swaps a finished index in
    /// whole, so there is no such thing as a partial answer — and the only thing a newer build
    /// changes is whether there are more of them, which is what "ask again" is for.
    @ViewBuilder
    private var indexBanner: some View {
        switch model.health {
        case .rebuilding:
            banner(
                icon: "arrow.triangle.2.circlepath",
                text: "a newer index is being built — these results are complete, one build behind",
                askAgain: true)
        case .unrecognised(let state):
            banner(
                icon: "questionmark.circle",
                text: "the index reports “\(state)”, which this build has no reading for — "
                    + "the results below are shown as they were returned",
                askAgain: false)
        default:
            EmptyView()
        }
    }

    private func banner(icon: String, text: String, askAgain: Bool) -> some View {
        VStack(spacing: 0) {
            HStack(spacing: 8) {
                Image(systemName: icon)
                Text(text)
                Spacer(minLength: 12)
                if askAgain {
                    Button("Ask again") { model.askAgain() }
                        .controlSize(.small)
                }
            }
            .font(.system(size: 11))
            .foregroundStyle(.secondary)
            .padding(.horizontal, 10)
            .padding(.vertical, 5)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(Color.secondary.opacity(0.08))
            Divider()
        }
    }

    @ViewBuilder
    private var content: some View {
        switch model.health {
        // The three states that came with results. The banner above is where they differ; here
        // they are the same screen, because in all three the rows are the answer.
        case .ready, .rebuilding, .unrecognised:
            if model.conversations.isEmpty {
                // Not an error state and not styled like one. An empty result is the most common
                // thing a search says.
                placeholder(
                    "no results",
                    model.query.isEmpty ? "type to search" : "nothing matched \(model.query)",
                    icon: "text.magnifyingglass", tone: .secondary)
            } else {
                // `List` and not `ScrollView { LazyVStack }`: `chat-search-me9.22` measured the
                // whole corpus through all three containers and this is the only one that
                // recycles — 5.2 MB scrolling 3,200 rows against LazyVStack's 65.6 MB and plain
                // VStack's 566 MB. The question is answered, so the app does not offer the others.
                List(model.conversations) { ResultRow(conv: $0) }
                    .listStyle(.inset)
            }
        case .noIndex(let detail):
            placeholder(
                "no index yet", "run `cs index` — this is a first-run state, not a failure\n\(detail)",
                icon: "tray", tone: .secondary)
        case .building(let detail):
            placeholder(
                "index is being built", "results arrive on their own when it finishes\n\(detail)",
                icon: "arrow.triangle.2.circlepath", tone: .secondary)
        case .stale(let detail):
            // Distinct from `noIndex` because the sentence a user needs is different: something
            // is there and cannot be read, which is not the same as nothing being there.
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

    private var footer: some View {
        HStack(spacing: 14) {
            // `total` is only a number when `settled`; otherwise it is a floor, and a poor one —
            // the typeahead `the` came back at 1,025 against a true 4,243 — so it is ranged
            // rather than printed. Saying "1,025 matches" when the answer is four times that is
            // the one thing this field exists to stop a client doing.
            Text("\(model.conversations.count) of \(model.settled ? "\(model.total)" : "\(model.total)+")")
            Spacer()
            // Kept because it costs nothing: `cs` measures its own query time and the client
            // already has a clock around the spawn, so the two numbers are decoded rather than
            // taken. A latency you cannot see while you type is a latency you will argue about.
            if let t = model.lastTiming {
                Text(String(format: "%.0f ms", t.total))
                Text(String(format: "%.1f in sqlite", t.serverMs)).foregroundStyle(.secondary)
            }
        }
        .font(.system(size: 11, design: .monospaced))
        .padding(.horizontal, 10).padding(.vertical, 6)
    }
}

/// One conversation, one line.
///
/// Deliberately plain. The row's anatomy — the agent badge, the kind ribbon, the marked snippet,
/// the date column — is `chat-search-me9.8.2`, argued against `poc/ui` and against the corpus
/// measurements in `poc/ui/NOTES.md`. A shell that guessed at it would have to be undone first.
struct ResultRow: View {
    let conv: Conversation

    var body: some View {
        HStack(spacing: 8) {
            Text(conv.title.flatMap { $0.isEmpty ? nil : $0 } ?? "untitled")
                .font(.system(size: 13))
                .foregroundStyle(.primary)
                .lineLimit(1)
                .truncationMode(.tail)
            Spacer(minLength: 12)
            Text(conv.source)
            if let date = conv.endedDate { Text(date) }
        }
        .font(.system(size: 11))
        .foregroundStyle(.secondary)
        .padding(.vertical, 2)
    }
}
