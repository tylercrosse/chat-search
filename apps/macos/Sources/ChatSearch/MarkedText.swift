import CsKit
import CsTheme
import SwiftUI

/// Each drawn message's text with its matches marked, built once rather than once a frame.
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
/// - **Which direction is drawing**, since a mark bakes in three colour tokens. Named by
///   `Theme.name`, which is what `ThemeSettings.choose(light:)` already treats as a direction's
///   identity. Appearance is deliberately not part of it: every token is one dynamic `NSColor`
///   that picks its own side where it is drawn, so a light/dark flip repaints these without
///   rebuilding them.
///
/// The first two are per entry. The last two invalidate every entry at once, which is what makes
/// a stale mark unreachable rather than merely unlikely.
@MainActor
final class MarkedText {
    private var built: [String: Entry] = [:]
    private var answering: Question?

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
        if let entry = built[block.id], entry.fold == fold { return entry.text }
        let text = Self.mark(block, fold: fold, in: transcript.markOffsets, theme: theme)
        built[block.id] = Entry(fold: fold, text: text)
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

    /// The message with its matches marked, in the units the transcript named.
    ///
    /// The two mark kinds are told apart by *form* and not by hue — a filled ground against an
    /// underline — for the same reason the TUI spends a text modifier on them: `--hit` and
    /// `--hit-bg` are one colour family, and a claim as consequential as "this is why the
    /// conversation is on screen" should not rest on which shade of amber it happens to be.
    private static func mark(
        _ block: Block, fold: Fold, in units: MarkOffsets, theme: Theme
    ) -> AttributedString {
        // Line breaks become spaces one byte at a time when collapsed, which is what keeps every
        // mark offset pointing at the character it was measured against.
        let source = fold == .collapsed ? block.text.lineBreaksAsSpaces : block.text
        var out = AttributedString()
        for run in source.runs(marking: block.marks, in: units) {
            var piece = AttributedString(run.text)
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
        return out
    }
}
