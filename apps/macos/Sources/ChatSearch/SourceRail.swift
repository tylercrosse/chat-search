import CsKit
import CsTheme
import SwiftUI

/// The left rail: which sources exist, how much of each one is indexed, and what clicking one
/// does to the query.
///
/// `poc/ui`'s sidebar is the layout reference — a section head, then one flush row per value with
/// its count pushed to the right margin. What the mockup has nothing to say about is every state
/// it never had: it draws the populated rail and no other, so the empty one below is this app's.
///
/// **A click writes the query box and nothing else.** There is no filter state here to fall out
/// of step with what is typed, because the chip already arrived carrying the text it produces
/// (docs/TUI-DESIGN.md §5). That is the whole reason `cs facets` exists.
///
/// Nothing here names a colour, a size or a face; every one is a token off `\.theme`
/// (`chat-search-me9.8.8`). The rail is deliberately *uncoloured by source*: the five
/// `--src-*` hues are in the token layer and the rule that maps a source id onto one belongs
/// with the row's agent badge (`chat-search-me9.8.2`), so writing a second copy here is exactly
/// the duplication this epic's sequencing exists to prevent.
struct SourceRail: View {
    @Bindable var model: SearchModel
    @Environment(\.theme) private var theme

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 0) {
                if let sources = model.rail?.sources {
                    head("Sources", sources.keyword + " · config ∪ index")
                    allRow(sources.all)
                    ForEach(sources.values) { chip in
                        row(chip)
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

    /// The All row. Selected means the query names no source at all, which is the only state in
    /// which every source is in the answer — an exclusion still standing is not "all".
    private func allRow(_ all: AllChip) -> some View {
        Button { model.apply(chip: all.query) } label: {
            HStack(spacing: 7) {
                glyph("·")
                Text("all sources")
                Spacer(minLength: 8)
            }
            .modifier(RailRow(on: all.selected, dimmed: false))
        }
        .buttonStyle(.plain)
        .disabled(all.selected)
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
                Text(unwatched ? "—" : chip.conversations.formatted())
                    .font(theme.font(.meta, .mono))
                    .foregroundStyle(theme.color(.ink3))
                    .monospacedDigit()
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

    /// The state marker, in the gutter `poc/ui` keeps for one. An excluded source is not drawn
    /// like an untouched one: filtering that is invisible where it applies is the defect the
    /// three-state chip exists to remove.
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
