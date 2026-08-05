import AppKit
import SwiftUI

/// The seam a visual direction is poured into.
///
/// `poc/ui/styles.css` pulled its type scale, radii and row rhythm out of the rules "so that a
/// *direction* can be a set of token values rather than a fork of this file", and this is the
/// same seam on the Swift side. It exists before the views that would otherwise hardcode it
/// (`chat-search-me9.8.2` onward all write views) because a token layer added after the views is
/// a rewrite of the views.
///
/// Nothing here chooses a value. Every token below already had a solved answer in both themes
/// before this file existed; `Tokens.swift` is generated from the stylesheet that holds them, so
/// the two cannot drift and neither one is a transcription of the other.
///
/// ## Why not `@Entry`
///
/// `@Entry` is the idiom in Apple's own documentation and in nearly everything written since
/// 2024, and it does not build here: the macro is implemented by `SwiftUIMacros`, a plugin that
/// ships with Xcode, and this package builds against the Command Line Tools SDK on purpose. The
/// explicit `EnvironmentKey` conformance below is the same shape four lines longer, and it
/// compiles clean in Swift 6 language mode. See `chat-search-me9.8.8`.
public struct Theme: Sendable {
    /// The direction this is. Carried so a screen, a log line or a bug report can say which set
    /// of values it was looking at.
    public let name: String

    /// The two themes as authored, kept alongside the resolved colours rather than thrown away.
    ///
    /// They are two *complete and independently solved* sets, not one derived from the other:
    /// on a light track contrast is distance from white, so the light ramp is reached by going
    /// darker, and `poc/ui/palette.py` solves each against whichever ground is worse for it.
    /// A theme that computed light from dark would not reproduce that. Keeping both also lets
    /// `ThemeCheck` re-measure what shipped rather than what was intended.
    public let dark: Palette
    public let light: Palette

    public let type: TypeScale
    public let geometry: Geometry

    /// Each colour token as one dynamic colour that picks its own side of the pair.
    ///
    /// Resolved once at construction rather than per access: `NSColor(name:dynamicProvider:)`
    /// allocates, and a row redrawing at 60 Hz asks for a dozen tokens.
    private let resolved: [ColorToken: NSColor]

    public init(name: String, dark: Palette, light: Palette, type: TypeScale, geometry: Geometry) {
        self.name = name
        self.dark = dark
        self.light = light
        self.type = type
        self.geometry = geometry
        self.resolved = Dictionary(
            uniqueKeysWithValues: ColorToken.allCases.map {
                ($0, Theme.dynamic(light: light[$0], dark: dark[$0]))
            })
    }

    /// The token, as a colour that already knows which appearance it is being drawn in.
    ///
    /// This is what keeps `colorScheme` out of every view: a dynamic `NSColor` carries both
    /// authored values and lets AppKit choose, so a view asks for `.ink` and never asks which
    /// theme it is in.
    public func color(_ token: ColorToken) -> Color { Color(nsColor: resolved[token]!) }

    /// The same token for the AppKit surfaces SwiftUI does not own — the window's own ground,
    /// which shows through for a frame during a live resize.
    public func nsColor(_ token: ColorToken) -> NSColor { resolved[token]! }

    /// One of the five sizes in the scale, in points.
    public func size(_ token: SizeToken) -> CGFloat { type.size(token) }

    /// A radius or a row measurement, in points.
    public func metric(_ token: MetricToken) -> CGFloat { geometry[token] }

    /// A size from the scale, set in one of the three faces.
    public func font(_ size: SizeToken, _ face: FaceToken = .ui, weight: Font.Weight = .regular)
        -> Font
    {
        .system(size: type.size(size), weight: weight, design: type.design(face))
    }

    /// A colour that resolves itself against the appearance it is drawn in.
    ///
    /// `bestMatch(from:)` rather than reading `NSApp.effectiveAppearance`: the provider is called
    /// with the appearance in force where the colour is being drawn, which is the only reading
    /// that stays right inside a view that has overridden it.
    public static func dynamic(light: RGB, dark: RGB) -> NSColor {
        let lightColor = light.nsColor
        let darkColor = dark.nsColor
        return NSColor(name: nil) { appearance in
            appearance.bestMatch(from: [.aqua, .darkAqua]) == .darkAqua ? darkColor : lightColor
        }
    }
}

// MARK: - Tokens

/// A colour token, named as the CSS custom property it was authored as.
///
/// The raw values are the property names on purpose: they are what `poc/ui/styles.css`,
/// `poc/ui/directions.css` and `poc/ui/palette.py` all call these, and a check that prints
/// `--ink-3` can be read against any of them without a translation table.
///
/// What is *not* here is as argued as what is. Radii below 3px, the 50% dots and the 10–11px
/// pills stay literal in the views for the reason `styles.css` gives: squaring a dot is not a
/// visual direction, it is a bug. A number does not become a token by being a number.
public enum ColorToken: String, CaseIterable, Sendable {
    case bg = "--bg"
    case panel = "--panel"
    case panel2 = "--panel-2"
    case panel3 = "--panel-3"
    case ink = "--ink"
    case ink2 = "--ink-2"
    case ink3 = "--ink-3"
    case rule = "--rule"
    case rule2 = "--rule-2"
    case sel = "--sel"
    case selBg = "--sel-bg"
    case selInk = "--sel-ink"
    case hit = "--hit"
    case hitBg = "--hit-bg"
    case err = "--err"
    case ok = "--ok"

    /// The kind ramp: four categories on an even luminance ramp against `mapBg`, because hue is
    /// the channel that degrades fastest at the ~2px the bands are drawn at. `ThemeCheck` holds
    /// the ratios; `poc/ui/NOTES.md` §2 is the argument.
    case kUser = "--k-user"
    case kAgent = "--k-agent"
    case kReason = "--k-reason"
    case kTool = "--k-tool"

    /// Sub-shades inside the tool band, by what the call was for. Deliberately narrow, so the
    /// primary read — tool against prose against reasoning — stays on the main ramp.
    case actLook = "--act-look"
    case actRun = "--act-run"
    case actChange = "--act-change"

    case kToolInk = "--k-tool-ink"
    /// The ground the kind ramp is measured against.
    case mapBg = "--map-bg"

    /// One hue per source. Carried by the token layer before anything draws them, so the row's
    /// anatomy (`chat-search-me9.8.2`) finds them solved rather than picking five more.
    case srcClaudeCode = "--src-claude-code"
    case srcCodex = "--src-codex"
    case srcChatgpt = "--src-chatgpt"
    case srcClaudeAi = "--src-claude-ai"
    case srcGemini = "--src-gemini"
}

/// The type scale. Five sizes, and no sixth: a view that needs a size not on this list is either
/// wrong or is asking for a scale change, and both are worth noticing.
public enum SizeToken: String, CaseIterable, Sendable {
    /// Pane titles, and the preview's title.
    case head = "--fs-head"
    /// Body text and row titles.
    case body = "--fs-body"
    /// The best-match line, and prose in the drawer.
    case sub = "--fs-sub"
    /// Everything tabular, which in practice means everything monospaced.
    case meta = "--fs-meta"
    /// Keys, section labels, the topics line.
    case micro = "--fs-micro"
}

/// The three faces.
///
/// Each resolves to a `Font.Design` rather than to a family name, because that is what the CSS
/// stacks resolve to on this platform anyway: `-apple-system` *is* `.default` and `ui-monospace`
/// *is* `.monospaced`. A direction that wants a specific face rather than a generic one — `paper`
/// asks for Iowan Old Style — currently lands on `.serif`, which is the right generic and not the
/// named face. Widening `Face` to carry a family is a direction's problem, not the seam's, and it
/// is one field.
public enum FaceToken: String, CaseIterable, Sendable {
    case ui = "--ui"
    case mono = "--mono"
    case serif = "--serif"
}

/// Radii, the row's rhythm, and the ribbon's geometry — everything a visual pass gets to touch
/// that is not a colour or a size.
public enum MetricToken: String, CaseIterable, Sendable {
    case r1 = "--r-1"
    case r2 = "--r-2"
    case r3 = "--r-3"
    case r4 = "--r-4"
    case r5 = "--r-5"
    case r6 = "--r-6"

    /// The row's rhythm. The tokens a direction is most likely to get wrong: every one of them
    /// trades against rows-per-screen, which `poc/ui/DESIGN-BRIEF.md` names first on its list of
    /// what would actually break this.
    case rowPaddingX = "--row-px"
    case rowPaddingTop = "--row-pt"
    case rowPaddingBottom = "--row-pb"
    /// Between the row's stacked lines.
    case rowLead = "--row-lead"
    /// Between cells on the meta line.
    case rowGap = "--row-gap"

    /// The ribbon. `ribbonTop` is not `(ribbonHeight - ribbonTrack) / 2` and the track is not
    /// centred: the bottom of the cell belongs to the failure and question marks and the top to
    /// the match ticks, both drawn outside the track. A direction that thickens the track has to
    /// take that from one end or the other.
    case ribbonHeight = "--rib-h"
    case ribbonTrack = "--rib-track"
    case ribbonTop = "--rib-top"
}

// MARK: - Token sets

/// One theme's colours, complete.
///
/// Incomplete is a programmer error rather than a runtime condition — the values are generated
/// from a stylesheet in one pass, so a gap means the generator and this enum disagree about what
/// a token is, which no amount of fallback colour makes better.
public struct Palette: Sendable {
    private let values: [ColorToken: RGB]

    public init(_ values: [ColorToken: RGB]) {
        let missing = ColorToken.allCases.filter { values[$0] == nil }
        precondition(
            missing.isEmpty,
            "palette is missing \(missing.map(\.rawValue).joined(separator: ", ")) — "
                + "regenerate Tokens.swift with poc/ui/tokens.py")
        self.values = values
    }

    public subscript(_ token: ColorToken) -> RGB { values[token]! }
}

/// The five sizes and the three faces.
public struct TypeScale: Sendable {
    private let sizes: [SizeToken: CGFloat]
    private let designs: [FaceToken: Font.Design]

    public init(sizes: [SizeToken: CGFloat], faces: [FaceToken: Font.Design]) {
        let missing = SizeToken.allCases.filter { sizes[$0] == nil }.map(\.rawValue)
            + FaceToken.allCases.filter { faces[$0] == nil }.map(\.rawValue)
        precondition(
            missing.isEmpty,
            "type scale is missing \(missing.joined(separator: ", ")) — "
                + "regenerate Tokens.swift with poc/ui/tokens.py")
        self.sizes = sizes
        self.designs = faces
    }

    public func size(_ token: SizeToken) -> CGFloat { sizes[token]! }
    public func design(_ token: FaceToken) -> Font.Design { designs[token]! }
}

/// Radii, the row's rhythm and the ribbon, in points.
///
/// CSS pixels map to points one for one. The prototype "renders in the system UI face at native
/// metrics on purpose" — set a native app mock in a display face and it stops telling the truth
/// about density — so its numbers are already native numbers.
public struct Geometry: Sendable {
    private let values: [MetricToken: CGFloat]

    public init(_ values: [MetricToken: CGFloat]) {
        let missing = MetricToken.allCases.filter { values[$0] == nil }
        precondition(
            missing.isEmpty,
            "geometry is missing \(missing.map(\.rawValue).joined(separator: ", ")) — "
                + "regenerate Tokens.swift with poc/ui/tokens.py")
        self.values = values
    }

    public subscript(_ token: MetricToken) -> CGFloat { values[token]! }
}

/// An authored colour, kept as the eight-bit sRGB it was solved and published as.
///
/// Not stored as a `Color`: the contrast measurements that fence half these tokens are defined on
/// sRGB bytes, and a value that has been through a colour space is a value those measurements no
/// longer describe. `palette.py` steps its solve until the *rounded* colour clears the target for
/// the same reason.
public struct RGB: Sendable, Equatable {
    public let red: UInt8
    public let green: UInt8
    public let blue: UInt8

    public init(_ hex: UInt32) {
        red = UInt8((hex >> 16) & 0xff)
        green = UInt8((hex >> 8) & 0xff)
        blue = UInt8(hex & 0xff)
    }

    public var nsColor: NSColor {
        NSColor(
            srgbRed: CGFloat(red) / 255, green: CGFloat(green) / 255, blue: CGFloat(blue) / 255,
            alpha: 1)
    }

    public var hex: String { String(format: "#%02x%02x%02x", red, green, blue) }

    /// Relative luminance, WCAG 2.x. The same arithmetic as `palette.py`, which is the point:
    /// a ratio measured here has to be the ratio that file published.
    public var luminance: Double {
        func linear(_ channel: UInt8) -> Double {
            let c = Double(channel) / 255
            return c <= 0.04045 ? c / 12.92 : pow((c + 0.055) / 1.055, 2.4)
        }
        return 0.2126 * linear(red) + 0.7152 * linear(green) + 0.0722 * linear(blue)
    }

    public func contrast(against other: RGB) -> Double {
        let a = luminance, b = other.luminance
        return (max(a, b) + 0.05) / (min(a, b) + 0.05)
    }
}

// MARK: - The environment

private struct ThemeKey: EnvironmentKey {
    /// `shipped` and not a direction by name: the name of the direction in force lives in the
    /// generated `Tokens.swift` and nowhere else, so regenerating for another one does not stop
    /// this line compiling. A default that named `terminal` would make every regeneration a view
    /// edit, which is the lift this seam exists to prevent.
    static let defaultValue = Theme.shipped
}

extension EnvironmentValues {
    /// Read by every view that draws anything: `@Environment(\.theme) private var theme`.
    ///
    /// A default rather than an optional, because the alternative is every view carrying a
    /// fallback and each fallback being a colour somebody chose in a view.
    public var theme: Theme {
        get { self[ThemeKey.self] }
        set { self[ThemeKey.self] = newValue }
    }
}
