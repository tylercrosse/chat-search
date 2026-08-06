import Foundation
import SwiftUI

/// A token set read off a file somebody wrote, and the same token set written back out.
///
/// The loop this exists to shorten is `chat-search-me9.8.10`'s: dialling in typography, sizing and
/// padding used to be edit `styles.css`, run `tokens.py`, `swift build`, relaunch. With a file here
/// it is edit and relaunch, and the values being dialled are exactly the ones somebody wants to
/// nudge twenty times in an afternoon.
///
/// ## Why CSS custom properties and not TOML
///
/// TOML would match `cs`'s own config, and that is the whole of its case. Against it: these values
/// are *already* authored as custom properties — in `poc/ui/styles.css`, in `poc/ui/directions.css`,
/// in the `ColorToken` raw values right here — so CSS is the one syntax where a line moves between
/// the mockup, the generator and this file without being rewritten, and where the name in an error
/// message is the name in all three. A TOML file would need a table of its own keys against these,
/// which is the translation table `Theme.swift` says the raw values exist to avoid. The cost is the
/// scanner below, and it is bounded because this is not a CSS engine: three selectors, one
/// declaration shape, and anything else is a sentence with a line number on it.
///
/// ## Why the file has to be complete
///
/// `docs/DECISIONS.md` ADR 25: a user theme is drawn as authored, entire, and never merged with a
/// direction — half a palette from each is a palette nobody designed and nobody can debug. So a
/// file missing a token is not a theme, and the app falls back and says which tokens.
///
/// That would be a cruel thing to ask of a hand-typed file, which is why `text(for:)` exists:
/// `--write-theme` dumps a direction the build already carries as exactly the file this reads, so
/// dialling in starts from a complete set rather than from a blank page. Writing one out is not the
/// app authoring a theme — ADR 25's revisit trigger — because every value in it was solved by
/// `poc/ui/palette.py` and compiled in long before. This prints them.
public enum ThemeFile {
    /// Why a file is not a theme. Both cases are recoverable — the app draws a direction instead —
    /// and both owe an explanation, because a theme that silently does nothing is the failure this
    /// whole surface is most likely to have.
    public enum Unreadable: Error {
        /// Nothing at that path, or nothing readable there. Only worth saying out loud when a flag
        /// named it: no file at the standard location is this app's ordinary state.
        case missing(URL)
        /// Something at that path that is not a theme: a line that will not parse, a token this
        /// build has no name for, a value in the wrong unit, or a set with holes in it.
        case notATheme(URL, [String])

        /// Whole sentences for stderr, each naming the file. `Report.failures`' register, and for
        /// its reason: nobody who has just launched an app has a table in front of them.
        public var sentences: [String] {
            switch self {
            case .missing(let url): ["no theme file at \(url.path)."]
            case .notATheme(let url, let reasons):
                reasons.map { "\(url.lastPathComponent): \($0)" }
            }
        }
    }

    /// The theme in that file, named after the file.
    ///
    /// The *file* names it, rather than a `--name` line inside it, because a name is not a token and
    /// a token file that carried one would have somewhere for the next non-token to go. `theme.css`
    /// is `theme`; `nightshift.css` is `nightshift`.
    public static func load(_ url: URL) throws -> Theme {
        guard let text = try? String(contentsOf: url, encoding: .utf8) else {
            throw Unreadable.missing(url)
        }
        let name = url.deletingPathExtension().lastPathComponent
        switch parse(text) {
        case .failure(let stopped): throw Unreadable.notATheme(url, stopped.reasons)
        case .success(let declarations): return try assemble(name: name, declarations, url)
        }
    }

    /// Where the app looks when no flag names a file.
    ///
    /// `cs`'s directory rather than `~/Library/Preferences`, where this app's own four preferences
    /// live, because the two are not the same kind of thing: a plist holds state the client wrote
    /// about itself and reads back with `defaults read`, and this is a document somebody authored in
    /// an editor. `~/.config/chat-search/` is already where a person of this project keeps a file
    /// they wrote — `config.toml` is there — and a theme is the second one.
    public static var standardLocation: URL {
        FileManager.default.homeDirectoryForCurrentUser
            .appending(path: ".config/chat-search/theme.css")
    }
}

// MARK: - Reading

extension ThemeFile {
    /// One declaration, with where it was written. The line number is the point: a hand-typed file's
    /// commonest failure is a typo in a token name, and "`--ink3` is not a token" is a much shorter
    /// hunt with a line beside it.
    private struct Declaration {
        let selector: String
        let token: String
        let value: String
        let line: Int
    }

    /// What a scan that stopped has to say. A struct rather than the bare list `Result` cannot
    /// carry, and one reason where `assemble` reports every one: the scanner cannot see past the
    /// line it choked on, so a second complaint from it would be a guess.
    private struct Unscannable: Error {
        let reasons: [String]
    }

    /// Enough CSS to read tokens, and deliberately not one thing more.
    ///
    /// `poc/ui/palette.py` reads the same subset through two regular expressions and with the same
    /// caveat — it "only has to be right about single-class selectors carrying custom properties,
    /// which is all either file uses". This is a scanner rather than two regexes because it can then
    /// say *where*, and because a regex that quietly matched nothing would leave a file with no
    /// tokens in it, which reads exactly like a file with a typo in every one.
    private static func parse(_ text: String) -> Result<[Declaration], Unscannable> {
        var declarations: [Declaration] = []
        var index = text.startIndex
        var line = 1

        func advance() {
            if text[index] == "\n" { line += 1 }
            index = text.index(after: index)
        }
        func next(is character: Character) -> Bool {
            let after = text.index(after: index)
            return after < text.endIndex && text[after] == character
        }
        /// Whitespace and `/* … */`, which is where the header `--write-theme` puts on a file lives.
        func skipBlanks() {
            while index < text.endIndex {
                if text[index].isWhitespace {
                    advance()
                } else if text[index] == "/", next(is: "*") {
                    advance()
                    advance()
                    while index < text.endIndex {
                        if text[index] == "*", next(is: "/") {
                            advance()
                            advance()
                            break
                        }
                        advance()
                    }
                } else {
                    return
                }
            }
        }
        /// Everything up to one of `stop`, or nil at the end of the file.
        func take(until stop: Set<Character>) -> String? {
            var taken = ""
            while index < text.endIndex {
                if stop.contains(text[index]) { return taken }
                taken.append(text[index])
                advance()
            }
            return nil
        }

        while true {
            skipBlanks()
            guard index < text.endIndex else { break }
            let opened = line
            guard let selector = take(until: ["{", "}", ";"]), text[index] == "{" else {
                return .failure(Unscannable(reasons: [
                    "line \(opened): a theme file is blocks of `selector { --token: value; }`, and "
                        + "\(quoted(sourceLine(text, opened))) is not inside one."
                ]))
            }
            advance()
            while true {
                skipBlanks()
                guard index < text.endIndex else {
                    return .failure(
                        Unscannable(reasons: ["line \(opened): this block is never closed."]))
                }
                if text[index] == "}" {
                    advance()
                    break
                }
                let at = line
                guard let token = take(until: [":", "}", ";"]), text[index] == ":" else {
                    return .failure(Unscannable(reasons: [
                        "line \(at): expected `--token: value;` and found "
                            + "\(quoted(sourceLine(text, at)))."
                    ]))
                }
                advance()
                guard let value = take(until: [";", "}"]), text[index] == ";" else {
                    return .failure(
                        Unscannable(reasons: [
                            "line \(at): \(quoted(token)) has no `;` after its value."
                        ]))
                }
                advance()
                declarations.append(
                    Declaration(
                        selector: selector.trimmed, token: token.trimmed, value: value.trimmed,
                        line: at))
            }
        }
        return declarations.isEmpty
            ? .failure(Unscannable(reasons: ["there are no tokens in it."]))
            : .success(declarations)
    }

    /// The offending line itself, so a complaint is about something the author can see rather than
    /// about a position in a file they are reading from the other end.
    private static func sourceLine(_ text: String, _ line: Int) -> String {
        let lines = text.split(separator: "\n", omittingEmptySubsequences: false)
        return line <= lines.count ? String(lines[line - 1]) : ""
    }

    /// Which side a block is, and nothing else a selector could mean.
    ///
    /// The three names are `poc/ui/palette.py`'s own light and dark tiers, minus the `.dir-X` ones —
    /// a file that is one theme has no direction to be. So a block naming one is a block whose
    /// values would silently never be read, which is the failure this file exists to prevent rather
    /// than to commit.
    private enum Side {
        case both, light, dark

        init?(selector: String) {
            switch selector {
            case ":root": self = .both
            case ":root.light", ".theme-light": self = .light
            case ".theme-dark": self = .dark
            default: return nil
            }
        }
    }

    /// Every declaration in its place, or every reason it is not a theme.
    ///
    /// Every reason and not the first: somebody who has just typed thirty colours wants the list,
    /// and a loader that reported one typo per relaunch would be a loop of its own.
    private static func assemble(name: String, _ declarations: [Declaration], _ url: URL) throws
        -> Theme
    {
        var reasons: [String] = []
        var dark: [ColorToken: RGB] = [:]
        var light: [ColorToken: RGB] = [:]
        var sizes: [SizeToken: CGFloat] = [:]
        var faces: [FaceToken: Font.Design] = [:]
        var metrics: [MetricToken: CGFloat] = [:]

        for declaration in declarations {
            guard let side = Side(selector: declaration.selector) else {
                reasons.append(
                    "line \(declaration.line): \(quoted(declaration.selector)) is not a block this "
                        + "reads. A theme file is one theme: `:root` carries the dark side, the "
                        + "type scale and the spacing, and `:root.light` carries the light side.")
                continue
            }
            let token = declaration.token
            if let colour = ColorToken(rawValue: token) {
                guard let rgb = hex(declaration.value) else {
                    reasons.append(
                        "line \(declaration.line): \(token) is \(quoted(declaration.value)), and a "
                            + "colour here is six hex digits — #1b2123.")
                    continue
                }
                if side != .light { dark[colour] = rgb }
                if side != .dark { light[colour] = rgb }
                continue
            }
            // Whether this build has heard of it at all comes before where it was written: a
            // mistyped `--ink3` in the light block is a typo, and telling its author it is in the
            // wrong block would send them to fix the one thing that is not wrong with it.
            let known =
                SizeToken(rawValue: token) != nil || MetricToken(rawValue: token) != nil
                || FaceToken(rawValue: token) != nil
            guard known else {
                reasons.append(
                    "line \(declaration.line): \(token) is not a token this build carries. "
                        + "`--write-theme` writes a complete file to start from.")
                continue
            }
            // The type scale and the geometry are one set against two palettes, which is the shape
            // `Theme` holds and the invariant `tokens.py` refuses to generate around. A size in a
            // side block has nowhere to go, so it is said rather than dropped.
            guard side == .both else {
                reasons.append(
                    "line \(declaration.line): \(token) is not a colour, and the type scale and the "
                        + "spacing are one set for both sides — it belongs in `:root`.")
                continue
            }
            if let size = SizeToken(rawValue: token) {
                guard let points = points(declaration.value) else {
                    reasons.append(lengthReason(declaration))
                    continue
                }
                sizes[size] = points
            } else if let metric = MetricToken(rawValue: token) {
                guard let points = points(declaration.value) else {
                    reasons.append(lengthReason(declaration))
                    continue
                }
                metrics[metric] = points
            } else if let face = FaceToken(rawValue: token) {
                guard let design = design(declaration.value) else {
                    reasons.append(
                        "line \(declaration.line): the stack \(quoted(declaration.value)) does not "
                            + "end in one of the generic families this platform models "
                            + "(\(designs.keys.sorted().joined(separator: ", "))). Only the generic "
                            + "survives here — a named face is a change to the token layer.")
                    continue
                }
                faces[face] = design
            }
        }

        // Completeness last, so a run reports the typo *and* the hole it left rather than one of
        // them — a mistyped `--ink3` is both, and hearing about them one launch apart is what would
        // make this read as an argument rather than as a check.
        missing(ColorToken.allCases, dark, "the dark side", &reasons)
        missing(ColorToken.allCases, light, "the light side", &reasons)
        missing(SizeToken.allCases, sizes, "the type scale", &reasons)
        missing(FaceToken.allCases, faces, "the faces", &reasons)
        missing(MetricToken.allCases, metrics, "the spacing", &reasons)

        guard reasons.isEmpty else { throw Unreadable.notATheme(url, reasons) }
        // Validated, and only now constructed: `Palette` treats an incomplete set as a programmer
        // error, which is right for a generated file and fatal for a typed one (ADR 25).
        return Theme(
            name: name, dark: Palette(dark), light: Palette(light),
            type: TypeScale(sizes: sizes, faces: faces), geometry: Geometry(metrics))
    }

    private static func missing<Token: RawRepresentable & Hashable, Value>(
        _ all: [Token], _ got: [Token: Value], _ what: String, _ reasons: inout [String]
    ) where Token.RawValue == String {
        let absent = all.filter { got[$0] == nil }.map(\.rawValue)
        guard !absent.isEmpty else { return }
        // `tokens.py`'s wording, because it is the same finding from the other end of the pipe:
        // "no value in force for" is what the generator says when a stylesheet has a hole in it.
        reasons.append(
            "\(what): no value in force for \(absent.joined(separator: ", ")). A theme is drawn "
                + "entire or not at all, so a file with a hole in it is not one "
                + "(docs/DECISIONS.md ADR 25).")
    }

    private static func lengthReason(_ declaration: Declaration) -> String {
        "line \(declaration.line): \(declaration.token) is \(quoted(declaration.value)), and a "
            + "length here is px — 12px, or 12.5px. A CSS pixel is a point one for one."
    }

    /// `#1b2123`, and nothing else. The measurements that fence half these tokens are defined on
    /// eight-bit sRGB, so a value that arrived through another colour space is a value they no
    /// longer describe — `RGB` says the same where it refuses to store a `Color`.
    private static func hex(_ value: String) -> RGB? {
        let digits = value.dropFirst()
        guard value.hasPrefix("#"), digits.count == 6, let packed = UInt32(digits, radix: 16)
        else { return nil }
        return RGB(packed)
    }

    /// CSS px to points, one for one, which is what `poc/ui/tokens.py` does and for its reason: the
    /// prototype renders at native metrics on purpose, so its numbers are already native numbers.
    private static func points(_ value: String) -> CGFloat? {
        if value == "0" { return 0 }
        guard value.hasSuffix("px"), let number = Double(value.dropLast(2)) else { return nil }
        return CGFloat(number)
    }

    /// The generic family a stack falls back to is the part SwiftUI models, and on this platform it
    /// is also the part that is true. A direction that names a specific face lands on the generic,
    /// which `FaceToken` says where it is declared.
    private static let designs: [String: Font.Design] = [
        "sans-serif": .default, "serif": .serif, "monospace": .monospaced,
    ]

    private static func design(_ stack: String) -> Font.Design? {
        let last = stack.split(separator: ",").last?.trimmed ?? ""
        return designs[last.trimmingCharacters(in: CharacterSet(charactersIn: "\"'"))]
    }

    private static func quoted(_ text: String) -> String { "`\(text.trimmed)`" }
}

// MARK: - Writing

extension ThemeFile {
    /// A theme this build carries, as the file this reads.
    ///
    /// The shape is `poc/ui/styles.css`'s, so a line lifted out of the stylesheet lands here
    /// unchanged: `:root` holds the dark colours with the type scale and the geometry, `:root.light`
    /// holds the light ones. Complete on purpose — that is what makes "a theme is drawn entire" an
    /// affordable rule rather than an hour of typing.
    public static func text(for theme: Theme) -> String {
        var out = """
            /* A chat-search theme, written out of the \(theme.name) direction.
             *
             * Edit and relaunch: this file is read at launch and drawn as authored. It is a user
             * theme, so it is measured by the same code that fences what this project ships and
             * drawn whatever those readings say — `chat-search --theme-file FILE --verify-theme`
             * prints them (docs/DECISIONS.md ADR 25).
             *
             * Every token below has to stay. A file with a hole in it is not a theme: the app draws
             * the direction it ships with instead, and says which token was missing.
             */

            :root {

            """
        out += group("the dark side", ColorToken.allCases) { theme.dark[$0].hex }
        out += "\n" + group("the type scale", SizeToken.allCases) { px(theme.size($0)) }
        out +=
            "\n"
            + group("the faces — the generic family is the part that survives", FaceToken.allCases) {
                token in designs.first { $0.value == theme.type.design(token) }?.key ?? "sans-serif"
            }
        let radii = MetricToken.allCases.filter { $0.rawValue.hasPrefix("--r-") }
        out += "\n" + group("the radii", radii) { px(theme.metric($0)) }
        out +=
            "\n"
            + group(
                "the row's rhythm and the ribbon's geometry",
                MetricToken.allCases.filter { !radii.contains($0) }
            ) { px(theme.metric($0)) }
        out += "}\n\n:root.light {\n"
        out += group("the light side", ColorToken.allCases) { theme.light[$0].hex }
        out += "}\n"
        return out
    }

    private static func group<Token: RawRepresentable>(
        _ heading: String, _ tokens: [Token], _ value: (Token) -> String
    ) -> String where Token.RawValue == String {
        var out = "  /* \(heading) */\n"
        let width = (tokens.map(\.rawValue.count).max() ?? 0) + 2
        for token in tokens {
            let name = (token.rawValue + ":").padding(toLength: width, withPad: " ", startingAt: 0)
            out += "  \(name)\(value(token));\n"
        }
        return out
    }

    /// A length as CSS writes it, and as this reads it back: `12px`, `12.5px`.
    private static func px(_ value: CGFloat) -> String {
        value == value.rounded() ? "\(Int(value))px" : "\(value)px"
    }
}

extension StringProtocol {
    fileprivate var trimmed: String { trimmingCharacters(in: .whitespacesAndNewlines) }
}
