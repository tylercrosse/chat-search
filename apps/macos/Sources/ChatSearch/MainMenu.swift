import AppKit

/// The app menu this executable never had, and the reason nothing could be hung anywhere.
///
/// `NSApplication` is created by hand here (`main.swift`) and a hand-made application has no menu
/// bar at all — not an empty one, none — so `Settings…` had nowhere to live and Cmd-Q did nothing.
/// Building it is a prerequisite for Cmd-comma rather than an alternative to it.
///
/// **Quit is not scope creep.** It is broken today, the menu is the only place a key equivalent can
/// live, and shipping the menu that fixes it while leaving it out would be a deliberate choice to
/// leave it broken.
///
/// **What is deliberately not here**: Edit, Window, and Hide. An Edit menu would fix Cmd-C and
/// Cmd-V in the search field, which are broken for exactly the same reason Cmd-Q was — that is a
/// real gap and it is `chat-search-me9.8.24`, filed rather than folded in, because "the menu is
/// where a key equivalent lives" is an argument for a menu bar and not an argument for any
/// particular menu on it.
@MainActor
enum MainMenu {
    /// The whole menu bar: one submenu, three items.
    ///
    /// The first item of a main menu is the application menu whatever it is called — AppKit takes
    /// its title from the process rather than from here — which is why the outer item's title is
    /// empty and the inner titles say the name themselves.
    static func build(openSettings target: AnyObject, action: Selector) -> NSMenu {
        let name = ProcessInfo.processInfo.processName

        let app = NSMenu()
        app.addItem(
            withTitle: "About \(name)",
            action: #selector(NSApplication.orderFrontStandardAboutPanel(_:)), keyEquivalent: "")
        app.addItem(.separator())

        // The ellipsis is the character rather than three dots, because it is what every other
        // menu on this machine uses and a menu is a place where the app is quoting the platform.
        let settings = NSMenuItem(title: "Settings…", action: action, keyEquivalent: ",")
        // Explicit, where About and Quit go to `nil` and up the responder chain: `AppHost` is the
        // application's delegate and a delegate is not in the responder chain, so an item aimed at
        // it by name is the only one that arrives.
        settings.target = target
        app.addItem(settings)
        app.addItem(.separator())

        app.addItem(
            withTitle: "Quit \(name)", action: #selector(NSApplication.terminate(_:)),
            keyEquivalent: "q")

        let item = NSMenuItem()
        item.submenu = app
        let bar = NSMenu()
        bar.addItem(item)
        return bar
    }
}
