import CsKit
import CsTheme
import SwiftUI

/// Each drawn message's text with its matches marked and its markdown set, built once rather than
/// once a frame.
///
/// `cs_core::blocks` refused exactly this shape one layer down. Marks are held on the block rather
/// than located at render time "because locating them means tokenizing the message — ~25 µs fixed
/// plus ~60 ns per byte — and a renderer runs on every frame, so marking there would pay that on
/// every wheel notch and every keystroke". Core paid once and the client then reintroduced the
/// cost at the `AttributedString` layer: `marked` was a computed property on a `View`, so every
/// body evaluation cut the whole message into runs and concatenated a fresh attributed copy of it.
/// `List` prepares rows well past the viewport and prepares the same row again each time it
/// crosses that edge, so a fling pays it many times over (`chat-search-me9.8.29`).
///
/// What a marked message is a function of, and therefore what this is keyed on:
///
/// - **Which message**, by its permanent id.
/// - **How it is folded**, because a collapsed message is drawn from a different string — line
///   breaks spent as spaces. One entry per message rather than one per (message, fold): toggling
///   replaces the entry instead of leaving both forms of a 40 KB tool result resident.
/// - **Which question the marks answer** — the conversation, and the terms it was marked against.
///   Both arrive with the transcript, which is why the query is not passed in: a re-read against
///   the same query produces the same marks and may reuse them, and a re-read against a different
///   one arrives with different `terms`.
/// - **Which direction is drawing**, since a mark bakes in three colour tokens and the markdown
///   over it bakes in a face and two sizes. Named by `Theme.name`, which is what
///   `ThemeSettings.choose(light:)` already treats as a direction's identity. Appearance is
///   deliberately not part of it: every token is one dynamic `NSColor` that picks its own side
///   where it is drawn, so a light/dark flip repaints these without rebuilding them — and the type
///   scale does not travel with a side at all, which is `Theme.composed`'s decision and the reason
///   an unattended sunset cannot relayout a conversation somebody is reading.
///
/// The first two are per entry. The last two invalidate every entry at once, which is what makes
/// a stale mark unreachable rather than merely unlikely.
@MainActor
final class MarkedText {
    private var built: [String: Entry] = [:]
    private var answering: Question?

    /// How many messages this has assembled, and how many it has handed back already assembled.
    ///
    /// Not private, for the reason `MinimapBands.renders` is not: the claim above is "once, not
    /// once a frame", and the wall clock cannot check it — a fling's frame lag on this machine
    /// swings further between runs of one build than the whole of this change is worth. These two
    /// can. Their sum is what the computed property this replaced would have built in full, so
    /// `reuses` is the work avoided, counted rather than inferred from a percentile.
    private(set) var builds = 0
    private(set) var reuses = 0

    /// Put the counters back where a pass found them.
    ///
    /// `--shot` drives several things in one run and only one of them is a claim about a scroll.
    /// Turning the fidelity knobs rebuilds every message on purpose — that is what a fold change
    /// *is* — so a pass that does it would leave these reading like the regression they exist to
    /// detect. The pass that disturbs them scopes itself instead of the passes downstream having
    /// to know about it. Never called by anything a person can reach.
    func restoreCounters(builds: Int, reuses: Int) {
        self.builds = builds
        self.reuses = reuses
    }

    private struct Entry {
        let fold: Fold
        let text: AttributedString
    }

    /// What every entry is an answer to. Not a key in the ordinary sense — when this changes there
    /// is nothing worth keeping, so the whole table goes rather than being searched.
    private struct Question: Equatable {
        let conv: String
        let terms: [String]
        let direction: String
    }

    /// The message as it is drawn: off the table when it is there, built and kept when it is not.
    func of(_ block: Block, fold: Fold, in transcript: Transcript, theme: Theme) -> AttributedString
    {
        let question = Question(
            conv: transcript.convId, terms: transcript.terms, direction: theme.name)
        if answering != question {
            built.removeAll(keepingCapacity: true)
            answering = question
        }
        if let entry = built[block.id], entry.fold == fold {
            reuses += 1
            return entry.text
        }
        let text = Self.mark(block, fold: fold, in: transcript.markOffsets, theme: theme)
        built[block.id] = Entry(fold: fold, text: text)
        builds += 1
        return text
    }

    /// Forget everything.
    ///
    /// The drawer closing is the one change nothing above can notice: no view asks for a message
    /// of a conversation nobody is reading, so the table would hold the last transcript's text for
    /// as long as the app ran. That is the cache `ReaderModel` already refuses to keep for the
    /// transcript itself, and it would be no better made of `AttributedString`.
    func forget() {
        built.removeAll()
        answering = nil
    }

    /// The message with its matches marked and its markdown set, in the units the transcript named.
    ///
    /// The two mark kinds are told apart by *form* and not by hue — a filled ground against an
    /// underline — for the same reason the TUI spends a text modifier on them: `--hit` and
    /// `--hit-bg` are one colour family, and a claim as consequential as "this is why the
    /// conversation is on screen" should not rest on which shade of amber it happens to be.
    ///
    /// Two span sets are laid over one string here and they answer different questions, so the cut
    /// is by both and the attributes are applied in an order that says which wins. Markdown goes
    /// on first and owns the *face*: a fence is monospaced whether or not the query matched inside
    /// it. The mark goes on second and owns the *colour*, so a term that lands on a `**` or inside
    /// a code block is still drawn as the reason this conversation is on screen — which is the one
    /// claim the reader came here to check, and the one that must not be dimmed by punctuation.
    private static func mark(
        _ block: Block, fold: Fold, in units: MarkOffsets, theme: Theme
    ) -> AttributedString {
        // Line breaks become spaces one byte at a time when collapsed, which is what keeps every
        // mark offset pointing at the character it was measured against.
        let source = fold == .collapsed ? block.text.lineBreaksAsSpaces : block.text
        let styling = Markdown.styling(source, of: block, fold: fold)
        let base = Display.textFace(block)
        var out = AttributedString()
        for run in source.runs(marking: block.marks, in: units) {
            for cut in styling.pieces(in: run.range) {
                var piece = AttributedString(source[cut.range])
                cut.style?.apply(to: &piece, base: base, theme: theme)
                if run.marked {
                    if block.markKind.claimsRanking {
                        piece.backgroundColor = theme.color(.hitBg)
                        piece.foregroundColor = theme.color(.ink)
                    } else {
                        piece.underlineStyle = .single
                        piece.foregroundColor = theme.color(.hit)
                    }
                }
                out.append(piece)
            }
        }
        return out
    }
}

extension MarkedText {
    /// A built message read back out of the drawing rather than off the string it was built from.
    ///
    /// `--shot`'s markdown pass is the only caller, and this lives here rather than there for one
    /// reason: reading an attribute back needs the same scope that wrote it, and `Measure` also
    /// imports AppKit — where `backgroundColor` is a second attribute of the same name. Beside the
    /// write is the one place that ambiguity cannot arise.
    enum Drawn {
        /// The marked stretches, in order.
        ///
        /// Adjacent runs are rejoined, because markdown cuts a mark wherever a `**` or a fence
        /// boundary falls inside one and the claim being checked is about the stretch rather than
        /// about the cut. Told apart by the two attributes a mark sets and markdown never does —
        /// a ground and an underline — rather than by comparing colours, which would be a check on
        /// `Color`'s equality as much as on this.
        static func marks(of text: AttributedString) -> [String] {
            var out: [String] = []
            var current = ""
            for run in text.runs {
                let piece = String(text[run.range].characters)
                if run.backgroundColor != nil || run.underlineStyle != nil {
                    current += piece
                } else if !current.isEmpty {
                    out.append(current)
                    current = ""
                }
            }
            if !current.isEmpty { out.append(current) }
            return out
        }

        /// How many runs carry a face of their own — which is everything markdown set and nothing
        /// a mark did, since a mark spends colour and an underline and never a font.
        static func faced(of text: AttributedString) -> Int {
            text.runs.count { $0.font != nil }
        }
    }
}
