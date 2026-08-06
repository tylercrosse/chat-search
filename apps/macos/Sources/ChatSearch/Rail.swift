import CsKit
import CsTheme
import SwiftUI

/// The left rail: when a conversation happened, which tool it happened in, where it happened, and
/// what clicking any of those does to the query.
///
/// `poc/ui`'s sidebar is the layout reference — a section head, then one flush row per value with
/// its count pushed to the right margin. Its ordering is kept too, and that ordering is by
/// *coverage* rather than by how interesting a facet is: `ended_at` answers for every
/// conversation, a source for every conversation, a `cwd` for a quarter of them. What the mockup
/// has nothing to say about is every state it never had: it draws the populated rail and no
/// other, so the empty ones below are this app's.
///
/// **A click writes the query box and nothing else.** There is no filter state here to fall out
/// of step with what is typed, because the chip already arrived carrying the text it produces
/// (docs/TUI-DESIGN.md §5). That is the whole reason `cs facets` exists — and the three sections
/// differ in ways this file cannot see: a second source widens the `agent:` token, a second span
/// *replaces* the `date:` one, and one `dir:` fragment lights every directory beneath it. Every
/// one of those rules is in `cs_core::query` with the grammar (`chat-search-1ld`).
///
/// Nothing here names a colour, a size or a face; every one is a token off `\.theme`
/// (`chat-search-me9.8.8`). The rail is deliberately *uncoloured by source*: the five
/// `--src-*` hues are in the token layer and the rule that maps a source id onto one belongs
/// with the row's agent badge (`chat-search-me9.8.2`), so writing a second copy here is exactly
/// the duplication this epic's sequencing exists to prevent.
struct Rail: View {
    @Bindable var model: SearchModel
    @Environment(\.theme) private var theme

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 0) {
                if let rail = model.rail {
                    // A `cs` that predates a section leaves it nil, and a section this build has
                    // no rail for is one it does not draw — where an empty head would read as a
                    // corpus with no history in it.
                    if let dates = rail.dates {
                        head("When", dates.keyword + " · every conversation")
                        allRow(dates.all, "any time")
                        ForEach(dates.values) { chip in
                            spanRow(chip)
                        }
                    }

                    head("Sources", rail.sources.keyword + " · config ∪ index")
                    allRow(rail.sources.all, "all sources")
                    ForEach(rail.sources.values) { chip in
                        row(chip)
                    }

                    // Keep the section visible when the index has no directories. `indexed == 0`
                    // is a real first-run state, not the absence of a facet; hiding it makes a
                    // configured rail look like the client never received one.
                    if let dirs = rail.dirs {
                        head("Projects", dirs.keyword + " · " + drawn(dirs))
                        allRow(dirs.all, "anywhere")
                        ForEach(dirs.values) { chip in
                            dirRow(chip)
                        }
                        unreached(dirs)
                    }
                } else {
                    // The state the mockup does not model. Not an error and not empty: the rail
                    // is one process behind the first search, which on a cold start is a few
                    // milliseconds and on a broken `cs` is forever — and the results pane beside
                    // this is already saying which.
                    head("Sources", "…")
                }
            }
            .padding(.vertical, 4)
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .frame(width: 232)
        .background(theme.color(.panel))
    }

    /// `poc/ui`'s `.side-h`: the label, and on the right what the section is a facet of.
    private func head(_ label: String, _ meta: String) -> some View {
        HStack(spacing: 6) {
            Text(label.uppercased())
            Spacer(minLength: 8)
            Text(meta)
        }
        .font(theme.font(.micro, .mono))
        .tracking(1.4)
        .foregroundStyle(theme.color(.ink3))
        .padding(.horizontal, 12)
        .padding(.top, 13)
        .padding(.bottom, 4)
    }

    /// The All row. Selected means the query names nothing on that facet at all, which is the
    /// only state in which every value is in the answer — an exclusion still standing is not
    /// "all", and neither is a `date:` or `dir:` token this rail has no chip for.
    private func allRow(_ all: AllChip, _ label: String) -> some View {
        Button { model.apply(chip: all.query) } label: {
            HStack(spacing: 7) {
                glyph("·")
                Text(label)
                Spacer(minLength: 8)
            }
            .modifier(RailRow(on: all.selected, dimmed: false))
        }
        .buttonStyle(.plain)
        .disabled(all.selected)
    }

    /// One span. The label rather than the value, because `>1mo` is the grammar's spelling and
    /// not a thing to click — and it arrives beside it rather than being written here, so the
    /// two clients say the same words.
    private func spanRow(_ chip: DateChip) -> some View {
        Button { model.apply(chip: chip.query) } label: {
            HStack(spacing: 7) {
                glyph(mark(chip.state))
                Text(chip.label)
                Spacer(minLength: 8)
                count(chip.conversations)
            }
            .modifier(RailRow(on: chip.state == .include, dimmed: chip.conversations == 0))
        }
        .buttonStyle(.plain)
        .help("conversations that ended in this span — the spans nest, so these do not sum")
    }

    /// One directory, shortened by the same rule the row beside it uses: `Display.dir` collapses
    /// `$HOME`, and the elision is in the middle because tail-elision discards the leaf, which is
    /// the discriminating token (docs/TUI-DESIGN.md §2, and `ResultRow.meta`). 232pt does not hold
    /// `~/dev/projects/chat-search`, and the whole of it is in the tooltip and in the query box
    /// the moment it is clicked, so nothing drawn here is the only copy of it.
    private func dirRow(_ chip: DirChip) -> some View {
        Button { model.apply(chip: chip.query) } label: {
            HStack(spacing: 7) {
                glyph(mark(chip.state))
                Text(Display.dir(chip.value))
                    .lineLimit(1)
                    .truncationMode(.middle)
                Spacer(minLength: 8)
                count(chip.conversations)
            }
            .modifier(RailRow(on: chip.state == .include, dimmed: chip.state == .exclude))
        }
        .buttonStyle(.plain)
        .help(chip.value)
    }

    /// What the section is showing, when it is not showing all of it. `poc/ui`'s `dir:` head says
    /// how many are indexed; ours says how many of them are drawn, because the rail is the busiest
    /// twelve and the tail is per-conversation scratch directories (`chat-search-6eb.26`).
    private func drawn(_ dirs: DirFacet) -> String {
        dirs.values.count < dirs.indexed
            ? "\(dirs.values.count) of \(dirs.indexed)"
            : "\(dirs.indexed) indexed"
    }

    /// The line that keeps this section from reading as a filter over everything.
    ///
    /// Three quarters of this corpus records no working directory, because only the agent sources
    /// have one — so `dir:` cannot reach a ChatGPT conversation at all, and a rail that showed
    /// only the directories would look like a complete account of where the work happened
    /// (`chat-search-6eb.26`). Not a button: there is no query that means "the ones with none".
    private func unreached(_ dirs: DirFacet) -> some View {
        HStack(spacing: 7) {
            glyph("·")
            Text("\(dirs.undirected.formatted()) record none")
            Spacer(minLength: 8)
        }
        .font(theme.font(.meta))
        .foregroundStyle(theme.color(.ink3))
        .padding(.horizontal, 12)
        .padding(.vertical, 4)
        .help("only the agent sources record a working directory, so `dir:` cannot reach these")
    }

    /// The count at the right margin, in the one place all three sections agree about it.
    private func count(_ n: Int) -> some View {
        Text(n.formatted())
            .font(theme.font(.meta, .mono))
            .foregroundStyle(theme.color(.ink3))
            .monospacedDigit()
    }

    private func row(_ chip: SourceChip) -> some View {
        // A source whose directory is here and which no `[[sources]]` entry claims has nothing to
        // filter to — its conversations are accruing uncaptured right now, which is what the row
        // is here to say. Clicking it would return an empty list, so it does not offer to.
        let unwatched = chip.coverage == .unconfigured
        return Button { model.apply(chip: chip.query) } label: {
            HStack(spacing: 7) {
                glyph(mark(chip.state))
                Text(chip.value)
                    .lineLimit(1)
                    .truncationMode(.middle)
                // A configured source holding nothing is a broken importer or an archive run
                // that never happened, and it is the one state a bar built from the index alone
                // cannot draw at all: you search, get nothing, and conclude you used a different
                // tool (`chat-search-a7k.29`).
                if chip.coverage == .live, chip.conversations == 0 {
                    Text("!")
                        .font(theme.font(.meta, .mono, weight: .bold))
                        .foregroundStyle(theme.color(.hit))
                }
                Spacer(minLength: 8)
                if unwatched {
                    Text("—")
                        .font(theme.font(.meta, .mono))
                        .foregroundStyle(theme.color(.ink3))
                } else {
                    count(chip.conversations)
                }
            }
            .modifier(
                RailRow(
                    on: chip.state == .include,
                    dimmed: unwatched || chip.state == .exclude || chip.conversations == 0))
        }
        .buttonStyle(.plain)
        .disabled(unwatched)
        .help(explain(chip))
    }

    /// The state marker, in the gutter `poc/ui` keeps for one. An excluded value is not drawn
    /// like an untouched one: filtering that is invisible where it applies is the defect the
    /// three-state chip exists to remove. One function for all three rails, because the three
    /// states mean the same thing on each.
    private func mark(_ state: ChipState) -> String {
        switch state {
        case .include: "▸"
        case .exclude: "✕"
        case .off, .unrecognised: "·"
        }
    }

    private func glyph(_ text: String) -> some View {
        Text(text)
            .font(theme.font(.micro))
            .foregroundStyle(theme.color(.ink3))
            .frame(width: 11)
    }

    /// The sentence behind the row, which is where coverage stops being a shade of grey.
    private func explain(_ chip: SourceChip) -> String {
        switch chip.coverage {
        case .live where chip.conversations == 0:
            "configured, and the index holds nothing for it — the importer or the archive run"
        case .live: "configured, and its directory is on this machine"
        case .missing: "configured, but its directory is gone — nothing new is arriving"
        case .unconfigured:
            "on this machine and watched by nothing — its conversations are not being captured"
        case .retired: "no longer configured; what was captured is still searchable"
        case .unrecognised(let name): "a coverage state this build has no reading for: \(name)"
        }
    }
}

/// One rail row's rhythm and its three grounds, so the All row and a source row cannot drift.
///
/// `poc/ui`'s `.srow`: `--ink-2` at `--fs-body`, `--sel` on `--sel-bg` when pressed, and 42%
/// opacity for a source there is nothing to click through to.
private struct RailRow: ViewModifier {
    @Environment(\.theme) private var theme
    let on: Bool
    let dimmed: Bool

    func body(content: Content) -> some View {
        content
            .font(theme.font(.body, weight: on ? .semibold : .regular))
            .foregroundStyle(theme.color(on ? .sel : .ink2))
            .padding(.horizontal, 12)
            .padding(.vertical, 4)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(on ? theme.color(.selBg) : .clear)
            .opacity(dimmed && !on ? 0.55 : 1)
            .contentShape(Rectangle())
    }
}
