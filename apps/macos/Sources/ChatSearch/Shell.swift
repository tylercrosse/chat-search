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

/// The window, which is now the search view and the frame around it.
///
/// **There used to be a strip above this holding a Search/Library switch, and a footer under it,
/// and all three are gone.** `poc/ui` put that switch in its own title bar, above the search field,
/// because grouping is a property of a list and a view is a place; this app had no title bar of its
/// own to put it in, so it became the first strip of the content. It is now the other way round —
/// ADR 27 took the titlebar back with `.fullSizeContentView`, and the search bar rose into it — and
/// the switch had nothing left to switch between in any case, because `LibraryView` was scrapped
/// for now (`chat-search-me9.8.30`). A strip holding one button that says *Search* on a window that
/// only searches is 30 points of the 480 pt floor spent on saying nothing.
///
/// The footer went the other way and up rather than out: everything it said is in `StatusStrip`,
/// directly under the search bar, where it is read beside the query that produced it rather than at
/// the far end of the window from it (`chat-search-me9.8.31`).
struct Shell: View {
    @Bindable var model: SearchModel
    @Environment(\.theme) private var theme

    var body: some View {
        SearchView(model: model)
            // Still 480, and it is a taller 480 than it was: the content view now reaches the top
            // edge, so the ~32 pt the system used to keep for its own titlebar are inside this
            // frame and are the app's to draw in.
            .frame(minWidth: 720, minHeight: 480)
            .background(theme.color(.bg))
    }
}
