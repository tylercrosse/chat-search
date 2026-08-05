import CsKit
import CsTheme
import SwiftUI

/// The head of one group: its name, how many rows are under it, when they happened, and the shape
/// of that in time.
///
/// `poc/ui`'s `.pj-head` is the layout — name, tags, count, span, sparkline — with two of its
/// cells deliberately absent here:
///
/// - **No twisty, because these do not fold.** The prototype folds every axis because it groups a
///   corpus-scale export, where thirteen open headers are a project index and one open group
///   buries the other twelve. This groups the `--limit` window of a ranked answer, so folding
///   would hide the answer behind its own headers — and the prototype's own note says the fold
///   left it leaning on a keyboard affordance it never wired (`poc/ui/NOTES.md` §2, §5). Filed as
///   the row window grows: `chat-search-me9.8.15`.
/// - **No source badges.** The prototype puts a strip of tiny source icons on every header. There
///   is no asset catalog in this package and SF Symbols has no glyph for a vendor
///   (`chat-search-me9.8.2`), so the honest version here would be colour alone — which
///   `poc/ui/NOTES.md` §7 rules out. The source is in every row underneath.
///
/// Nothing here names a colour, a size or a face; every one is a token off `\.theme`
/// (`chat-search-me9.8.8`). The source hue is read from `Display.sourceHue`, the row's own
/// mapping, rather than restated (`chat-search-g6u`).
struct GroupHeader: View {
    let group: ConversationGroup
    let axis: Grouping
    @Environment(\.theme) private var theme

    var body: some View {
        HStack(spacing: theme.metric(.rowGap)) {
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
            Text(verbatim: "\(group.items.count)")
                .foregroundStyle(theme.color(.ink))
                .monospacedDigit()
                .layoutPriority(3)

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
        .background(theme.color(.panel))
        .overlay(alignment: .bottom) {
            Rectangle().fill(theme.color(.rule)).frame(height: 1)
        }
    }

    /// The residue group is named by the axis, because "what these rows are missing" is a fact
    /// about the axis rather than about the rows: `no working directory` and `no last message` are
    /// two different absences and only the axis knows which one this is.
    private var name: String { group.isResidue ? (axis.residue?.label ?? "ungrouped") : group.label }

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
