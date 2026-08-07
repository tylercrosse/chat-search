import CsKit
import CsTheme
import SwiftUI

/// The root of the view tree, and the one place the theme in force enters the environment.
///
/// A view rather than an `.environment(\.theme, theme)` written once into `NSHostingView`'s root:
/// the settings window changes the theme while the app is running (`chat-search-me9.8.21`), and an
/// environment fixed at construction would make every choice a relaunch. Reading `settings.theme`
/// inside `body` is the whole of the subscription, and swapping the value redraws every view under
/// it — which is what the token seam was built before the views to buy (`chat-search-me9.8.8`).
///
/// The titlebar's measurements come the other way — taken once off the window and never moved —
/// but through the same environment, because the view that needs them is three levels down and the
/// levels between have no opinion about window chrome (`TitleBar`).
struct Root: View {
    let model: SearchModel
    let settings: ThemeSettings
    let titleBar: TitleBar.Metrics

    var body: some View {
        Shell(model: model)
            .environment(\.theme, settings.theme)
            .environment(\.titleBar, titleBar)
    }
}

/// The window: the search view, and one footer under it.
///
/// **There used to be a strip above this holding a Search/Library switch, and both are gone.**
/// `poc/ui` put that switch in its own title bar, above the search field, because grouping is a
/// property of a list and a view is a place; this app had no title bar of its own to put it in, so
/// it became the first strip of the content. It is now the other way round — ADR 27 took the
/// titlebar back with `.fullSizeContentView`, and the search bar rose into it — and the switch had
/// nothing left to switch between in any case, because `LibraryView` was scrapped for now
/// (`chat-search-me9.8.30`). A strip holding one button that says *Search* on a window that only
/// searches is 30 points of the 480 pt floor spent on saying nothing.
struct Shell: View {
    @Bindable var model: SearchModel
    @Environment(\.theme) private var theme

    var body: some View {
        VStack(spacing: 0) {
            SearchView(model: model)
            rule
            footer
        }
        // Still 480, and it is a taller 480 than it was: the content view now reaches the top edge,
        // so the ~32 pt the system used to keep for its own titlebar are inside this frame and are
        // the app's to draw in. With the nav strip gone as well, the list starts about 60 pt higher
        // in a window of the same height — which is the room `chat-search-me9.8.31`'s status strip
        // and scrubber are about to spend.
        .frame(minWidth: 720, minHeight: 480)
        .background(theme.color(.bg))
    }

    /// `Divider()` draws a system separator, which is a colour this app did not choose. One point
    /// of `--rule` is the same line and is the theme's.
    private var rule: some View {
        Rectangle().fill(theme.color(.rule)).frame(height: 1)
    }

    /// One footer, saying what the list above it is made of: the count, and what the query cost.
    private var footer: some View {
        HStack(spacing: 14) {
            // `total` is only a number when `settled`; otherwise it is a floor, and a poor one —
            // the typeahead `the` came back at 1,025 against a true 4,243 — so it is ranged rather
            // than printed. Saying "1,025 matches" when the answer is four times that is the one
            // thing this field exists to stop a client doing.
            Text(
                "\(model.conversations.count) of "
                    + (model.settled ? "\(model.total)" : "\(model.total)+"))
                .foregroundStyle(theme.color(.ink2))
            if model.grouping != .none {
                // The folded count sits with the group count rather than replacing it: a folded
                // group is still a group, and a screen of eleven heads has to be able to say
                // whether it is eleven groups or eleven groups with nine of them shut.
                Text(
                    "\(model.groups.count) groups by \(model.grouping.label)"
                        + (model.foldedGroups > 0 ? " · \(model.foldedGroups) folded" : ""))
            }
            // What Enter would do where the cursor is, and only when that is not what Enter has
            // always done. The prototype drew `→` here and wired nothing (`poc/ui/NOTES.md` §5);
            // wiring a key and drawing nothing is the same defect from the other side.
            if let hint = model.cursorHint {
                Text(hint).foregroundStyle(theme.color(.ink2))
            }
            Spacer()
            // Kept because it costs nothing: `cs` measures its own query time and the client
            // already has a clock around the spawn, so the two numbers are decoded rather than
            // taken. A latency you cannot see while you type is a latency you will argue about.
            if let t = model.lastTiming {
                Text(String(format: "%.0f ms", t.total))
                Text(String(format: "%.1f in sqlite", t.serverMs))
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
