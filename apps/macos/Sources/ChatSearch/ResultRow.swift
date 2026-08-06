import AppKit
import CsKit
import CsTheme
import SwiftUI

/// One conversation: three lines with a query and two without.
///
/// `poc/ui/index.html` is the structure — metadata on line 1, the title full width on line 2, the
/// best match on line 3 and only when there is one — and `poc/ui/NOTES.md` §2 is why. Its pixel
/// values are not the reference (`chat-search-me9.8`), and this is where that distinction gets
/// spent: the mockup was drawn against a fixed 706px list column and a native window is resizable,
/// so the two trades that column forced were re-measured rather than inherited.
///
/// **The agent badge is the word, not an icon.** The mockup dropped the word to buy `cwd` back the
/// 66px it needed at 706. This row does not spend that: with no ribbon it costs about 130pt less
/// than the mockup's, so at the window's 720pt floor there is ~360pt left for the directory and
/// the gutter between the clusters, against a mockup that could afford the path 17 characters.
/// Even once the ribbon lands the directory keeps ~24 of its 44. The word is also the
/// only channel left, since a package with no asset catalog has no
/// per-source icon and SF Symbols has no glyph for a vendor; `poc/ui/NOTES.md` §7 asks for three
/// redundant channels and this row has two, colour and text, rather than the mockup's shape and
/// colour. Text is the one that survives greyscale and a colourblind reader.
///
/// **There is no ribbon, and nothing stands in its place.** It needs `kind_runs` drawn in the same
/// coordinate space as `match_seqs`, and `chat-search-me9.25` says those are two spaces today;
/// `chat-search-me9.31` is the bead. A ribbon in the wrong space is worse than none, and a
/// stand-in mark would have to be undone when the real one arrives.
///
/// **Two of the mockup's cells are still not on the wire** — total edit lines and topics, both
/// derived by `poc/ui/export.py` from data the contract has no field for. They are absent rather
/// than blank: a cell with no data anywhere behind it is not a column this row gets to reserve
/// space for. Model and forks were in that list until `chat-search-me9.8.14` put them on the wire,
/// and each is now drawn per row on the same principle — present when the row has something to
/// say, gone when it does not, so neither spends width on 1,300 and 4,381 rows respectively.
struct ResultRow: View {
    let conv: Conversation
    /// How to read `snippetSpans`, off the envelope that carried them rather than assumed. The
    /// client that guessed here mis-highlighted every snippet with an em-dash in it
    /// (`chat-search-me9.33`), and a wrong offset marks the wrong word rather than failing.
    let marks: MarkOffsets
    @Environment(\.theme) private var theme

    var body: some View {
        VStack(alignment: .leading, spacing: theme.metric(.rowLead)) {
            meta
            title
            // Line 3 is spent only when there is something to show. `matches` is empty for an
            // empty query — the contract says so — which is what makes "three lines with a query,
            // two without" a property of the data rather than a flag this view has to be handed.
            if let best = conv.matches.first { bestMatch(best) }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        // The mockup's `border-bottom: 1px solid var(--rule)`. Untinted, a `List` separator is the
        // system's line — the same colour `SearchView` refuses `Divider()` over.
        .listRowSeparatorTint(theme.color(.rule))
    }

    // MARK: - The three lines

    /// Line 1: two clusters split by a gutter — who and how big on the left, where and when on the
    /// right. `cwd` sits with the date because both answer *which of my worlds was this* rather
    /// than *what is it*.
    ///
    /// Every cell has a fixed width so that the eye can run down a column, which is the reason the
    /// mockup uses a grid and not a flex row. `cwd` is the one that stretches, and only up to a
    /// ceiling; past that the gutter takes the slack, which is what keeps the two clusters two
    /// clusters at a window width the mockup never had.
    private var meta: some View {
        HStack(spacing: theme.metric(.rowGap)) {
            Text(Display.sourceLabel(conv.source))
                // A source the palette has no hue for reads in the quiet tier rather than in a
                // sixth colour invented here. The word still identifies it.
                .foregroundStyle(theme.color(Display.sourceHue(conv.source) ?? .ink3))
                .lineLimit(1)
                .truncationMode(.tail)
                .frame(width: cell(Columns.source), alignment: .leading)

            // `verbatim` because a `Text` built from a string *literal* is a `LocalizedStringKey`,
            // and an `Int` interpolated into one is formatted for the locale: this cell read
            // `2,553 ms…` — the separator pushed it past its width — until it was measured against
            // the largest conversation in the corpus. Plain digits are also what the terminal and
            // the mockup print.
            Text(verbatim: "\(conv.msgCount) msgs")
                .lineLimit(1)
                .frame(width: cell(Columns.size), alignment: .trailing)

            // Last in the left cluster rather than second, which is where the mockup puts it,
            // and the swap is what lets the cell be absent instead of blank. 1,300 rows of
            // 4,426 name no model — the whole of Google Takeout and a scattering elsewhere —
            // so a cell that comes and goes has to be the one next to the gutter, or the size
            // column steps left every time one of them scrolls past. Here the gutter absorbs
            // it and nothing else in the row moves.
            if let model = conv.model, !model.isEmpty {
                Text(model)
                    .foregroundStyle(theme.color(.ink3))
                    .lineLimit(1)
                    .truncationMode(.tail)
                    .frame(width: cell(Columns.model), alignment: .leading)
            }

            // The gutter takes the slack, and `cwd` takes it first up to a ceiling. `styles.css`
            // makes `cwd` the flexible track and its own comment says the gutter is — a
            // disagreement no fixed 706px column could show. A resizable window shows it at once:
            // let `cwd` grow without limit and at 1400pt the right cluster is not a cluster, with
            // the directory stranded mid-row and the age against the far edge. The ceiling is the
            // corpus's: 88% of directories are 35 characters or fewer and 91% are 44 or fewer, so
            // past that the tail is worktree paths, which elide to their leaf.
            Spacer(minLength: cell(Columns.gutter))

            Text(Display.dir(conv.cwd))
                .foregroundStyle(theme.color(hasDir ? .ink2 : .ink3))
                .lineLimit(1)
                // Middle, never tail: tail-elision discards the leaf, which is the discriminating
                // token — `~/dev/projects/personal-site` must not become `~/dev/projects/pe…`
                // (docs/TUI-DESIGN.md §2). The terminal reaches this with a character budget
                // because it has no text metrics; here the layout measures the string it actually
                // drew, so there is no budget to keep in sync with a column width.
                .truncationMode(.middle)
                .frame(maxWidth: cell(Columns.dir), alignment: .leading)
                // Sized before the gutter, which is what makes the ceiling above a ceiling: two
                // flexible siblings otherwise split the slack, leaving a path elided next to an
                // empty gap wide enough to have shown it.
                .layoutPriority(1)

            // A mark, not a column (`docs/TUI-DESIGN.md` §3). 45 conversations in 4,426 hold
            // more than one strand, so a cell reserved on every row would be three characters
            // of blank on 99% of them; absent, it costs the directory those characters only on
            // the rows that have something to say, and the directory is the cell built to
            // stretch. `age` is anchored to the trailing edge either way, so the right cluster
            // stays a cluster.
            if let forks = conv.forks {
                Text(verbatim: "\(Display.forkMark)\(forks)")
                    .foregroundStyle(theme.color(.ink3))
                    .lineLimit(1)
                    .frame(width: cell(Columns.forks), alignment: .trailing)
            }

            Text(Display.age(endedAt: conv.endedAt))
                .lineLimit(1)
                .frame(width: cell(Columns.age), alignment: .trailing)
        }
        .font(theme.font(.meta, .mono))
        .foregroundStyle(theme.color(.ink2))
    }

    /// Line 2: the title, the full width of the row.
    ///
    /// Tail-elided, unlike `cwd` and for the opposite reason — the front of a title is the
    /// discriminating part. 11 conversations in the corpus have none, and none of them is in the
    /// first 10 rows of anything, so the absence is drawn in the quiet tier rather than left to
    /// look like a title somebody chose.
    private var title: some View {
        Text(titleText ?? "untitled")
            .font(theme.font(.body, weight: .medium))
            .foregroundStyle(theme.color(titleText == nil ? .ink3 : .ink))
            .lineLimit(1)
            .truncationMode(.tail)
            .frame(maxWidth: .infinity, alignment: .leading)
    }

    /// Line 3: the true best match — the message that won the ranking, not the first prose one —
    /// framed with where it came from.
    ///
    /// 66–85% of message-level hits land in tool traffic, so an unframed snippet reads as log
    /// spew (`poc/ui/NOTES.md` §2). The frame the mockup draws is `⚙ Bash(cargo test) ›`; the tool
    /// name is not on the wire, so this is the signal glyph and the quotes.
    private func bestMatch(_ match: Match) -> some View {
        let snippet = Self.fragment(match, marks: marks, theme: theme)
        return HStack(alignment: .firstTextBaseline, spacing: characterWidth) {
            Text(Display.signal(role: match.role, kind: match.kind))
                .font(theme.font(.meta, .mono))
                .foregroundStyle(theme.color(.ink3))
            // The `⟨no match⟩` label, drawn outside the quotes because it is a label and not
            // content: inside them it would read as something the conversation said.
            if let label = snippet.label {
                Text(label)
                    .font(theme.font(.meta, .mono))
                    .foregroundStyle(theme.color(.ink3))
            }
            Text(snippet.text)
                .lineLimit(1)
                .truncationMode(.tail)
        }
        // Italic, as `.row-snip .frag` is: it is the one place in the row that quotes somebody.
        // A slant is not a token — the seam carries colours, sizes and faces, and a direction that
        // wanted this upright would be changing what the row says rather than what it looks like.
        .font(theme.font(.sub).italic())
        .foregroundStyle(theme.color(.ink2))
    }

    // MARK: - What the fields say

    /// Null and empty are the same absence here. The contract makes `title` nullable and says
    /// nothing about it being non-empty, and a row that drew one as "untitled" and the other as
    /// blank would be reporting a difference nobody meant.
    private var titleText: String? { conv.title.flatMap { $0.isEmpty ? nil : $0 } }
    private var hasDir: Bool { !(conv.cwd ?? "").isEmpty }

    /// The snippet as one line: the highlight runs the ranker returned, and the label if the
    /// match could not be located.
    ///
    /// The spans are drawn rather than re-derived, and they are dropped rather than guessed at
    /// when the envelope names an offset encoding this build has no reading for. No marks is a
    /// visible loss; marks in the wrong place is an invisible lie.
    private static func fragment(_ match: Match, marks: MarkOffsets, theme: Theme)
        -> (label: String?, text: AttributedString)
    {
        let snippet = match.snippet
        var body = AttributedString(snippet)
        for span in match.snippetSpans {
            guard let range = snippet.range(of: span, in: marks),
                let marked = Range(range, in: body)
            else { continue }
            body[marked].backgroundColor = theme.color(.hitBg)
            body[marked].foregroundColor = theme.color(.ink)
            // Upright inside the italic line, as `.row-snip mark` is: the highlight is the part
            // being read, and italic inside italic says nothing.
            body[marked].font = theme.font(.sub)
        }

        var label: String?
        if snippet.hasPrefix(Display.unlocated) {
            label = Display.unlocated.trimmingCharacters(in: .whitespaces)
            let prefix = snippet.startIndex..<snippet.index(
                snippet.startIndex, offsetBy: Display.unlocated.count)
            if let range = Range(prefix, in: body) { body.removeSubrange(range) }
        }

        var quoted = AttributedString("\u{201C}")
        quoted.append(body)
        quoted.append(AttributedString("\u{201D}"))
        return (label, quoted)
    }

    // MARK: - The column plan

    /// The fixed cells, in **characters of the meta face** rather than in points.
    ///
    /// The mockup states these in pixels because a stylesheet can; the terminal states its own in
    /// character cells because that is all a terminal has (`cs-tui/src/layout.rs`). Characters are
    /// the right unit here for the seam's sake: a direction that moves `--fs-meta` moves these
    /// columns with it, where a point width would leave the text to outgrow the cell it sits in.
    private enum Columns {
        /// `google-takeout` is 14 and holds 1,280 of the live index's conversations, so the
        /// column is sized to the longest label the corpus actually produces rather than to the
        /// terminal's 12, which clips it. A longer id than that tail-elides.
        static let source = 14.0
        /// `2553 msgs` — the largest conversation in the corpus is 2,553 messages.
        static let size = 9.0
        /// Sized the way the directory is, off what the corpus actually holds: 94.6% of model
        /// names are 16 characters or fewer, and the tail is 134 rows of
        /// `text-davinci-002-render-sha`, a ChatGPT internal name whose 134 instances are not
        /// told apart by the eleven characters it would take to finish it. Tail-elided, unlike
        /// the directory: `claude-opus-4-7` and `claude-opus-4-8` are 15 and both fit whole, so
        /// nothing discriminating is ever the part that goes.
        static let model = 16.0
        /// `⑂10` — the most-forked conversation in the corpus holds ten strands.
        static let forks = 3.0
        /// As much of a directory as the row will show before eliding it. Not a fixed width: the
        /// cell is smaller than this whenever the window is.
        static let dir = 44.0
        /// The narrowest the split between the two clusters gets. Wider than the gap between
        /// cells within one, which is what makes them read as two clusters rather than as nine
        /// columns, and it is the gutter that grows when the window does.
        static let gutter = 4.0
        /// `11mo` is the widest age `Display.age` renders.
        static let age = 4.0
    }

    private func cell(_ characters: Double) -> CGFloat { characterWidth * characters }

    /// One character of the meta face, measured off the theme's own size and design.
    private var characterWidth: CGFloat {
        FaceMetrics.advance(size: theme.size(.meta), design: theme.type.design(.mono))
    }
}

/// How wide one character is in a face the theme names.
///
/// Measured rather than assumed: SF Mono's advance happens to be 0.6 × its point size, and a
/// direction that moved `--mono` to a face with a different ratio would silently mis-size every
/// column that trusted the constant. Cached because a `List` asks once per row per update and the
/// answer only changes when a direction does.
@MainActor
private enum FaceMetrics {
    private struct Key: Hashable {
        let size: CGFloat
        let design: Font.Design
    }

    private static var cache: [Key: CGFloat] = [:]

    static func advance(size: CGFloat, design: Font.Design) -> CGFloat {
        let key = Key(size: size, design: design)
        if let known = cache[key] { return known }
        let width = ("0" as NSString).size(withAttributes: [.font: font(size, design)]).width
        cache[key] = width
        return width
    }

    /// The `NSFont` for a `Font.Design`, which is the one thing SwiftUI will not measure for us.
    /// Only the generic designs, because that is all `FaceToken` resolves to — a direction asking
    /// for a named family is `chat-search-me9.8.8`'s note, not this function's problem.
    private static func font(_ size: CGFloat, _ design: Font.Design) -> NSFont {
        let system = NSFont.systemFont(ofSize: size)
        switch design {
        case .monospaced: return .monospacedSystemFont(ofSize: size, weight: .regular)
        case .serif, .rounded:
            let wanted: NSFontDescriptor.SystemDesign = design == .serif ? .serif : .rounded
            guard let descriptor = system.fontDescriptor.withDesign(wanted),
                let font = NSFont(descriptor: descriptor, size: size)
            else { return system }
            return font
        default: return system
        }
    }
}
