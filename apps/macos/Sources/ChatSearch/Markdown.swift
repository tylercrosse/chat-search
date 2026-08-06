import CsKit
import CsTheme
import SwiftUI

/// Markdown, set over the source rather than cut out of it.
///
/// A quarter of the assistant's prose in this corpus carries `**`, an eighth carries a heading and
/// a twelfth carries a fence — and reasoning is denser than any of them, 77% asterisks with
/// essentially no code and no headings. The reader drew all of it literally, so the band that
/// benefits most was a wall of punctuation.
///
/// **The constraint that shapes everything here: nothing may move a byte.** `Block.marks` are
/// UTF-8 offsets into `text` and `runs(marking:in:)` slices the string by them, so a transform
/// that deleted the `**` would move every offset after it and silently mark the wrong words —
/// the failure `MarkOffsets` exists to prevent, arriving by another door. Styling over the source
/// has no such edge: a scanner that gets a line wrong renders it exactly as the reader renders it
/// today. The failure mode is *no styling*, never wrong highlighting and never corrupted text,
/// which is what makes a hand-rolled scanner safe here when it normally would not be.
///
/// It follows that the syntax stays on screen. It is dimmed rather than deleted — `.ink3`, which
/// is the same token the quiet tier is drawn in — so it recedes without going anywhere, and what
/// is copied out of the reader is the message exactly as the wire sent it.
///
/// ## What is deliberately not here
///
/// Italics, because a single `*` false-positives on globs and on multiplication where a doubled
/// one is unambiguous. Links, lists, blockquotes and tables, because nesting is where hand-rolled
/// scanners actually break. Inline `` `code` ``, which is common enough to be worth having and is
/// the one construct whose delimiter is a single character that also appears in fences — so it
/// waits for a scanner that shares state with them rather than guessing beside one. Any new
/// colour, because a code ground is two tokens across six directions and two sides through the
/// contrast gate. And syntax highlighting, which is `chat-search-me9.8.38`'s question.
enum Markdown {
    /// What a stretch of the source is, for the purpose of setting it. Not what it *means*: this
    /// is a face and a weight, and a scanner that reported meaning would owe an answer for the
    /// nesting it refuses to read.
    enum Style: Equatable {
        /// The ``` line that opens or closes a fence.
        case fence
        /// Everything between two fence lines.
        case code
        /// The `**` and the leading `#` — punctuation that is not content.
        case syntax
        /// What a pair of `**` surrounds.
        case strong
        /// What follows a `#` at the start of a line.
        case heading
    }

    /// One stretch of the source and how it is set. Ordered, and never overlapping: the styles are
    /// a partition of the parts of the source that are styled at all, which is what lets the merge
    /// with the marks below be a walk rather than a resolution of conflicts.
    struct Span: Equatable {
        let range: Range<String.Index>
        let style: Style
    }

    /// Every styled stretch of one message, in order.
    struct Styling {
        let spans: [Span]

        var isEmpty: Bool { spans.isEmpty }

        /// How many stretches of each kind, for the pass that has to say the scanner did anything.
        func count(_ style: Style) -> Int { spans.filter { $0.style == style }.count }

        /// A stretch of the source cut so that no piece straddles a style boundary.
        ///
        /// The caller hands in one marked or unmarked run and gets back the pieces of it, each
        /// with the style in force there or nil. That is the whole of the merge between the two
        /// span sets, and it is this small because neither set overlaps itself.
        func pieces(in range: Range<String.Index>) -> [(range: Range<String.Index>, style: Style?)] {
            guard !spans.isEmpty else { return [(range, nil)] }
            var out: [(range: Range<String.Index>, style: Style?)] = []
            var cursor = range.lowerBound
            for span in spans {
                if span.range.lowerBound >= range.upperBound { break }
                let lower = Swift.max(span.range.lowerBound, cursor)
                let upper = Swift.min(span.range.upperBound, range.upperBound)
                guard lower < upper else { continue }
                if lower > cursor { out.append((cursor..<lower, nil)) }
                out.append((lower..<upper, span.style))
                cursor = upper
            }
            if cursor < range.upperBound { out.append((cursor..<range.upperBound, nil)) }
            return out
        }
    }

    /// How this message is set, or nothing at all — the gate, and it is two gates.
    ///
    /// **Never tool traffic.** 32,813 of the corpus's 137,393 tool messages contain a `#` or a
    /// `*`, and almost none of it is markdown: a shell command, a diff, a glob and a JSON key all
    /// carry those characters as themselves. Styling a quarter of the tool band on that evidence
    /// would be a claim about what those messages are, made by a scanner that cannot read them.
    ///
    /// **Never a collapsed message.** A fold is one line with its line breaks spent as spaces
    /// (`lineBreaksAsSpaces`), which is a string every line-oriented rule below would read wrong —
    /// and a heading, a fence and a paragraph all render as the same single truncated row anyway,
    /// so there is nothing there for markdown to say.
    ///
    /// The source is handed in rather than read off the block, because those two are the same
    /// string only while the second gate holds and this should not be the third place that is
    /// true by coincidence.
    static func styling(_ source: String, of block: Block, fold: Fold) -> Styling {
        guard fold == .expanded else { return Styling(spans: []) }
        switch block.band {
        case .user, .agent, .reasoning: return Styling(spans: scan(source))
        // An unrecognised band takes the quiet reading for the reason `Display.bandToken` gives it
        // the quiet colour: a band that postdates this build might be anything, and tool traffic
        // is what it would most likely be.
        case .tool, .unrecognised: return Styling(spans: [])
        }
    }

    /// The scan. Fences first and whole, then headings and bold inside what is left.
    ///
    /// Fences first is not an optimisation: code containing `**` is code, and a scanner that
    /// looked for bold before it knew where the fences were would set half a Rust generic in
    /// semibold. Everything below is per line, and bold does not cross one — CommonMark lets it,
    /// and a single unclosed `**` in a 40 KB message would then style the rest of the message.
    /// Confining the damage to its own line is worth more here than the paragraph-spanning bold
    /// nobody writes.
    static func scan(_ source: String) -> [Span] {
        let lines = lines(of: source)
        var spans: [Span] = []
        var index = 0
        while index < lines.count {
            let line = lines[index]
            if let fence = opener(source, line.content) {
                // An unclosed fence runs to the end of the message, which is what a reader sees in
                // any other markdown renderer and is usually a message that was cut off.
                let closer = ((index + 1)..<lines.count).first {
                    closes(source, lines[$0].content, fence)
                }
                spans.append(Span(range: line.whole, style: .fence))
                // One span for the whole interior rather than one per line, so a fence is one
                // block of monospaced text with its own line breaks inside it.
                let first = index + 1
                let last = (closer ?? lines.count) - 1
                if first <= last {
                    spans.append(
                        Span(
                            range: lines[first].whole.lowerBound..<lines[last].whole.upperBound,
                            style: .code))
                }
                if let closer { spans.append(Span(range: lines[closer].whole, style: .fence)) }
                index = (closer ?? lines.count - 1) + 1
                continue
            }
            if let heading = heading(source, line.content) {
                spans.append(Span(range: heading.hashes, style: .syntax))
                if !heading.text.isEmpty { spans.append(Span(range: heading.text, style: .heading)) }
                index += 1
                continue
            }
            spans.append(contentsOf: strong(source, in: line.content))
            index += 1
        }
        return spans
    }

    // MARK: - Lines

    /// Each line's own characters, and the same line with whatever ended it.
    ///
    /// Two ranges because they are wanted in two places: a fence is set in the monospaced face
    /// through its own line breaks, so that a code block is one block and not a stack of lines
    /// with default-face gaps between them, while a heading stops at its last character — a line
    /// break one size up would open a gap under every heading that nothing asked for.
    ///
    /// `isNewline` rather than a test against `\n`: Swift reads `\r\n` as one `Character`, which
    /// is the same fact `lineBreaksAsSpaces` is spelled in bytes to survive.
    private static func lines(
        of source: String
    ) -> [(content: Range<String.Index>, whole: Range<String.Index>)] {
        var out: [(content: Range<String.Index>, whole: Range<String.Index>)] = []
        var start = source.startIndex
        var cursor = source.startIndex
        while cursor < source.endIndex {
            let next = source.index(after: cursor)
            if source[cursor].isNewline {
                out.append((start..<cursor, start..<next))
                start = next
            }
            cursor = next
        }
        if start < source.endIndex { out.append((start..<source.endIndex, start..<source.endIndex)) }
        return out
    }

    // MARK: - Fences

    /// The run of backticks or tildes a fence was opened with. A closer has to match the character
    /// and be at least as long, which is what lets a fence contain a shorter run of its own marker.
    private struct Fence {
        let marker: Character
        let count: Int
    }

    private static func opener(_ source: String, _ line: Range<String.Index>) -> Fence? {
        let start = afterIndent(source, line)
        guard start < line.upperBound else { return nil }
        let marker = source[start]
        guard marker == "`" || marker == "~" else { return nil }
        let end = run(source, of: marker, from: start, upTo: line.upperBound)
        let count = source.distance(from: start, to: end)
        guard count >= 3 else { return nil }
        // A backtick in the info string means this is an inline code span with backticks in it and
        // not a fence at all — CommonMark's rule, and the one case where the two constructs this
        // file reads and refuses to read are told apart by the same character.
        guard marker == "~" || !source[end..<line.upperBound].contains("`") else { return nil }
        return Fence(marker: marker, count: count)
    }

    private static func closes(_ source: String, _ line: Range<String.Index>, _ fence: Fence)
        -> Bool
    {
        let start = afterIndent(source, line)
        guard start < line.upperBound, source[start] == fence.marker else { return false }
        let end = run(source, of: fence.marker, from: start, upTo: line.upperBound)
        guard source.distance(from: start, to: end) >= fence.count else { return false }
        return source[end..<line.upperBound].allSatisfy(\.isWhitespace)
    }

    // MARK: - Headings

    /// An ATX heading: up to three spaces, one to six `#`, then a space or the end of the line.
    ///
    /// The space is CommonMark's requirement and it is what keeps `#42` and `#!/bin/sh` out — both
    /// of which are ordinary things to write in a message about code.
    private static func heading(_ source: String, _ line: Range<String.Index>)
        -> (hashes: Range<String.Index>, text: Range<String.Index>)?
    {
        let start = afterIndent(source, line)
        guard start < line.upperBound, source[start] == "#" else { return nil }
        let end = run(source, of: "#", from: start, upTo: line.upperBound)
        guard (1...6).contains(source.distance(from: start, to: end)) else { return nil }
        guard end == line.upperBound || source[end] == " " || source[end] == "\t" else { return nil }
        var text = end
        while text < line.upperBound, source[text] == " " || source[text] == "\t" {
            text = source.index(after: text)
        }
        return (start..<end, text..<line.upperBound)
    }

    // MARK: - Bold

    /// Every `**…**` on one line, markers and all, in order.
    private static func strong(_ source: String, in line: Range<String.Index>) -> [Span] {
        var out: [Span] = []
        var cursor = line.lowerBound
        while let open = doubleStar(source, from: cursor, upTo: line.upperBound) {
            let inner = source.index(open, offsetBy: 2)
            guard let close = doubleStar(source, from: inner, upTo: line.upperBound) else { break }
            // `****` and nothing between: not emphasis, and skipping past it rather than treating
            // the second pair as an opener is what stops a row of asterisks pairing off.
            guard close > inner else {
                cursor = source.index(close, offsetBy: 2)
                continue
            }
            out.append(Span(range: open..<inner, style: .syntax))
            out.append(Span(range: inner..<close, style: .strong))
            let after = source.index(close, offsetBy: 2)
            out.append(Span(range: close..<after, style: .syntax))
            cursor = after
        }
        return out
    }

    /// Where the next `**` starts, or nil if there is none before the end of the line.
    private static func doubleStar(
        _ source: String, from: String.Index, upTo end: String.Index
    ) -> String.Index? {
        var cursor = from
        while cursor < end {
            let next = source.index(after: cursor)
            if source[cursor] == "*", next < end, source[next] == "*" { return cursor }
            cursor = next
        }
        return nil
    }

    // MARK: - Shared

    /// Past up to three leading spaces, which is what every block construct here allows before it
    /// stops being one. A fourth space is an indented code block in CommonMark; this reads it as
    /// prose, which is the quiet reading and the one that cannot be wrong on screen.
    private static func afterIndent(_ source: String, _ line: Range<String.Index>) -> String.Index {
        var cursor = line.lowerBound
        var spaces = 0
        while cursor < line.upperBound, source[cursor] == " ", spaces < 3 {
            cursor = source.index(after: cursor)
            spaces += 1
        }
        return cursor
    }

    /// The end of the run of `character` starting where the caller is standing.
    private static func run(
        _ source: String, of character: Character, from: String.Index, upTo end: String.Index
    ) -> String.Index {
        var cursor = from
        while cursor < end, source[cursor] == character { cursor = source.index(after: cursor) }
        return cursor
    }
}

extension Markdown.Style {
    /// The face, the weight and — for syntax only — the tone this stretch is set in.
    ///
    /// **Existing tokens, and no new ones.** `.mono` and `.ink3` are the quiet tier the reader
    /// already draws its tool traffic in, semibold is a weight rather than a value, and a heading
    /// steps up the same five-size scale everything else is set on. Nothing here reaches
    /// `ThemeCheck`, because nothing here is a colour anybody had to solve — which is the whole
    /// reason a code *ground* is out of scope: that would be two tokens through the contrast gate
    /// in six directions on both sides, and Solarized is the direction where the darkest colour
    /// published is the page itself.
    ///
    /// Set relative to the face the message is already in, so this reads the same in the reading
    /// face a user turn is set in and in the quiet monospaced tier reasoning gets. In reasoning
    /// that makes `.fence` and `.code` a no-op on the face and a dimming on the fence line, which
    /// is correct rather than a gap: mono text does not become more mono.
    func apply(to text: inout AttributedString, base: Display.TextFace, theme: Theme) {
        switch self {
        case .fence:
            text.font = theme.font(base.size, .mono)
            text.foregroundColor = theme.color(.ink3)
        case .code:
            text.font = theme.font(base.size, .mono)
        case .syntax:
            text.foregroundColor = theme.color(.ink3)
        case .strong:
            text.font = theme.font(base.size, base.face, weight: .semibold)
        case .heading:
            text.font = theme.font(base.size.larger, base.face, weight: .semibold)
        }
    }
}
