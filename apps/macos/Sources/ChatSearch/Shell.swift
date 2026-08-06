import CsKit
import CsTheme
import SwiftUI

/// The two views this window has, and there are two rather than four for a measured reason.
///
/// `poc/ui` had Search, Projects and Sittings drawing the same rows from the same filters into the
/// same drawer — `GROUP BY` wearing the costume of a destination — so they collapsed into
/// `Grouping`, a control on the one list. What was left over is the only surface that is not
/// derived from the index at all, and it got the freed tab (`chat-search-4ar.10`).
enum Surface: String, CaseIterable, Identifiable, Sendable {
    case search
    case library

    var id: String { rawValue }

    var label: String {
        switch self {
        case .search: "Search"
        case .library: "Library"
        }
    }
}

/// The window: a view switch, whichever view is showing, and one footer under both.
///
/// `poc/ui`'s title bar is where the switch lives, above the search bar rather than in it, because
/// grouping is a property of a list and a view is a place. This app's title bar belongs to the
/// window server, so the switch is the first strip of the content and reads in the same order.
struct Shell: View {
    @Bindable var model: SearchModel
    @Environment(\.theme) private var theme

    var body: some View {
        VStack(spacing: 0) {
            nav
            rule
            switch model.surface {
            case .search: SearchView(model: model)
            case .library: LibraryView()
            }
            rule
            footer
        }
        .frame(minWidth: 720, minHeight: 480)
        .background(theme.color(.bg))
    }

    /// `Divider()` draws a system separator, which is a colour this app did not choose. One point
    /// of `--rule` is the same line and is the theme's.
    private var rule: some View {
        Rectangle().fill(theme.color(.rule)).frame(height: 1)
    }

    /// `poc/ui`'s `.views`: two buttons in one bordered group, the showing one filled.
    private var nav: some View {
        HStack(spacing: 12) {
            HStack(spacing: 0) {
                ForEach(Surface.allCases) { surface in
                    Button { model.surface = surface } label: {
                        Text(surface.label)
                            .font(theme.font(.sub, weight: model.surface == surface ? .semibold : .regular))
                            .foregroundStyle(theme.color(model.surface == surface ? .selInk : .ink2))
                            .padding(.horizontal, 12)
                            .padding(.vertical, 3)
                            .background(
                                RoundedRectangle(cornerRadius: theme.metric(.r2))
                                    .fill(model.surface == surface ? theme.color(.sel) : .clear))
                            .contentShape(Rectangle())
                    }
                    // `.plain`, because a stock button paints itself in the system accent — a
                    // colour chosen somewhere this app cannot see.
                    .buttonStyle(.plain)
                }
            }
            .padding(1.5)
            .overlay(
                RoundedRectangle(cornerRadius: theme.metric(.r4))
                    .strokeBorder(theme.color(.rule2)))
            Spacer(minLength: 0)
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 6)
        .frame(maxWidth: .infinity, alignment: .leading)
        // The prototype's title bar gradient, which is two tokens rather than a colour: it is what
        // separates the chrome from the page without a second rule under the one below.
        .background(
            LinearGradient(
                colors: [theme.color(.panel2), theme.color(.panel)],
                startPoint: .top, endPoint: .bottom))
    }

    /// One footer, saying what the view under it is made of.
    ///
    /// Under Search that is the count and what the query cost. Under Library it is the sentence
    /// that separates the two: everything else in this window is derived from the index and can be
    /// thrown away with it, and nothing in Library can.
    private var footer: some View {
        HStack(spacing: 14) {
            switch model.surface {
            case .search:
                // `total` is only a number when `settled`; otherwise it is a floor, and a poor one
                // — the typeahead `the` came back at 1,025 against a true 4,243 — so it is ranged
                // rather than printed. Saying "1,025 matches" when the answer is four times that
                // is the one thing this field exists to stop a client doing.
                Text(
                    "\(model.conversations.count) of "
                        + (model.settled ? "\(model.total)" : "\(model.total)+"))
                    .foregroundStyle(theme.color(.ink2))
                if model.grouping != .none {
                    Text("\(model.groups.count) groups by \(model.grouping.label)")
                }
                Spacer()
                // Kept because it costs nothing: `cs` measures its own query time and the client
                // already has a clock around the spawn, so the two numbers are decoded rather than
                // taken. A latency you cannot see while you type is a latency you will argue about.
                if let t = model.lastTiming {
                    Text(String(format: "%.0f ms", t.total))
                    Text(String(format: "%.1f in sqlite", t.serverMs))
                }
            case .library:
                Text("authored, not derived — survives a reindex")
                Spacer()
            }
        }
        // Monospaced at `--fs-meta`, which is the mockup's rule for everything tabular: a number
        // that changes every keystroke has to stop the line reflowing while you read it.
        .font(theme.font(.meta, .mono))
        .foregroundStyle(theme.color(.ink3))
        .padding(.horizontal, 12)
        .padding(.vertical, 6)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(theme.color(.panel))
    }
}
