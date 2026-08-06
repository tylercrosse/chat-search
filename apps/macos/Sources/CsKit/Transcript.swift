import Foundation

// The `cs show --json` body, decoded — the second client contract ADR 12 published, and the only
// way a client that is not Rust can draw a conversation.
//
// Written against what the payload says rather than against the Rust structs behind it, for the
// same reason `Model.swift` is: a decoder derived from the producer proves nothing about the
// contract, because it moves whenever the producer does. `docs/JSON-CONTRACT.md` does not
// describe this half yet (`chat-search-me9.34`), so the field notes below are where the reading
// this client shipped against is written down.
//
// **Nothing here re-derives a rule.** Whether a message is drawn, which band it sits in, how it
// folds by default, and whether a match may claim to be why the conversation ranked are all
// answered on the wire by `cs_core::blocks`, which is the module that exists because the first
// two of those had already been worked out twice — once in Rust and once in the prototype's
// JavaScript. A third copy here would be the same mistake with a new language on it.

/// One conversation, in reading order: main thread first, each strand contiguous.
public struct Transcript: Decodable, Sendable {
    /// Contract version, moving only when a field changes meaning. Shares its numbering with the
    /// search envelope's `v` and is not the same contract; both are 1 today.
    public let v: Int
    /// The conversation this is a transcript of — **not always the id that was asked for**. Ask
    /// for any member of a sitting and this names the record that opened it, because the answer
    /// is the whole sitting and echoing the argument back would name one record of it as though
    /// it were all of it.
    public let convId: String
    /// What the blocks were marked against, after the query grammar had its say. Carried so a
    /// client can say *why* something is highlighted without parsing the query a second time.
    public let terms: [String]
    /// Set when these messages are several activity-log records read back as one chat — the only
    /// nullable field in this payload, and null for every conversation that is one conversation,
    /// which is the whole corpus bar the folded Takeout records.
    public let sitting: Sitting?
    /// Distinct strands (ADR 4). More than one means this conversation is a DAG.
    public let threads: Int
    /// Messages on the head path — the length of `messages`.
    public let count: Int
    /// How many of those a reader draws. The difference is successful tool results, which travel
    /// flagged rather than omitted so a client can offer to show one without a second request.
    public let drawn: Int
    /// How to read every `marks` below, read off the wire rather than assumed — the search
    /// envelope names its own the same way and for the same reason.
    public let markOffsets: MarkOffsets
    public let messages: [Block]

    /// Only what a reader draws, in order.
    ///
    /// Filtered on the wire's own answer rather than on `kind` and `isError`, which is the whole
    /// reason `drawn` is a field: the rule that a successful tool result is implied by its call
    /// and a failed one is recognition information lives in `cs_core::blocks` and is read here.
    public var drawnMessages: [Block] { messages.filter(\.drawn) }
}

/// Several activity-log records read as one chat, and what produced the fold.
///
/// Kept as data rather than drawn: those records were separate requests with no shared context
/// on Google's side, so the boundary is real information. A reader who does not care sees one
/// continuous conversation, which is the point.
public struct Sitting: Decodable, Sendable, Equatable {
    /// Every conversation folded into this answer, earliest first. Each is a real permanent id;
    /// the sitting itself never acquires one (ADR 16).
    public let members: [String]
    /// The silence that delimited it, in milliseconds.
    public let gapMs: Int
}

/// One message, with any matches in it already located and every rule about it already answered.
public struct Block: Decodable, Sendable, Identifiable {
    public let msgId: String
    public let role: String
    /// `prose` · `reasoning` · `tool_call` · `tool_result`. An open set, and one this client
    /// never switches on: `band` and `fold` are the two questions it would have been asked.
    public let kind: String
    /// Position within its own thread, not within the conversation. Ordering by it alone
    /// interleaves every strand at once, which is why the wire's order is the one to draw in.
    public let seq: Int
    /// Always true today — `cs show` returns the head path only — and carried because the
    /// off-path toggle is a thing §8 describes and nothing has built.
    public let onPath: Bool
    public let text: String
    /// A subagent strand, which runs parallel to the main one rather than being an abandoned
    /// branch. Part of what happened, but not the conversation you were having.
    public let isSidechain: Bool
    /// This result reports a failure, from the source's own signal rather than from its text.
    public let isError: Bool
    /// Which strand within the conversation. Opaque — never parsed.
    public let threadKey: String
    /// Where the query matched, in the units the transcript's `markOffsets` names. Offsets into
    /// `text` as emitted, so any transform a client applies before drawing has to preserve them
    /// byte for byte or drop them.
    public let marks: [Span]
    /// Whether a reader draws this message at all.
    public let drawn: Bool
    /// Which claim a match inside this message is entitled to make.
    public let markKind: MarkKind
    /// What this message is, at the resolution a conversation's *shape* is drawn at.
    public let band: Band
    /// How this message folds when the reader has not said otherwise. A default and not a state:
    /// an override belongs to the client, because which messages someone has opened is session
    /// state rather than a property of the conversation.
    public let fold: Fold

    public var id: String { msgId }
}

/// Which claim a match inside a block is entitled to make.
///
/// Not a colour — core states the claim and each client decides how loudly to say it. The TUI
/// spends a text modifier so a monochrome terminal can still tell them apart; this app spends a
/// different *form* rather than a different hue, for the same reason.
public enum MarkKind: Decodable, Sendable, Equatable {
    /// This word is among the reasons the conversation ranked.
    case ranked
    /// This word matched here but carries no postings, so `cs search` will not return the
    /// conversation for it. Reasoning is the kind that produces these.
    case unranked
    /// A claim this build has no reading for. Carried by name rather than folded into one of the
    /// two above, so an added value costs a line of prose rather than a coordinated release.
    case unrecognised(String)

    public init(from decoder: any Decoder) throws {
        switch try decoder.singleValueContainer().decode(String.self) {
        case "ranked": self = .ranked
        case "unranked": self = .unranked
        case let other: self = .unrecognised(other)
        }
    }

    /// Whether a mark here may be drawn as a reason the conversation is on screen.
    ///
    /// False for anything but `ranked`, including a claim this build cannot read. Over-claiming
    /// is the failure this field exists to prevent — marking an unrankable match exactly like a
    /// prose hit states something false in the one place a reader has gone to check — so an
    /// unknown value falls to the quieter of the two rather than the louder.
    public var claimsRanking: Bool { self == .ranked }
}

/// What a message is, at the resolution a conversation's shape is drawn at.
///
/// Four categories, and the same four `cs search --json` puts on a row's `kind_runs`. Coarser
/// than `kind` in one place — a call and its result are one stretch of traffic — and finer in
/// another, since prose splits by role. Both departures are what makes a 900-message agent
/// session legible as a handful of things you asked for.
public enum Band: Decodable, Sendable, Equatable {
    /// Prose from the person, and usually the reason for everything after it.
    case user
    /// Prose from anyone else — the assistant, and the handful of `system` messages.
    case agent
    /// Thinking. Drawn, never searched.
    case reasoning
    /// Calls and their results, failures included.
    case tool
    /// A band that postdates this build. Drawn in the quiet tier rather than guessed into one of
    /// the four, because a wrong stripe is a claim about what a conversation is made of.
    case unrecognised(String)

    public init(from decoder: any Decoder) throws {
        switch try decoder.singleValueContainer().decode(String.self) {
        case "user": self = .user
        case "agent": self = .agent
        case "reasoning": self = .reasoning
        case "tool": self = .tool
        case let other: self = .unrecognised(other)
        }
    }
}

/// How much of a message is shown.
public enum Fold: Decodable, Sendable, Equatable {
    case collapsed
    case expanded

    /// Anything but `expanded` reads as `collapsed`, which is the reading that cannot turn an
    /// added value into a screen of raw tool traffic. Nothing is lost either way: a collapsed
    /// message is still on screen and still one click from its full text.
    public init(from decoder: any Decoder) throws {
        self = try decoder.singleValueContainer().decode(String.self) == "expanded"
            ? .expanded : .collapsed
    }

    public var toggled: Fold { self == .expanded ? .collapsed : .expanded }
}
