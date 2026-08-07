import CsKit
import CsTheme
import SwiftUI

/// One line under the search bar saying what came back: how many, how settled, what it cost, and —
/// while the scrubber under it is open — what the picture below is of.
///
/// **`poc/ui`'s `.fbar`, moved to the top of the window with the scrubber's own header folded into
/// it** (`chat-search-me9.8.31`). The 2026-08-06 markup asked for the move on layout grounds — "the
/// # of results + timing info should be directly under the search bar" — and the merge is what the
/// move turned up: this window was printing the same count twice, six hundred points apart, and the
/// two copies disagreed. `58 of 58 · 334 ms` sat in the footer while `3,617 in range · 58 the query
/// selects` sat above the scrubber (`TimelineDrawer`, which is still what the type is called), from
/// two replies that arrive separately and can land in either order.
///
/// **They disagreed by nearly four times, and on the ordinary keystroke rather than a rare one.**
/// The list's total is a prefix search's, which is a floor on almost every keystroke; the
/// scrubber's is `cs timeline`'s, which is always whole. Typing `the` against this archive, both on
/// screen at once: `897+` in the footer against `3,549 the query selects` below it, and 3,549 is
/// exactly what a settled `cs search "the"` returns. Two numbers, one fact.
///
/// The one that survives is the list's, ranged — `Shell`'s rule before this bead moved it up here:
/// `total` is a number only when `settled` and a floor otherwise, so it is printed with a `+` and
/// not as a fact. **The scrubber's settled total is deliberately not borrowed for it.** It is the
/// better number and reading it would make the count exact whenever the scrubber happens to be open
/// and vague whenever it is shut, which is a count that appears to move when a region is closed —
/// and the scrubber is a third `cs` per keystroke that exists to be closable.
struct StatusStrip: View {
    @Bindable var model: SearchModel
    @Environment(\.theme) private var theme

    /// The whole line is measured, not the clause that shrinks — and that is the one thing here
    /// that is not obvious from reading it.
    ///
    /// **A `ViewThatFits` around a cluster inside an `HStack` elides early**, which the first
    /// version of this strip did: `· 4 undated` disappeared at 720 pt with 200 points of empty line
    /// to its right. What that `ViewThatFits` gets proposed is not the room on the line, it is the
    /// share an `HStack` hands a flexible child once the `Spacer` beside it has been counted as a
    /// claimant too, and no arrangement of layout priorities moved it. Three whole candidate lines
    /// measure the thing that actually has to fit: an `HStack`'s ideal width is the sum of its
    /// children's, a `Spacer`'s ideal width is its `minLength`, so the ideal of a candidate is the
    /// line at its tightest and the first one under the window's width is the true answer.
    ///
    /// What goes, in order. `undated` first: the number is small by construction — four
    /// conversations here — and it is a footnote about the axis rather than about the answer.
    /// Then `in sqlite`, because a round trip nobody can attribute is still a round trip, where a
    /// server reading with no total beside it is a number with no unit. Nothing else is ever
    /// dropped, and every clause carries its sentence on hover in whichever line won.
    var body: some View {
        ViewThatFits(in: .horizontal) {
            line(undated: true, serverMs: true)
            line(undated: false, serverMs: true)
            line(undated: false, serverMs: false)
        }
        // Monospaced at `--fs-meta`, which is `.fbar`'s size and the mockup's rule for everything
        // tabular: a number that changes every keystroke has to stop the line reflowing while you
        // read it.
        .font(theme.font(.meta, .mono))
        .foregroundStyle(theme.color(.ink3))
        .lineLimit(1)
        .padding(.horizontal, 12)
        .padding(.vertical, 5)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(theme.color(.panel))
    }

    private func line(undated: Bool, serverMs: Bool) -> some View {
        HStack(spacing: 8) {
            counts
            range(undated: undated)
            // How many groups are shut. It sits beside the count rather than replacing it — a
            // folded group is still a group, and a screen of eleven heads has to be able to say
            // whether it is eleven groups or eleven groups with nine of them shut. The group key
            // line above the list is where this belongs by subject and it has no room for it; see
            // `SearchView.groupKey`.
            if model.foldedGroups > 0 {
                Text(verbatim: "· \(model.foldedGroups) folded")
                    .fixedSize()
                    .help("shut, and still counted in the group key above the list")
            }
            // What Enter would do where the cursor is, and only when that is not what Enter has
            // always done. The prototype drew `→` here and wired nothing (`poc/ui/NOTES.md` §5);
            // wiring a key and drawing nothing is the same defect from the other side.
            if let hint = model.cursorHint {
                Text(verbatim: "· \(hint)").foregroundStyle(theme.color(.ink2)).fixedSize()
            }
            Spacer(minLength: 8)
            // Kept because it costs nothing: `cs` measures its own query time and the client
            // already has a clock around the spawn, so the two numbers are decoded rather than
            // taken. A latency you cannot see while you type is a latency you will argue about.
            if let t = model.lastTiming { timing(t, serverMs: serverMs) }
            // The scrubber's toggle, which had to come with its header. It is on this line rather
            // than inside the region it opens because a shut scrubber now draws nothing at all: the
            // old closed state was a whole strip whose only job was to hold the way back in, and
            // that is 20 points of the 480 pt floor spent on one button.
            button(model.timeline.shown ? "hide timeline" : "show timeline") {
                model.toggleTimeline()
            }
        }
    }

    /// The answer: rows in hand, of everything the query selects.
    ///
    /// Saying "897 matches" when the answer is 3,549 is the one thing this field exists to stop a
    /// client doing, so an unsettled total is ranged rather than printed.
    private var counts: some View {
        Text(
            "\(model.conversations.count) of "
                + (model.settled ? "\(model.total)" : "\(model.total)+"))
            .foregroundStyle(theme.color(.ink2))
            .fixedSize()
            .help(
                model.settled
                    ? "rows in hand, of every conversation the query selects with --limit ignored"
                    : "rows in hand, of a floor and not a count — a prefix search stops early, so "
                        + "the whole number is this or more")
    }

    /// What the picture under this line is of, in numbers the picture itself cannot state.
    ///
    /// **Only while there is a picture.** A shut scrubber asks `cs timeline` for nothing, by
    /// design, so `TimelineModel.drawn` goes on holding whatever the last open one left — and a
    /// count standing next to a count from a query somebody typed past is the defect this whole
    /// strip was filed to remove, not one to reintroduce at the other end.
    @ViewBuilder
    private func range(undated: Bool) -> some View {
        if model.timeline.shown, let drawn = model.timeline.drawn {
            HStack(spacing: 6) {
                // A separator and not a wider gap. Two numbers a space apart read as one number
                // with a space in it — `58 of 58 3617 in range` — which is what the first frame of
                // this strip actually said.
                Text(verbatim: "·")
                Text(verbatim: "\(drawn.inRange)").foregroundStyle(theme.color(.ink2))
                Text("in range")
                // The window as a reader would have typed it, which is `cs_core`'s rendering and
                // not this app's: deriving a local day in a client is the shape of the bug
                // `cs_core::time` exists to have fixed once.
                Text(verbatim: "· \(drawn.window?.value ?? "all time")")
                if undated, drawn.undated > 0 {
                    Text(verbatim: "· \(drawn.undated) undated")
                }
            }
            .fixedSize()
            .help(
                "how much of the archive is inside the drawn window, free text ignored"
                    + (drawn.undated > 0
                        ? " · \(drawn.undated) have no ending, so no bar can hold them" : ""))
        }
    }

    /// The round trip, and `cs`'s own reading of the query inside it.
    private func timing(_ t: Timing, serverMs: Bool) -> some View {
        HStack(spacing: 12) {
            Text(String(format: "%.0f ms", t.total))
            if serverMs { Text(String(format: "%.1f in sqlite", t.serverMs)) }
        }
        .fixedSize()
        .help("the whole spawn, and what cs says the query itself took inside it")
    }

    /// `.tld-btn`: monospaced micro in a hairline box. It kept the scrubber's size rather than
    /// taking this line's, because it is the same control it always was and a boxed label the size
    /// of the text beside it reads as a word somebody boxed.
    private func button(_ label: String, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Text(label)
                .font(theme.font(.micro, .mono))
                .foregroundStyle(theme.color(.ink3))
                .padding(.horizontal, 7)
                .padding(.vertical, 2)
                .overlay(
                    RoundedRectangle(cornerRadius: theme.metric(.r2))
                        .strokeBorder(theme.color(.rule2)))
                .contentShape(Rectangle())
        }
        // `.plain`, because a stock button paints itself in the system accent — a colour chosen
        // somewhere this app cannot see.
        .buttonStyle(.plain)
        .fixedSize()
    }
}
