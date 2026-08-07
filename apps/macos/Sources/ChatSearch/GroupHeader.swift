import CsKit
import CsTheme
import SwiftUI

/// The head of one group: its name, how many rows are under it and which number that is, when they
/// happened, and the shape of that in time.
///
/// `poc/ui`'s `.pj-head` is the layout — twisty, name, tags, count, span, sparkline — with one of
/// its cells deliberately absent here:
///
/// - **No source badges.** The prototype puts a strip of tiny source icons on every header. There
///   is no asset catalog in this package and SF Symbols has no glyph for a vendor
///   (`chat-search-me9.8.2`), so the honest version here would be colour alone — which
///   `poc/ui/NOTES.md` §7 rules out. The source is in every row underneath.
///
/// The head is a control as well as a label: clicking it folds the group, and so does Enter with
/// the cursor on it (`chat-search-me9.8.15`). What the fold does *not* change is anything the head
/// says — the count, the span and the sparkline are drawn folded exactly as they are open, because
/// a folded group and a group that is not there have to look different.
///
/// Nothing here names a colour, a size or a face; every one is a token off `\.theme`
/// (`chat-search-me9.8.8`). The source hue is read from `Display.sourceHue`, the row's own
/// mapping, rather than restated (`chat-search-g6u`).
struct GroupHeader: View {
    let group: ConversationGroup
    let axis: Grouping
    /// Whether the rows under this head are drawn. The head is drawn either way.
    let folded: Bool
    /// Whether the keyboard cursor is standing on this head, which is a place it could not be
    /// before there was a fold to reach.
    let cursored: Bool
    @Environment(\.theme) private var theme

    var body: some View {
        HStack(spacing: theme.metric(.rowGap)) {
            // `poc/ui`'s `.tw`: one triangle that rotates rather than two glyphs that swap, so the
            // mark stays continuous through the state change. Its cell is a fixed width — the
            // prototype spends a 14px grid track on the same thing — because the only thing this
            // cell has to guarantee is that the name starts at the same x folded and open.
            Image(systemName: "chevron.right")
                .font(theme.font(.micro))
                .foregroundStyle(theme.color(.ink3))
                .rotationEffect(.degrees(folded ? 0 : 90))
                .frame(width: 9)

            Text(name)
                .font(theme.font(.body, .mono, weight: .semibold))
                .foregroundStyle(theme.color(group.hue ?? (group.isResidue ? .ink2 : .ink)))
                .lineLimit(1)
                // Middle, for the same reason the row's directory elides that way: the leaf is
                // the discriminating token, and a project axis is mostly paths.
                .truncationMode(.middle)
                // Before the gap to its right, which otherwise takes the slack and elides a name
                // there was room for — the same trade the row's directory cell makes.
                .layoutPriority(2)
                .help(group.isResidue ? (axis.residue?.why ?? "") : group.label)

            // No `residue` tag beside it, unlike the prototype's: `no working directory` already
            // says what the tag would, and a label that describes itself is the channel to spend
            // 60pt on in a column this narrow. `cross-tool` stays, because it says something the
            // label — a date — cannot.
            if group.crossTool { tag("cross-tool") }

            Spacer(minLength: 8)

            // Priority descending to the left, which is the order these may be given up in. The
            // count is the one cell that must never be clipped — a group header reading `3` where
            // it means `39` is a wrong number rather than an elided one — and the reader pane
            // beside this list leaves the column ~400pt at the window's floor, so something has
            // to give. The name gives first, and it elides in the middle where a path survives it.
            // The word beside the number is part of the number and is clipped with it, for the
            // same reason: `39` where it means `39 here` is also a wrong number.
            count
                .monospacedDigit()
                // The pair has a space in it now, which is a place a `Text` will wrap before it
                // will truncate — and a cell that answered a narrow column by becoming two lines
                // would move the height of every head on the screen. `.fixedSize()` is what every
                // other must-not-degrade cell in this file already carries.
                .fixedSize()
                .layoutPriority(3)
                .help(countHelp)

            // Whole or not at all. An elided date range says less than no date range does —
            // `Oct 8 ’2…` is a worse cell than an empty one — so this is the piece that leaves
            // when the column cannot hold everything, rather than the piece that degrades.
            ViewThatFits(in: .horizontal) {
                Text(right)
                    .foregroundStyle(theme.color(.ink3))
                    .fixedSize()
                Color.clear.frame(width: 0, height: 0)
            }
            .layoutPriority(1)

            Sparkline(stamps: group.items.compactMap(\.endedAt))
        }
        .font(theme.font(.meta, .mono))
        .padding(.horizontal, theme.metric(.rowPaddingX))
        .padding(.vertical, 6)
        .frame(maxWidth: .infinity, alignment: .leading)
        // A section header in a plain `List` scrolls over the rows above it, so it needs a ground
        // of its own — `--panel`, the same one the rail and the footer stand on. The prototype's
        // header is transparent because a web list does not float it.
        //
        // Under the cursor it takes `--sel-bg`, the same mark a row under the cursor takes: the
        // cursor is one cursor whether it is standing on a conversation or on the head of a group,
        // and drawing it two ways would say it was two things.
        .background(theme.color(cursored ? .selBg : .panel))
        .contentShape(Rectangle())
        .overlay(alignment: .bottom) {
            Rectangle().fill(theme.color(.rule)).frame(height: 1)
        }
    }

    /// The residue group is named by the axis, because "what these rows are missing" is a fact
    /// about the axis rather than about the rows: `no working directory` and `no last message` are
    /// two different absences and only the axis knows which one this is.
    private var name: String { group.isResidue ? (axis.residue?.label ?? "ungrouped") : group.label }

    /// The count, and — in one word — which number it is (`chat-search-me9.8.23`).
    ///
    /// `39` alone is the defect: it is the rows this query returned that landed in this group,
    /// which is the `--limit` window and not the corpus, and a head reading `39` cannot say
    /// whether the project has 39 conversations or 400. So the number never appears without the
    /// set it counted. `39 here` is that set named; `56 of 458` is the corpus's number beside it,
    /// on the one axis a census reaches (see `Census`), and it is the prototype's `114 · 30 here`
    /// written the way an English sentence reads.
    ///
    /// One `Text` and not two views, so the pair is laid out and clipped as a unit — half of a
    /// two-part number is a wrong number. The qualifier is `--ink3`: it changes how to read the
    /// count without competing with it, which is the same weight the tag beside the name carries.
    private var count: Text {
        let here = Text(verbatim: group.items.count.formatted())
            .foregroundStyle(theme.color(.ink))
        // `formatted()` and not interpolation, because the corpus's number is in the thousands and
        // the rail on the left of this same window draws `2,011` for the source this head is
        // counting. Two spellings of one number on one screen is the smaller version of the thing
        // this whole head is about.
        let qualifier = group.corpus.map { " of \($0.formatted())" } ?? " here"
        return here + Text(verbatim: qualifier).foregroundStyle(theme.color(.ink3))
    }

    /// The whole sentence, on hover. The cell has room for one word and this is the paragraph that
    /// word stands for — including, on a corpus-true head, that the number on the right is
    /// query-blind: the index holds 458 claude-code conversations whatever was typed in the box.
    private var countHelp: String {
        let here = group.items.count.formatted()
        guard let corpus = group.corpus else {
            return "\(here) of the rows this query returned — the --limit window of the answer, "
                + "not the corpus"
        }
        return "\(here) of the rows this query returned, out of \(corpus.formatted()) this index "
            + "holds in all"
    }

    /// The right-hand cell. A run's label is already its day span, so repeating it there says
    /// nothing; how long the run lasted does, and it is a duration rather than a clock (see
    /// `Display.elapsed`).
    private var right: String {
        if axis == .run, !group.isResidue { return group.elapsed ?? "" }
        return group.days
    }

    /// `poc/ui`'s `.tag`: uppercase micro mono in a hairline box, for what changes how to read the
    /// count beside it.
    private func tag(_ text: String) -> some View {
        Text(text.uppercased())
            .font(theme.font(.micro, .mono))
            .tracking(0.6)
            .foregroundStyle(theme.color(.ink3))
            .padding(.horizontal, 5)
            .padding(.vertical, 1.5)
            .overlay(
                RoundedRectangle(cornerRadius: theme.metric(.r1))
                    .strokeBorder(theme.color(.rule2)))
            .fixedSize()
    }
}

/// Twelve bars of when this group happened, so a dormant project and a live one are
/// distinguishable before reading a single row.
///
/// `poc/ui`'s `.spark`, including both of its measured decisions: 12 bars at 3px rather than 20 at
/// 2px, because the finer version reads as dust at the only zoom anyone uses; and nothing at all
/// below three timestamps, where there is no shape to show — only empty buckets and a spike, which
/// reads as a broken rule.
private struct Sparkline: View {
    let stamps: [Int]
    @Environment(\.theme) private var theme

    private static let bars = 12
    private static let height: CGFloat = 16

    var body: some View {
        // Bucketed once and the peak taken once, rather than per bar: this is a header in a list
        // that is rebuilt on every keystroke, and the arithmetic below is cheap only while it
        // happens twelve times fewer than the obvious way of writing it.
        let buckets = self.buckets
        let peak = CGFloat(max(1, buckets.max() ?? 1))
        return HStack(alignment: .bottom, spacing: 1) {
            ForEach(Array(buckets.enumerated()), id: \.offset) { _, count in
                RoundedRectangle(cornerRadius: 0.5)
                    .fill(theme.color(count == 0 ? .ink3 : .kUser).opacity(count == 0 ? 0.25 : 0.8))
                    .frame(
                        width: 3,
                        height: count == 0
                            ? 1.5 : max(3, (CGFloat(count) / peak * Self.height).rounded()))
            }
        }
        // Fixed whether or not there are bars, so a group with two conversations in it does not
        // sit at a different row height from the one above it.
        .frame(width: CGFloat(Self.bars) * 4 - 1, height: Self.height, alignment: .bottomLeading)
    }

    /// Counts per bucket, oldest at the left. Empty below three timestamps.
    private var buckets: [Int] {
        guard stamps.count >= 3, let low = stamps.min(), let high = stamps.max() else { return [] }
        var buckets = [Int](repeating: 0, count: Self.bars)
        for stamp in stamps {
            let position = low == high
                ? Self.bars - 1
                : Int((Double(stamp - low) / Double(high - low) * Double(Self.bars - 1)).rounded())
            buckets[min(max(position, 0), Self.bars - 1)] += 1
        }
        return buckets
    }
}
