import CsKit
import CsTheme
import SwiftUI

/// A search field, a list, a drawer, and a line saying what the index is doing.
///
/// The drawer is `ReaderView` and everything about it lives there; what is here is the seam it
/// hangs on — a selection, and where it sits beside the results. The row's anatomy, grouping and
/// facets are `chat-search-me9.8.2` onward, and taking any of them here is what re-blocks them.
///
/// Nothing below names a colour, a size or a face. Every one is a token off `\.theme`
/// (`chat-search-me9.8.8`), which is what makes a change of direction a change to one generated
/// file. Where a value here differs from the plain one it replaced, the mockup is why: the search
/// field is monospaced because `.qbox` is, the footer is `--fs-meta` because `.fbar` is, and the
/// empty state has a dashed rule around it because `.empty` does.
struct SearchView: View {
    @Bindable var model: SearchModel
    @Environment(\.theme) private var theme
    @FocusState private var focused: Bool

    var body: some View {
        VStack(spacing: 0) {
            field
            rule
            indexBanner
            content
            rule
            footer
        }
        .frame(minWidth: 720, minHeight: 480)
        .background(theme.color(.bg))
        .onAppear { focused = true }
    }

    /// `Divider()` draws a system separator, which is a colour this app did not choose. One point
    /// of `--rule` is the same line and is the theme's.
    private var rule: some View {
        Rectangle().fill(theme.color(.rule)).frame(height: 1)
    }

    /// Which conversation the drawer has open, as the list's selection.
    ///
    /// A binding built here rather than a property on the model, because the two are one gesture:
    /// selecting a row opens it and deselecting closes it, so there is no state to keep in step.
    /// The reader is handed the whole row and not its id, which is what lets the drawer stay open
    /// when the query moves on and the row it came from is no longer in the results.
    private var opened: Binding<String?> {
        Binding(
            get: { model.reader.conv?.id },
            set: { id in
                model.reader.open(
                    model.conversations.first { $0.id == id }, query: model.query)
            })
    }

    private var field: some View {
        HStack(spacing: 8) {
            Image(systemName: "magnifyingglass").foregroundStyle(theme.color(.ink3))
            TextField("search your conversations", text: $model.query)
                .textFieldStyle(.plain)
                // Monospaced, and at the body size rather than larger: `.qbox` in the mockup is
                // `--mono` at `--fs-body`, because the query is a grammar — `agent:`, `after:` —
                // and a proportional face makes a typed grammar look like prose.
                .font(theme.font(.body, .mono))
                .foregroundStyle(theme.color(.ink))
                .focused($focused)
                .onChange(of: model.query) { model.queryChanged() }
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 7)
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
                    Button {
                        model.askAgain()
                    } label: {
                        Text("Ask again")
                            .font(theme.font(.meta, .mono))
                            .foregroundStyle(theme.color(.sel))
                            .padding(.horizontal, 8)
                            .padding(.vertical, 3)
                            .background(
                                RoundedRectangle(cornerRadius: theme.metric(.r3))
                                    .fill(theme.color(.selBg)))
                    }
                    // `.plain`, because a stock button paints itself in the system accent — a
                    // colour chosen somewhere this app cannot see. The mockup's pressed control
                    // is `--sel` on `--sel-bg`, which is the same affordance in the theme's own
                    // palette.
                    .buttonStyle(.plain)
                }
            }
            .font(theme.font(.sub))
            .foregroundStyle(theme.color(.ink2))
            .padding(.horizontal, 12)
            .padding(.vertical, 5)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(theme.color(.panel2))
            rule
        }
    }

    @ViewBuilder
    private var content: some View {
        switch model.health {
        // The three states that came with results. The banner above is where they differ; here
        // they are the same screen, because in all three the rows are the answer.
        case .ready, .rebuilding, .unrecognised:
            HStack(spacing: 0) {
                if model.conversations.isEmpty {
                    // Not an error state and not styled like one. An empty result is the most
                    // common thing a search says.
                    placeholder(
                        "no results",
                        model.query.isEmpty ? "type to search" : "nothing matched \(model.query)",
                        icon: "text.magnifyingglass", tone: theme.color(.ink3))
                } else {
                    results
                }
                // Beside the results and outside the empty case, because the drawer belongs to
                // what you are reading rather than to what the last keystroke returned. Typing on
                // with something open is ordinary — you have found it and are now looking for the
                // next one — and a drawer that shut itself on the first query that matched
                // nothing would take the conversation away mid-sentence.
                if let open = model.reader.conv {
                    Rectangle().fill(theme.color(.rule)).frame(width: 1)
                    ReaderView(conv: open, reader: model.reader)
                        .frame(minWidth: 380, idealWidth: 460, maxWidth: 620)
                }
            }
        case .noIndex(let detail):
            placeholder(
                "no index yet", "run `cs index` — this is a first-run state, not a failure\n\(detail)",
                icon: "tray", tone: theme.color(.ink3))
        case .building(let detail):
            placeholder(
                "index is being built", "results arrive on their own when it finishes\n\(detail)",
                icon: "arrow.triangle.2.circlepath", tone: theme.color(.ink3))
        case .stale(let detail):
            // Distinct from `noIndex` because the sentence a user needs is different: something
            // is there and cannot be read, which is not the same as nothing being there.
            placeholder(
                "index cannot be read", "run `cs index` to rebuild it\n\(detail)",
                icon: "questionmark.folder", tone: theme.color(.ink3))
        case .noBinary(let detail):
            placeholder("cs not found", detail, icon: "exclamationmark.triangle", tone: theme.color(.err))
        case .failed(let detail):
            placeholder("cs failed", detail, icon: "exclamationmark.triangle", tone: theme.color(.err))
        }
    }

    /// `List` and not `ScrollView { LazyVStack }`: `chat-search-me9.22` measured the whole corpus
    /// through all three containers and this is the only one that recycles — 5.2 MB scrolling
    /// 3,200 rows against LazyVStack's 65.6 MB and plain VStack's 566 MB. The question is
    /// answered, so the app does not offer the others.
    private var results: some View {
        List(model.conversations, selection: opened) { conv in
            ResultRow(conv: conv)
                .listRowInsets(
                    EdgeInsets(
                        top: theme.metric(.rowPaddingTop),
                        leading: theme.metric(.rowPaddingX),
                        bottom: theme.metric(.rowPaddingBottom),
                        trailing: theme.metric(.rowPaddingX))
                )
                // The open row is `--sel-bg` and not the system's selection colour, which is
                // chosen somewhere this app cannot see.
                .listRowBackground(theme.color(conv.id == model.reader.conv?.id ? .selBg : .bg))
        }
        .listStyle(.plain)
        // The list is the page, so it takes the page's ground rather than the system's list
        // ground. `.plain` and not `.inset` for the same reason the insets above are set by hand:
        // the row's margin is `--row-px`, which a direction gets to move.
        .scrollContentBackground(.hidden)
        .background(theme.color(.bg))
        .frame(minWidth: 280)
    }

    /// The mockup's `.empty`: a dashed rule, the quiet tier, and no chrome. The icon was the
    /// shell's own and stays, sized off the scale — `.imageScale` rather than a number, because a
    /// glyph two and a bit times the head size is a number this app would then own.
    private func placeholder(_ title: String, _ detail: String, icon: String, tone: Color)
        -> some View
    {
        VStack(spacing: 8) {
            Image(systemName: icon)
                .font(theme.font(.head))
                .imageScale(.large)
                .foregroundStyle(tone)
            Text(title)
                .font(theme.font(.body, weight: .semibold))
                .foregroundStyle(theme.color(.ink2))
            Text(detail)
                .font(theme.font(.body))
                .foregroundStyle(theme.color(.ink3))
                .multilineTextAlignment(.center)
                .textSelection(.enabled)
        }
        .padding(22)
        .overlay(
            RoundedRectangle(cornerRadius: theme.metric(.r6))
                .strokeBorder(theme.color(.rule2), style: StrokeStyle(lineWidth: 1, dash: [3, 3])))
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
                .foregroundStyle(theme.color(.ink2))
            Spacer()
            // Kept because it costs nothing: `cs` measures its own query time and the client
            // already has a clock around the spawn, so the two numbers are decoded rather than
            // taken. A latency you cannot see while you type is a latency you will argue about.
            if let t = model.lastTiming {
                Text(String(format: "%.0f ms", t.total))
                Text(String(format: "%.1f in sqlite", t.serverMs))
            }
        }
        // Monospaced at `--fs-meta`, which is the mockup's rule for everything tabular: a number
        // that changes every keystroke has to stop the line reflowing while you read it.
        .font(theme.font(.meta, .mono))
        .foregroundStyle(theme.color(.ink3))
        .padding(.horizontal, 12)
        .padding(.vertical, 6)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(theme.color(.panel))
    }
}

/// One conversation, one line.
///
/// Deliberately plain. The row's anatomy — the agent badge, the kind ribbon, the marked snippet,
/// the date column — is `chat-search-me9.8.2`, argued against `poc/ui` and against the corpus
/// measurements in `poc/ui/NOTES.md`. A shell that guessed at it would have to be undone first.
///
/// What it does carry is the row's rhythm, which is a token and not a guess: the padding is set
/// by `SearchView` from `--row-pt` / `--row-pb` / `--row-px` and the gutter here is `--row-gap`.
/// Those are the tokens a direction is most likely to get wrong, since every one of them trades
/// against rows-per-screen.
struct ResultRow: View {
    let conv: Conversation
    @Environment(\.theme) private var theme

    var body: some View {
        HStack(spacing: theme.metric(.rowGap)) {
            Text(conv.title.flatMap { $0.isEmpty ? nil : $0 } ?? "untitled")
                .font(theme.font(.body, weight: .medium))
                .foregroundStyle(theme.color(.ink))
                .lineLimit(1)
                .truncationMode(.tail)
            Spacer(minLength: 12)
            Text(conv.source)
            if let date = conv.endedDate { Text(date) }
        }
        .font(theme.font(.meta, .mono))
        .foregroundStyle(theme.color(.ink3))
    }
}
