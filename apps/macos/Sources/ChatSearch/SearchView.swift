import CsKit
import CsTheme
import SwiftUI

/// A search field, a grouping control, a facet rail, a result list, a reader, and a way back into
/// a conversation.
///
/// The row's anatomy is `ResultRow`, the drawer is `ReaderView`, the group head is `GroupHeader`;
/// what lives here is the seam between them — selection, opening, and where the reader sits beside
/// the results.
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
            searchBar
            rule
            indexBanner
            unappliedBanner
            openBanner
            HStack(spacing: 0) {
                Rail(model: model)
                Rectangle().fill(theme.color(.rule)).frame(width: 1)
                content
            }
        }
        .onAppear { focused = true }
        // The fallback half of the arrangement below: this fires only when the focused view did
        // not handle Return, which is exactly the case where a click has moved focus into the
        // list. When the field has it — the ordinary state — `onSubmit` gets there first and
        // consumes it, so the two are mutually exclusive rather than both firing.
        .onKeyPress(.return) { model.openSelected(); return .handled }
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
                model.selected = id
                model.reader.open(
                    model.conversations.first { $0.id == id }, query: model.query)
            })
    }

    /// The group control leads the query it modifies, which is `poc/ui`'s arrangement and its
    /// reasoning: it reads in the order it applies — group by project, *then* narrow with the
    /// query — where on the right it sat between the query and the count and read as status.
    private var searchBar: some View {
        HStack(spacing: 12) {
            groupControl
            field
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 7)
    }

    /// The axis, as five chips of which four are offered.
    ///
    /// **`topic` is drawn and cannot be clicked**, which is the prototype's rule for a narrowing
    /// with no grammar behind it — dashed, not hidden and not faked. Hiding it would say this
    /// corpus has four axes; drawing it live would need a taxonomy that is a `poc/ui` export
    /// rather than an index fact. A dashed chip with the reason on it says the true thing, which
    /// is that the axis exists and this wire cannot carry it.
    private var groupControl: some View {
        HStack(spacing: 2) {
            Text("GROUP")
                .font(theme.font(.micro, .mono))
                .tracking(1.4)
                .foregroundStyle(theme.color(.ink3))
                .padding(.trailing, 3)
            ForEach(Grouping.allCases) { axis in
                Button {
                    model.group(by: axis)
                    // A click anywhere but the field is a click that took focus off it, and the
                    // arrangement this app is built on is one box that never gives it up.
                    focused = true
                } label: {
                    chip(axis.label, on: model.grouping == axis, dashed: false)
                }
                .buttonStyle(.plain)
                .help(axis.note)
            }
            chip("topic", on: false, dashed: true)
                .opacity(0.55)
                .help(
                    "a seeded taxonomy over the corpus — 26 seeds, and 37% of conversations match "
                        + "none of them. Derived by poc/ui/export.py, not an index fact, and not "
                        + "on this wire (chat-search-me9.18)")
        }
        .fixedSize()
    }

    /// `poc/ui`'s `.group button`: monospaced meta in a hairline box, `--sel` on `--sel-bg` when it
    /// is the axis in force.
    private func chip(_ text: String, on: Bool, dashed: Bool) -> some View {
        Text(text)
            .font(theme.font(.meta, .mono, weight: on ? .semibold : .regular))
            .foregroundStyle(theme.color(on ? .sel : .ink3))
            .padding(.horizontal, 8)
            .padding(.vertical, 3)
            .background(
                RoundedRectangle(cornerRadius: theme.metric(.r3))
                    .fill(on ? theme.color(.selBg) : .clear))
            .overlay(
                RoundedRectangle(cornerRadius: theme.metric(.r3))
                    .strokeBorder(
                        theme.color(on ? .sel : .rule2),
                        style: StrokeStyle(lineWidth: 1, dash: dashed ? [2.5, 2.5] : []))
                    .opacity(on || dashed ? 1 : 0))
            .contentShape(Rectangle())
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
                // The TUI's arrangement: one input box that never gives up focus, arrows moving
                // the cursor in the list beside it, and Enter opening what the cursor is on. The
                // bindings are on the field rather than on the list for that reason — the field
                // is the focused view, and a key reaches it first.
                //
                // Return goes through `onSubmit` and not `onKeyPress`, because a text field
                // consumes Return itself and `onSubmit` is the seam it consumes it into. The
                // arrows have no such seam and no default this app wants, so they are read
                // directly.
                .onSubmit { model.openSelected() }
                .onKeyPress(.upArrow) { model.moveSelection(by: -1); return .handled }
                .onKeyPress(.downArrow) { model.moveSelection(by: 1); return .handled }
        }
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

    /// A filter token whose value selects nothing.
    ///
    /// **Exit status 0 and not an error**, which is exactly why it needs drawing: the results
    /// below are wider than the query asked for, and a client that ignores this shows unfiltered
    /// results for a filtered query and it looks like it worked (`chat-search-6eb.11`). The
    /// tokens are quoted as typed, because `agent:` and `date:nope` fail for different reasons
    /// and the one that says which is the text.
    @ViewBuilder
    private var unappliedBanner: some View {
        if !model.unappliedFilters.isEmpty {
            banner(
                icon: "line.3.horizontal.decrease.circle",
                text: "\(model.unappliedFilters.joined(separator: ", ")) — not a value this can "
                    + "select on, so it is not filtering",
                askAgain: false)
        }
    }

    /// What the last attempt to open a conversation said. Only ever non-empty after an Enter, and
    /// cleared by the next keystroke: a sentence about a row is stale the moment the list is not
    /// that list.
    ///
    /// Drawn in the error tone, unlike the two above, because every message that reaches here is
    /// something that did not happen — including "this source has no way to reopen", which is a
    /// fact about the source rather than a fault but is still an Enter that opened nothing.
    @ViewBuilder
    private var openBanner: some View {
        if let failure = model.openFailure {
            banner(icon: "exclamationmark.triangle", text: failure, askAgain: false, tone: .err)
        }
    }

    private func banner(
        icon: String, text: String, askAgain: Bool, tone: ColorToken = .ink2
    ) -> some View {
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
            .foregroundStyle(theme.color(tone))
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

    /// One list, cut along whichever axis is in force. **Not two lists and not two views**: the
    /// rows, the row component, the selection and the drawer are the same in both arrangements,
    /// which is the whole of `chat-search-4ar.10` — three of the prototype's views were `GROUP BY`
    /// over one set, so what was a place became a setting.
    ///
    /// `List` and not `ScrollView { LazyVStack }`: `chat-search-me9.22` measured the whole corpus
    /// through all three containers and this is the only one that recycles — 5.2 MB scrolling
    /// 3,200 rows against LazyVStack's 65.6 MB and plain VStack's 566 MB. The question is
    /// answered, so the app does not offer the others. Sections keep that: a grouped list is one
    /// `List` with headers in it, not a stack of lists.
    private var results: some View {
        VStack(spacing: 0) {
            if model.grouping != .none { groupKey }
            List(selection: opened) {
                if model.grouping == .none {
                    ForEach(model.conversations) { row($0) }
                } else {
                    ForEach(model.groups) { group in
                        Section {
                            ForEach(group.items) { row($0) }
                        } header: {
                            GroupHeader(group: group, axis: model.grouping)
                                .listRowInsets(EdgeInsets())
                                .listRowSeparator(.hidden)
                        }
                    }
                }
            }
            .listStyle(.plain)
            // The list is the page, so it takes the page's ground rather than the system's list
            // ground. `.plain` and not `.inset` for the same reason the insets below are set by
            // hand: the row's margin is `--row-px`, which a direction gets to move.
            .scrollContentBackground(.hidden)
            .background(theme.color(.bg))
        }
        .frame(minWidth: 280)
    }

    /// One row, drawn identically grouped and ungrouped.
    private func row(_ conv: Conversation) -> some View {
        // `marks` travels with the rows it describes: the offsets in a snippet are only readable
        // in the units the envelope that carried them named.
        ResultRow(conv: conv, marks: model.marks)
            .listRowInsets(
                EdgeInsets(
                    top: theme.metric(.rowPaddingTop),
                    leading: theme.metric(.rowPaddingX),
                    bottom: theme.metric(.rowPaddingBottom),
                    trailing: theme.metric(.rowPaddingX))
            )
            // The keyboard cursor is `--sel-bg` and not the system's selection colour, which is
            // chosen somewhere this app cannot see. A click synchronises this id through `opened`
            // before opening the reader.
            .listRowBackground(theme.color(model.selected == conv.id ? .selBg : .bg))
            // Double-click and Enter reopen externally; a single click opens the reader.
            .onTapGesture(count: 2) { model.open(conv) }
            .contextMenu { openMenu(conv) }
    }

    /// What the axis did to this answer, above the list it did it to.
    ///
    /// The prototype's `.ribbon-key`, and it exists for the residue: a count of groups says
    /// nothing about the rows that landed in none of them, and on `project` that is two thirds of
    /// the corpus. It also states the set being grouped — the rows in hand, which is the `--limit`
    /// window of the answer and not the corpus — because a group header showing `12` cannot say
    /// which of the two it counted.
    private var groupKey: some View {
        // Three facts, in the order they may be given up in. The reader pane leaves this column
        // ~400pt at the window's floor, which is about seventy characters of the micro face, so
        // the last of them elides — and every one of them carries the whole sentence on hover.
        HStack(spacing: 12) {
            ViewThatFits(in: .horizontal) {
                Text("\(model.groups.count) groups · \(model.conversations.count) rows in hand")
                    .fixedSize()
                Text("\(model.groups.count) groups · \(model.conversations.count) rows")
                    .fixedSize()
            }
            .help(
                "grouped over the rows this query returned, which is the --limit window of the "
                    + "answer and not the corpus")
            .layoutPriority(1)
            if let residue = model.groups.first(where: \.isResidue),
                let named = model.grouping.residue
            {
                // The count inline and the reason on hover: `codex and claude-code carry one,
                // chatgpt and gemini-cli never do` is what makes the number mean something, and
                // it is three times the length of the line it would have to sit in.
                Text("\(residue.items.count) with \(named.label)")
                    .foregroundStyle(theme.color(.hit))
                    .help(named.why)
                    .layoutPriority(2)
            }
            // Whole or not at all, as in the group head: half a measurement is not a measurement,
            // and the chip that turned this axis on carries the same sentence on hover.
            ViewThatFits(in: .horizontal) {
                Text(model.grouping.note)
                    .foregroundStyle(theme.color(.ink3).opacity(0.7))
                    .fixedSize()
                Color.clear.frame(width: 0, height: 0)
            }
            Spacer(minLength: 0)
        }
        .font(theme.font(.micro, .mono))
        .tracking(0.6)
        .foregroundStyle(theme.color(.ink3))
        .lineLimit(1)
        .padding(.horizontal, 12)
        .padding(.vertical, 5)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(theme.color(.bg))
        .overlay(alignment: .bottom) {
            Rectangle().fill(theme.color(.rule)).frame(height: 1)
        }
    }

    /// Where this conversation can be reopened, best first — and, when that list is empty, the
    /// sentence saying so rather than a menu with nothing in it.
    ///
    /// A menu and not a modal: `cs_core::destinations` returns at most one entry for every source
    /// today, so a picker would be a dialog nobody is ever asked. docs/TUI-DESIGN.md §6's rule is
    /// to skip the modal when there is one answer; this shows the answer, and has room for the
    /// second the day a source offers one.
    @ViewBuilder
    private func openMenu(_ conv: Conversation) -> some View {
        if conv.destinations.isEmpty {
            Text("\(conv.source) conversations cannot be reopened from here")
        } else {
            ForEach(Array(conv.destinations.enumerated()), id: \.offset) { _, destination in
                Button(label(for: destination)) { model.open(conv, at: destination) }
            }
        }
    }

    /// Named after what the reader would be taken to, not after the variant.
    private func label(for destination: Destination) -> String {
        switch destination {
        case .terminal(let argv): "Resume in a terminal — \(argv.first ?? "")"
        case .web: "Open in the browser"
        case .unsupported(let kind): "\(kind): this build cannot open it"
        }
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
}
