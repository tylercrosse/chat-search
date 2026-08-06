import CsKit
import CsTheme
import SwiftUI

/// One conversation, folded: a header, and every drawn message on a coloured spine.
///
/// `poc/ui`'s drawer is the visual reference — a 3px gutter, a sigil column, and the text — and
/// what it decides is the structure and the density, never the pixel values. Every colour, size
/// and face below is a token off `\.theme` (`chat-search-me9.8.8`); the two literals are the 2px
/// gutter radius and the column widths the gutter and the sigil are drawn at, which the seam
/// deliberately leaves in the views: a number does not become a token by being a number, and
/// squaring a 3px stripe is a bug rather than a visual direction.
///
/// What it does *not* do is decide anything about the conversation. Which messages appear, which
/// spine each gets, how each folds and whether a match may look like a reason are four answers
/// that arrive on the wire from `cs_core::blocks`. This file turns them into a sigil and a
/// colour, which is the half core refuses to hold.
struct ReaderView: View {
    let conv: Conversation
    let reader: ReaderModel
    @Environment(\.theme) private var theme

    var body: some View {
        VStack(spacing: 0) {
            head
            Rectangle().fill(theme.color(.rule)).frame(height: 1)
            content
        }
        // The drawer is a panel beside the page rather than more page, which is the one thing
        // `--panel` exists to say.
        .background(theme.color(.panel))
    }

    private var head: some View {
        VStack(alignment: .leading, spacing: 5) {
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                Text(conv.title.flatMap { $0.isEmpty ? nil : $0 } ?? "untitled")
                    .font(theme.font(.head, weight: .semibold))
                    .foregroundStyle(theme.color(.ink))
                    .lineLimit(2)
                Spacer(minLength: 8)
                Button {
                    reader.close()
                } label: {
                    Image(systemName: "xmark")
                        .font(theme.font(.meta))
                        .foregroundStyle(theme.color(.ink3))
                }
                // `.plain` for the reason the shell's own button gives: a stock button paints
                // itself in the system accent, which is a colour this app cannot see.
                .buttonStyle(.plain)
                .help("close the reader")
            }
            Text(meta)
                .font(theme.font(.meta, .mono))
                .foregroundStyle(theme.color(.ink3))
            // The four knobs, under the facts and above the transcript they govern. In the header
            // rather than beside the messages for the reason the prototype gives: they are a
            // property of the whole reading, and a control that scrolled away with the text would
            // be reachable only from the top of a 2,553-message conversation.
            FidelityBar(reader: reader)
            if let sitting = reader.transcript?.sitting {
                // Said rather than drawn. Those records were separate requests with no shared
                // context on Google's side, so the fold is a reconstruction — and a reader who
                // notices the message count is larger than the row promised deserves the reason.
                Text(
                    "\(sitting.members.count) records read as one sitting — a reconstruction, "
                        + "not something the export recorded")
                    .font(theme.font(.micro))
                    .foregroundStyle(theme.color(.ink2))
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 10)
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    /// The row's own facts until the transcript lands, and the transcript's after.
    ///
    /// They are not the same facts. `msgCount` counts one conversation and a sitting's transcript
    /// counts every record folded into it, so showing the row's number beside a longer transcript
    /// would read as a miscount rather than as a fold.
    private var meta: String {
        var bits = [conv.source]
        if let project = conv.project { bits.append(project) }
        if let transcript = reader.transcript {
            bits.append("\(transcript.drawn) of \(transcript.count) drawn")
            // More than one strand means subagents ran beside the main thread, which is why some
            // messages below say so. Silent at one, which is almost every conversation.
            if transcript.threads > 1 { bits.append("\(transcript.threads) threads") }
        } else {
            bits.append("\(conv.msgCount) messages")
        }
        if let date = conv.endedDate { bits.append(date) }
        return bits.joined(separator: " · ")
    }

    @ViewBuilder
    private var content: some View {
        if let transcript = reader.transcript {
            if transcript.drawnMessages.isEmpty {
                // Not a failure. `cs show` exits nonzero for an id that does not exist, so
                // reaching here means the conversation is real and every message on its head
                // path is a successful tool result.
                notice("nothing on the head path to draw")
            } else if reader.rows.isEmpty {
                // A different sentence, because it has a different fix. The conversation has
                // messages and the four knobs have taken all of them off the screen, which is a
                // thing the controls above make easy to do by accident and which the empty column
                // beneath them would otherwise report as "there is nothing here".
                notice("every kind is switched off — turn one back on above")
            } else {
                // `List` for the reason the results pane uses one: it is the only container
                // `chat-search-me9.22` measured that recycles, and the longest conversation in
                // this corpus is 2,553 messages.
                ScrollViewReader { scroll in
                    HStack(spacing: 0) {
                        // The rows come off the model rather than off the transcript, because what
                        // the transcript draws is now a question with four knobs in it and a row
                        // may be a statement about a run rather than a message — see
                        // `ReaderModel.restack`.
                        List(reader.rows) { row in
                            switch row {
                            case .message(let block):
                                BlockRow(
                                    block: block, fold: reader.fold(of: block),
                                    marked: reader.marked(block, in: transcript, theme: theme),
                                    toggle: { reader.toggle(block) }
                                )
                                .listRowInsets(
                                    EdgeInsets(top: 0, leading: 12, bottom: 0, trailing: 12))
                                .listRowBackground(theme.color(.panel))
                                .listRowSeparator(.hidden)
                            // Half of what the minimap knows about where you are: where this row
                            // is, on every frame of a scroll. A row half off the top of the list
                            // reports it, which is what lets the box say "57% of the way into
                            // message 384" instead of naming the message and stopping there.
                            //
                            // The window's coordinate space rather than the list's own, and this
                            // is measured rather than preferred: both `.scrollView` and a
                            // `.coordinateSpace(.named:)` declared on the enclosing stack put the
                            // top row of a 352 pt viewport at y=150 — the chrome above the drawer
                            // — so neither of them resolves inside a `List`'s rows, and a row
                            // above the fold reports a positive `minY` under both. That is the
                            // old bug wearing a new coordinate space: the fraction is always
                            // zero, and the box goes back to moving a message at a time. So the
                            // rows and the list are measured in the one space they agree on, and
                            // the list's own edges come from the modifier below.
                            .onGeometryChange(for: CGRect.self) { $0.frame(in: .global) }
                                action: { reader.rowMoved(block.id, to: $0) }
                            // A row the list has let go of has no position, and the rectangle it
                            // last had is not one — see `ReaderModel.rowLeft`.
                            case .summary(let segment):
                                SegmentRow(
                                    segment: segment, open: reader.isOpen(segment),
                                    toggle: { reader.toggle(segment) }
                                )
                                .listRowInsets(
                                    EdgeInsets(top: 0, leading: 12, bottom: 0, trailing: 12))
                                .listRowBackground(theme.color(.panel))
                                .listRowSeparator(.hidden)
                            }
                        }
                        .listStyle(.plain)
                        .scrollContentBackground(.hidden)
                        .background(theme.color(.panel))
                        .contentMargins(.vertical, 10, for: .scrollContent)
                        // Where the list is, in the same space its rows just reported themselves
                        // in. This is the rectangle a row has to overlap to be on screen.
                        .onGeometryChange(for: CGRect.self) { $0.frame(in: .global) }
                            action: { reader.viewportMoved(to: $0) }
                        // And how long the document is and how far down it the list sits, which
                        // is the reason the floor is macOS 15: before it a `List` published
                        // neither. `--shot` prints these against AppKit's own numbers for the
                        // same scroll view, which is the check that the box and the transcript
                        // are the same instrument. `chat-search-me9.8.27`.
                        .onScrollGeometryChange(for: ScrollGeometry.self) { $0 } action: {
                            _, geometry in reader.scrolled(geometry)
                        }
                        // Beside the transcript and inside the same `ScrollViewReader`, because
                        // the two are one instrument: the map reports where the transcript is and
                        // the transcript goes where the map is dragged.
                        Minimap(layout: reader.minimap, reader: reader)
                    }
                    // Opened at the first message that matched, not at the top. A conversation
                    // reached through a search is one you were sent to for a reason, and the
                    // reason is 183 messages down often enough that the top is the wrong place
                    // to start reading.
                    //
                    // Anchored on the marks and not on `match_seqs`, which is the same trap the
                    // TUI documents: that list counts positions in one order and the transcript
                    // arrives in another, so a position resolved against the wrong one lands on
                    // an unrelated message. The marks are per message and came from the matcher
                    // that did the ranking, so they cannot be misaligned.
                    //
                    // A summary row counts as a match, because in segment mode the message that
                    // matched may be inside a run that is shut — and a scroll to a row `List` does
                    // not have is a scroll that silently does not happen.
                    .onChange(of: transcript.convId, initial: true) {
                        guard let first = reader.firstMarkedRow else { return }
                        scroll.scrollTo(first, anchor: .top)
                    }
                    // The other half of the scroll relationship: the minimap resolves a drag to a
                    // message and this is the only place holding the reader that can go there.
                    // `ScrollViewReader` scrolls to an id and to nothing else — macOS 15's
                    // `ScrollPosition` was measured and does not move a `List` at all — so the
                    // request carries an anchor, which is how a drag reaches a point *inside* a
                    // message. `ReaderModel.anchor(for:within:)` is where that is worked out.
                    .onChange(of: reader.scrollRequest) {
                        guard let request = reader.scrollRequest else { return }
                        scroll.scrollTo(ReaderRow.id(ofMessage: request.id), anchor: request.anchor)
                    }
                }
            }
        } else if let failure = reader.failure {
            notice(failure, tone: theme.color(.err))
        } else {
            notice(reader.loading ? "reading…" : "")
        }
    }

    /// The pane's non-content states, deliberately not the shell's dashed `.empty` box. That one
    /// is a whole screen saying a search found nothing; this is a column saying it has no
    /// transcript yet, and a box around it would claim more than that.
    private func notice(_ text: String, tone: Color? = nil) -> some View {
        Text(text)
            .font(theme.font(.sub))
            .foregroundStyle(tone ?? theme.color(.ink3))
            .multilineTextAlignment(.center)
            .textSelection(.enabled)
            .padding(22)
            .frame(maxWidth: .infinity, maxHeight: .infinity)
    }
}

/// One message: a spine, a sigil, and the text.
///
/// The spine is the categorical channel, which is what lets a user turn and an assistant turn
/// carry identical text contrast — `poc/ui` states it as "do not privilege user turns", and it
/// is the reason the band is drawn beside the message rather than in it.
struct BlockRow: View {
    let block: Block
    let fold: Fold
    /// The text with its matches marked, handed in already built. A row is given a drawing rather
    /// than the parts of one because assembling it is the expensive thing this file used to do on
    /// every body evaluation — `MarkedText` is where that argument and the two mark forms live.
    let marked: AttributedString
    let toggle: () -> Void
    @Environment(\.theme) private var theme

    var body: some View {
        HStack(alignment: .top, spacing: 8) {
            // 3pt wide, full height of whatever the text turns out to be, and 2pt of corner —
            // small enough that the seam's own note applies: a radius below 3 is a bug when it
            // is missing rather than a direction when it is there.
            RoundedRectangle(cornerRadius: 2)
                .fill(spine)
                .frame(width: 3)
            // The fold lives on the sigil rather than on the text, which departs from the mockup
            // deliberately: clicking the text is what the prototype does because a prototype has
            // nothing to select, and a transcript you cannot copy out of is a transcript you
            // cannot quote.
            Button(action: toggle) {
                Text(sigil)
                    .font(theme.font(.meta, .mono))
                    .foregroundStyle(theme.color(.ink3))
                    .frame(width: 13, alignment: .leading)
            }
            .buttonStyle(.plain)
            .help(fold == .collapsed ? "expand this message" : "collapse it")
            VStack(alignment: .leading, spacing: 2) {
                if block.isSidechain {
                    // A subagent turn is part of what happened and is not a reply you received,
                    // so it says which it is rather than sitting in the transcript as though it
                    // were the conversation.
                    Text("subagent")
                        .font(theme.font(.micro, .mono))
                        .foregroundStyle(theme.color(.ink3))
                }
                text
            }
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .padding(.bottom, 7)
    }

    private var text: some View {
        Text(marked)
            .font(face)
            .foregroundStyle(tone)
            // Collapsed is one line by definition — it stands in for a message too long to show,
            // so wrapping it would defeat the fold. The truncation is the view's, over the whole
            // text: nothing here picks a line or synthesises a summary, because the collapsed
            // *form* of a message is core's to state and does not exist on the wire yet
            // (`chat-search-me9.20`). Until it does, a collapsed tool call reads as its raw
            // argument, which is honest and is not pretty.
            .lineLimit(fold == .collapsed ? 1 : nil)
            .truncationMode(.tail)
            .textSelection(.enabled)
            .fixedSize(horizontal: false, vertical: true)
    }

    /// The kind ramp, which was solved as an even luminance ramp against `--map-bg` so that four
    /// categories stay apart at the ~2px a strip draws them at. A spine is wider than that and
    /// reads more easily, which is a reason to reuse the ramp and not a reason to pick again.
    ///
    /// The choice itself is `Display.bandToken` and not here, because the minimap beside this
    /// transcript draws the same message and has to reach the same colour.
    private var spine: Color { theme.color(Display.bandToken(block)) }

    private var sigil: String {
        if block.isError { return "✕" }
        return switch block.band {
        case .user: "▸"
        case .agent: "◂"
        case .reasoning: "⋯"
        // One glyph for every call. The prototype varies it by what the call was *for* — look,
        // run, change — which is a fact nothing on the wire carries.
        case .tool: "⚙"
        case .unrecognised: "·"
        }
    }

    /// Prose in the reading face, everything else in the quiet monospaced tier — the mockup's
    /// split, and the reason a screen of tool traffic does not read as a screen of conversation.
    ///
    /// The choice is `Display.textFace` and not here, for the same reason the spine's colour is
    /// `Display.bandToken`: `Markdown` sets a fence in the monospaced face *at this size* and a
    /// heading one step above it, so the size a message is set in has to be one answer rather than
    /// this view's and the styling's.
    private var face: Font {
        let setting = Display.textFace(block)
        return theme.font(setting.size, setting.face)
    }

    private var tone: Color {
        if block.isError { return theme.color(.err) }
        // Equal contrast for both sides of the prose. Neither role is demoted, which is what the
        // spine beside them is for.
        return Display.isProse(block) ? theme.color(.ink) : theme.color(.ink3)
    }
}
