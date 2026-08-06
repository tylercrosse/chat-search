import CsKit
import CsTheme

/// How much of each kind of message the reader is shown — `poc/ui`'s fidelity model, ported.
///
/// The bead this came from is a port and not a design, because the iteration already happened:
/// `poc/ui/NOTES.md` §"the drawer" carries five rounds of this control with the rejected
/// arrangements written down, and every piece below is one of those decisions rather than a fresh
/// one. What is new here is only the spelling.
///
/// Four knobs over three levels. The four are exactly `cs_core::blocks::Band`, which is the
/// vocabulary the wire already speaks and the one `ThemeCheck` already fences on a luminance ramp
/// — so this control names the same four things the spine beside the text and the band on the
/// minimap name, and nobody has to learn a fifth categorisation to use it.

/// One band's control. A knob rather than a `Band` because the two are not the same set: `Band`
/// has an arm for a name this build has no reading for, and a control that offered a knob for it
/// would be offering to hide messages it cannot describe.
enum Knob: String, CaseIterable, Identifiable, Sendable {
    case user
    case agent
    case reasoning
    case tool

    var id: String { rawValue }

    var band: Band {
        switch self {
        case .user: .user
        case .agent: .agent
        case .reasoning: .reasoning
        case .tool: .tool
        }
    }

    /// The knob for a band, or nil for a band no knob names.
    init?(_ band: Band) {
        switch band {
        case .user: self = .user
        case .agent: self = .agent
        case .reasoning: self = .reasoning
        case .tool: self = .tool
        case .unrecognised: return nil
        }
    }

    /// `poc/ui`'s labels, which are not the band names. `you` rather than `user` because the chip
    /// is addressed to the person reading it, and `tools` plural because the knob governs a run
    /// and not a call.
    var label: String {
        switch self {
        case .user: "you"
        case .agent: "agent"
        case .reasoning: "reasoning"
        case .tool: "tools"
        }
    }
}

/// How much of a message in that band is drawn.
///
/// Three levels where the wire has two. `hidden` is a client concept and stays one: `cs_core`
/// answers how much of a message to show and deliberately refuses to hold what a reader has done
/// with the answer, because that is session state rather than a property of the conversation. The
/// names are TUI-DESIGN §8's; the words on the chip are shorter, for the reason the prototype
/// gives — "collapsed" does not fit a 10px chip beside its label and "off" says it faster.
enum Level: String, CaseIterable, Sendable {
    case hidden
    case collapsed
    case expanded

    /// What the chip says.
    var word: String {
        switch self {
        case .hidden: "off"
        case .collapsed: "brief"
        case .expanded: "full"
        }
    }

    /// The wire's two-valued answer, or nil for the level the wire has no name for.
    var fold: Fold? {
        switch self {
        case .hidden: nil
        case .collapsed: .collapsed
        case .expanded: .expanded
        }
    }

    init(_ fold: Fold) {
        self = fold == .expanded ? .expanded : .collapsed
    }

    /// off → brief → full → off, which is the path a reader actually walks: peek at the tools,
    /// read them, put them away. `poc/ui/NOTES.md` §3 #22 records that the earlier decision here
    /// optimised the rare transition instead.
    func cycled(back: Bool = false) -> Level {
        let order = Level.allCases
        let index = order.firstIndex(of: self) ?? 0
        return order[(index + (back ? order.count - 1 : 1)) % order.count]
    }
}

/// One level per knob — the whole of what the reader has asked for.
struct Fidelity: Equatable, Sendable {
    var user: Level
    var agent: Level
    var reasoning: Level
    var tool: Level

    subscript(knob: Knob) -> Level {
        get {
            switch knob {
            case .user: user
            case .agent: agent
            case .reasoning: reasoning
            case .tool: tool
            }
        }
        set {
            switch knob {
            case .user: user = newValue
            case .agent: agent = newValue
            case .reasoning: reasoning = newValue
            case .tool: tool = newValue
            }
        }
    }

    /// The level a message in this band is drawn at.
    ///
    /// A band no knob names is never hidden and is drawn `collapsed`. Silently folding it into one
    /// of the four would let a control that does not name a kind of message decide not to show it,
    /// which is the same over-claim `Display.bandToken` refuses when it draws an unknown band in
    /// the quiet tier rather than borrowing one of the four hues.
    func level(of band: Band) -> Level {
        guard let knob = Knob(band) else { return .collapsed }
        return self[knob]
    }

    /// Whether the transcript summarises runs rather than listing them.
    ///
    /// `app.js:1230` — "segments mode is what tools hidden, agent collapsed means in practice: the
    /// run is summarised rather than listed". It is the tool knob alone that decides, because
    /// hiding the calls is what leaves nothing in their place; the agent knob only decides how
    /// much of the prose between the summaries is drawn.
    var summarisesRuns: Bool { tool == .hidden }

    /// Four presets, and there are four rather than the six this replaced for a reason worth
    /// keeping: `poc/ui/NOTES.md` records that of three presets plus `expand all` and
    /// `collapse all`, two were the same command — `outline` set every kind to collapsed and so
    /// did `collapse all` — while `full` was *not* full, since it left reasoning and tools
    /// collapsed, and `expand all` was. Naming a preset for what it does and letting `everything`
    /// mean everything removes both the duplicate and the lie.
    enum Preset: String, CaseIterable, Identifiable, Sendable {
        case segments
        case outline
        case read
        case everything

        var id: String { rawValue }

        var fidelity: Fidelity {
            switch self {
            // The prototype's opening state, and not this app's — see `Fidelity.opening`.
            case .segments:
                Fidelity(user: .expanded, agent: .collapsed, reasoning: .hidden, tool: .hidden)
            case .outline:
                Fidelity(user: .collapsed, agent: .collapsed, reasoning: .collapsed, tool: .collapsed)
            case .read:
                Fidelity(user: .expanded, agent: .expanded, reasoning: .collapsed, tool: .collapsed)
            case .everything:
                Fidelity(user: .expanded, agent: .expanded, reasoning: .expanded, tool: .expanded)
            }
        }
    }

    /// Which preset these knobs match, or nil — which the control draws as `custom`.
    var preset: Preset? { Preset.allCases.first { $0.fidelity == self } }

    /// What the drawer opens at, and it is `read` because that is what the wire already says.
    ///
    /// Two of these four presets are `cs_core::blocks::Density`'s two named points written out in
    /// this language: `read` is `Density::Full` and `outline` is `Density::Outline`. Only the
    /// first of them is on the wire — `cs show --json` puts `Density::Full.default_fold(band)` on
    /// every block — so opening at `read` is opening at the answer core gave, and `--shot` checks
    /// the two against each other rather than leaving the agreement to a comment. The other two
    /// presets have no counterpart there and cannot: `everything` needs a level `Density` has no
    /// reason to name, and `segments` needs `hidden`, which is not a fold at all.
    ///
    /// The prototype opens at `segments` instead and then corrects per conversation, and the
    /// correction is the one piece of this model that is deliberately not ported: its prose > 0.5
    /// test is ~87% predicted by the source badge already on the row — chatgpt 0.88, claude-code
    /// 0.30, codex 0.23 — so it is close to a source rule wearing a content costume, and it is
    /// broken in the prototype besides. Without it, `segments` on a 7-message ChatGPT thread draws
    /// a summary line after every single turn, which is what `poc/ui/NOTES.md` means by "the
    /// segment fold is for agentic runs and actively harms conversational ones". So the drawer
    /// opens where the wire points and `segments` is one click away.
    static let opening = Preset.read.fidelity
}
