import AppKit
import CsKit
import CsTheme
import SwiftUI

/// The four knobs and the four presets, drawn as `poc/ui` draws them.
///
/// The arrangement is the part that took five rounds in the prototype, so it is the part reproduced
/// most literally. Label, state and dot live in **one box** per band, because the 2×2 grid that
/// preceded it put `you`'s control nearer to `agent`'s *label* than `agent`'s own control was, and
/// proximity therefore pointed at the wrong thing on every read — `poc/ui/NOTES.md` records that as
/// most of why the control felt fiddly, rather than the cycling everyone blamed.
///
/// The state is a **word** and not a glyph. `○ ◐ ●` is a legend you have to learn, on an 18pt
/// target; `off / brief / full` is neither.
struct FidelityBar: View {
    let reader: ReaderModel
    @Environment(\.theme) private var theme

    var body: some View {
        VStack(alignment: .leading, spacing: 7) {
            presets
            chips
            if reader.overrideCount > 0 { clear }
        }
    }

    /// `.zoom`: one bordered group, the matching preset filled. `custom` appears only when none
    /// matches, and it is inert — it is a readout of where the knobs are, not a fifth preset.
    private var presets: some View {
        HStack(spacing: 0) {
            ForEach(Fidelity.Preset.allCases) { preset in
                let on = reader.fidelity.preset == preset
                Button { reader.apply(preset) } label: {
                    Text(preset.rawValue)
                        .font(theme.font(.micro, .mono, weight: on ? .semibold : .regular))
                        .foregroundStyle(theme.color(on ? .selInk : .ink3))
                        .padding(.horizontal, 9)
                        .padding(.vertical, 2)
                        .background(
                            RoundedRectangle(cornerRadius: theme.metric(.r2))
                                .fill(on ? theme.color(.sel) : .clear))
                        .contentShape(Rectangle())
                }
                // `.plain` for the reason every other button in this app is: a stock button paints
                // itself in the system accent, which is a colour this app cannot see.
                .buttonStyle(.plain)
                .help("\(preset.rawValue): \(Self.explains(preset))")
            }
            if reader.fidelity.preset == nil {
                Text("custom")
                    .font(theme.font(.micro, .mono, weight: .semibold))
                    .foregroundStyle(theme.color(.ink2))
                    .padding(.horizontal, 9)
                    .padding(.vertical, 2)
            }
        }
        .padding(1.5)
        .overlay(
            RoundedRectangle(cornerRadius: theme.metric(.r4))
                .strokeBorder(theme.color(.rule2)))
    }

    /// Two columns, because four chips in a row do not fit a 380pt drawer and four in a column is
    /// most of the header.
    private var chips: some View {
        Grid(horizontalSpacing: 8, verticalSpacing: 5) {
            GridRow {
                chip(.user)
                chip(.agent)
            }
            GridRow {
                chip(.reasoning)
                chip(.tool)
            }
        }
    }

    /// One band: a dot that hides it, and a body that says how much of it is drawn.
    ///
    /// **Two targets in one box, deliberately.** Visibility and detail are two axes — hiding a band
    /// is "is it on screen", brief against full is "how much of it" — and `poc/ui/NOTES.md` names
    /// conflating them as why every arrangement of this control felt wrong: with a single 3-cycle,
    /// one of the six transitions always costs two clicks.
    private func chip(_ knob: Knob) -> some View {
        let level = reader.fidelity[knob]
        let on = level != .hidden
        return HStack(spacing: 0) {
            Button { reader.toggleVisible(knob) } label: {
                RoundedRectangle(cornerRadius: 2)
                    .fill(theme.color(Display.bandToken(knob.band)))
                    .frame(width: 8, height: 8)
                    .opacity(on ? 1 : 0.28)
                    .frame(width: 22)
                    .frame(maxHeight: .infinity)
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .help("\(knob.label): \(on ? "hide" : "show")")
            .accessibilityLabel("\(knob.label): \(on ? "hide" : "show")")
            Rectangle().fill(theme.color(.rule2)).frame(width: 1)
            Button {
                // Shift reverses the cycle. Read off `NSEvent` at the moment of the click because
                // `modifierKeyAlternate` is macOS 15 and this app's floor is 14 — which is
                // `chat-search-me9.8.27`, and the day that moves this becomes the ordinary
                // spelling. A modifier nobody discovers costs nothing here: the cycle is three
                // long, so going back is going forward twice.
                reader.cycle(knob, back: NSEvent.modifierFlags.contains(.shift))
            } label: {
                HStack(spacing: 8) {
                    Text(knob.label)
                        .lineLimit(1)
                        .truncationMode(.tail)
                    Spacer(minLength: 0)
                    Text(level.word)
                        .font(theme.font(.micro, .mono, weight: on ? .semibold : .regular))
                        .foregroundStyle(theme.color(on ? .sel : .ink3))
                        .opacity(on ? 1 : 0.7)
                }
                .font(theme.font(.micro, .mono))
                .foregroundStyle(theme.color(on ? .ink2 : .ink3))
                .padding(.horizontal, 8)
                .padding(.vertical, 4)
                .frame(maxWidth: .infinity, alignment: .leading)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .help("\(knob.label): \(level.word) — click to cycle, shift-click to go back")
            .accessibilityLabel("\(knob.label): \(level.word)")
        }
        .background(on ? theme.color(.panel2) : .clear)
        .clipShape(RoundedRectangle(cornerRadius: theme.metric(.r4)))
        .overlay(
            RoundedRectangle(cornerRadius: theme.metric(.r4))
                .strokeBorder(theme.color(.rule2)))
        .fixedSize(horizontal: false, vertical: true)
    }

    /// `.fid-all`. Only there when there is something to undo, and it says how much — a reader who
    /// has opened nine messages by hand and then moved a knob needs to know why the knob appears
    /// not to have taken.
    private var clear: some View {
        Button { reader.clearOverrides() } label: {
            Text(
                "clear \(reader.overrideCount) per-message "
                    + "override\(reader.overrideCount == 1 ? "" : "s")")
                .font(theme.font(.micro, .mono))
                .foregroundStyle(theme.color(.hit))
                .padding(.horizontal, 9)
                .padding(.vertical, 2.5)
                .overlay(
                    RoundedRectangle(cornerRadius: theme.metric(.r3))
                        .strokeBorder(theme.color(.rule2)))
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }

    /// What each preset is for, in the tooltip. The names are short enough to be ambiguous —
    /// `outline` and `segments` both sound like "less" — and this is where the difference is said.
    private static func explains(_ preset: Fidelity.Preset) -> String {
        switch preset {
        case .segments: "runs summarised, so a long agent session reads as what you asked for"
        case .outline: "one line per message — the whole conversation as a map"
        case .read: "both sides of the prose in full, everything else brief. The wire's own default"
        case .everything: "all of it, tool arguments included"
        }
    }
}

/// A run, folded to the line that says what it did: `→ 12 calls · 2 failed · asked you 1×`.
///
/// The alternative is per-message toggling, which `poc/ui/NOTES.md` costs at "211 toggles on a
/// 211-message conversation". The dot on the right is the one thing this row must not leave out —
/// a closed run that contains the match has to say so, or the search that brought you here appears
/// to have found nothing.
struct SegmentRow: View {
    let segment: Segment
    let open: Bool
    let toggle: () -> Void
    @Environment(\.theme) private var theme

    var body: some View {
        Button(action: toggle) {
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                // The transcript's own 3pt spine, in the tool band's colour, so a summary sits in
                // the column the calls it replaces would have sat in.
                RoundedRectangle(cornerRadius: 2)
                    .fill(theme.color(.kTool))
                    .frame(width: 3)
                Text(open ? "▾" : "▸")
                    .font(theme.font(.micro, .mono))
                    .frame(width: 13, alignment: .leading)
                Text(line)
                    .lineLimit(1)
                    .truncationMode(.tail)
                Spacer(minLength: 4)
                if segment.marked {
                    Text("●").font(theme.font(.micro, .mono)).foregroundStyle(theme.color(.hit))
                }
            }
            .font(theme.font(.meta, .mono))
            .foregroundStyle(theme.color(.ink3))
            .padding(.top, 3)
            .padding(.bottom, 6)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .help(open ? "fold this run back up" : "open this run")
    }

    /// The parts joined, each in its own colour. An `AttributedString` rather than an `HStack` of
    /// `Text`s because the line has to truncate as one thing in a 380pt drawer, and a stack
    /// truncates whichever child runs out of room first.
    private var line: AttributedString {
        var out = AttributedString("→ ")
        out.foregroundColor = theme.color(.ink3)
        for (index, part) in segment.summary.enumerated() {
            if index > 0 {
                var dot = AttributedString(" · ")
                dot.foregroundColor = theme.color(.ink3)
                out.append(dot)
            }
            var piece = AttributedString(part.text)
            piece.foregroundColor = theme.color(part.tone)
            out.append(piece)
        }
        return out
    }
}
