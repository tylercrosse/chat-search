import AppKit

/// The app menu this executable never had, and the reason nothing could be hung anywhere.
///
/// `NSApplication` is created by hand here (`main.swift`) and a hand-made application has no menu
/// bar at all — not an empty one, none — so `Settings…` had nowhere to live and Cmd-Q did nothing.
/// Building it is a prerequisite for Cmd-comma rather than an alternative to it.
///
/// **One rule decides what is on it**, and it is `chat-search-me9.8.21`'s argument for Quit carried
/// to the end rather than a template copied. macOS delivers a key equivalent through a menu and
/// through nothing else, so an item earns its place here when a key would otherwise reach nothing.
/// That is one rule with two halves, and only the first was visible while every item on the bar was
/// AppKit's:
///
/// - **A key somebody already presses is dead without it.** Quit, and then the whole Edit menu. An
///   Edit menu is not a feature this app grew; it is the thing that was missing while Cmd-C did
///   nothing in an entirely ordinary text field — the field was never broken, the bar was absent.
///   `chat-search-me9.8.24`.
/// - **Or this app can do a thing the keyboard cannot reach at all.** Hiding the bottom drawer was
///   a button and nothing else, in a window whose only focused view is the query box — so there was
///   no view to bind a key on that would not be taking that key away from the box. A menu is
///   outside the responder chain and therefore outside that argument, which is what makes
///   `View ▸ Hide Timeline` possible where an arrow pair was not. `chat-search-me9.8.26`.
///
/// The rule cuts the other way just as hard, which is the half worth writing down. Every item a
/// template would put here and this one does not — Zoom, Bring All to Front, the window list, Hide
/// and its two companions, Delete, Paste and Match Style, Find, spelling, substitutions, speech —
/// either carries no key equivalent at all, or carries one that resolves to nothing in an app whose
/// only editable text is a one-line query box. An item that greys out the moment you look at it
/// says this app has a capability it does not have, and it says it in the one place people go to
/// find out. Each is named below or in `apps/macos/README.md` with what would have to become true
/// for it to arrive, and one of them — Hide — was on this menu until the pass measured it.
///
/// **The bottom of the Edit menu is not this app's**, and there is no version of this where it is.
/// The moment a menu is titled `Edit`, AppKit inserts Writing Tools, AutoFill, Start Dictation and
/// Emoji & Symbols into it. Two of those have documented switches
/// (`NSDisabledDictationMenuItem`, `NSDisabledCharacterPaletteMenuItem`, both verified to work
/// here) and two do not, so throwing the switches would buy a menu that is still the system's at
/// the bottom while taking away two working ways of *entering* text into a search box. They are
/// left alone and printed by `--clipboard` beside the seven items this app authored, which is the
/// honest arrangement: what is here on purpose is legible, and what the platform added is visible
/// rather than quietly attributed to this app.
@MainActor
enum MainMenu {
    /// The whole menu bar: four submenus.
    ///
    /// The first item of a main menu is the application menu whatever it is called — AppKit takes
    /// its title from the process rather than from here — which is why the outer item's title is
    /// empty and the inner titles say the name themselves. The other three are titled, and `Edit`
    /// is titled exactly that because AppKit matches the name when it decides where to insert the
    /// input-method items it owns (see `Bar.suppressInputMethodItems`).
    ///
    /// Both of this app's own items are aimed at one object, which is `AppHost`: it is the only
    /// thing in the process that holds both the windows and the model.
    static func build(target: AnyObject, settings: Selector, timeline: Selector) -> NSMenu {
        let bar = NSMenu()
        for (title, menu) in [
            ("", application(openSettings: target, action: settings)), ("Edit", edit()),
            ("View", view(toggleTimeline: target, action: timeline)), ("Window", window()),
        ] {
            let item = NSMenuItem()
            item.title = title
            item.submenu = menu
            menu.title = title
            bar.addItem(item)
        }
        return bar
    }

    /// About, Settings…, Quit — unchanged, and the unchanged part is a finding.
    ///
    /// **Hide was going to be here and is not, because the pass measured it.** Cmd-H is the most
    /// reflexive key on this platform after Cmd-Q, so it arrived on exactly the argument that
    /// carried Quit. Then `--clipboard` read the bar back: AppKit validates `hide:` as *disabled*
    /// in this process, with an explicit `NSApp` target and without one, while `NSApp.hide(nil)`
    /// called directly hides the app perfectly well. So the capability is there and the menu route
    /// is the part AppKit refuses. The suspect is the same one behind the defaults domain being
    /// named after the executable — this is a plain binary and not a bundle — but the reading is
    /// what governs either way, and a permanently grey item is precisely what the rule above
    /// excludes: it advertises a key that will never fire. `--clipboard` still presses Cmd-H, now
    /// as the negative control, because `matched false` is what every key on this bar looked like
    /// before this bead.
    ///
    /// Hide Others and Show All were never candidates. Both act on *other* applications, so neither
    /// is a key this app owes anybody, and Show All carries no key equivalent to deliver.
    private static func application(openSettings target: AnyObject, action: Selector) -> NSMenu {
        let name = ProcessInfo.processInfo.processName
        let menu = NSMenu()

        menu.addItem(
            withTitle: "About \(name)",
            action: #selector(NSApplication.orderFrontStandardAboutPanel(_:)), keyEquivalent: "")
        menu.addItem(.separator())

        // The ellipsis is the character rather than three dots, because it is what every other
        // menu on this machine uses and a menu is a place where the app is quoting the platform.
        let settings = NSMenuItem(title: "Settings…", action: action, keyEquivalent: ",")
        // Explicit, where the rest go to `nil` and up the responder chain: `AppHost` is the
        // application's delegate and a delegate is not in the responder chain, so an item aimed at
        // it by name is the only one that arrives.
        settings.target = target
        menu.addItem(settings)
        menu.addItem(.separator())

        menu.addItem(
            withTitle: "Quit \(name)", action: #selector(NSApplication.terminate(_:)),
            keyEquivalent: "q")
        return menu
    }

    /// The clipboard, and the undo the clipboard implies.
    ///
    /// Every action here goes to `nil`, which is the whole mechanism: AppKit walks the responder
    /// chain from the first responder and delivers `copy:` to whatever answers it. In the search
    /// field that is the window's field editor, an `NSTextView` that has implemented all five of
    /// these since before this app existed; in the reader it is the view SwiftUI's
    /// `textSelection(.enabled)` puts behind the transcript, which answers `copy:` and refuses the
    /// rest. Nothing in this app implements a clipboard, and after this menu nothing has to —
    /// which is why the fix is a file about menus and not a file about text.
    ///
    /// **Undo is on it because Paste is.** Cmd-V into a field with a phrase already in it destroys
    /// the phrase, and the key that gets it back is the one people reach for without looking. The
    /// query box is a one-line field and its undo stack is the field editor's, so this costs
    /// nothing to maintain and is wrong only if the field editor is.
    ///
    /// **Not here, and each for its own reason.** Delete: the key already deletes, through the
    /// field editor's own bindings, and the menu item is a mouse route to a keyboard act. Paste and
    /// Match Style: there is no style in a plain query and nothing in this app is rich text. Find:
    /// Cmd-F belongs to a document you search inside, and this window *is* a search — the box takes
    /// focus at launch and takes it back on any click that loses it (`SearchView`), so the key
    /// would either duplicate the state the app is already in or bind a second, narrower search
    /// beside the real one. Spelling, substitutions, transformations, speech and the emoji palette:
    /// a query is a grammar (`agent:`, `after:`, a quoted `dir:`), and the machinery that
    /// autocorrects prose is the machinery that would quietly rewrite one.
    private static func edit() -> NSMenu {
        let menu = NSMenu()
        // `Selector(("undo:"))` rather than `#selector`: the responder-chain action is `undo:` with
        // a sender, where `UndoManager.undo` takes none, so the two names do not refer to the same
        // message and only the first one arrives.
        menu.addItem(withTitle: "Undo", action: Selector(("undo:")), keyEquivalent: "z")
        let redo = NSMenuItem(title: "Redo", action: Selector(("redo:")), keyEquivalent: "z")
        redo.keyEquivalentModifierMask = [.command, .shift]
        menu.addItem(redo)
        menu.addItem(.separator())

        menu.addItem(withTitle: "Cut", action: #selector(NSText.cut(_:)), keyEquivalent: "x")
        menu.addItem(withTitle: "Copy", action: #selector(NSText.copy(_:)), keyEquivalent: "c")
        menu.addItem(withTitle: "Paste", action: #selector(NSText.paste(_:)), keyEquivalent: "v")
        menu.addItem(.separator())
        menu.addItem(
            withTitle: "Select All", action: #selector(NSText.selectAll(_:)), keyEquivalent: "a")
        return menu
    }

    /// One item, and the first thing on this bar that is an act of this app's rather than AppKit's.
    ///
    /// The bottom drawer opens and shuts from a button in its own corner and from nowhere else,
    /// which made it mouse-only — and `chat-search-me9.8.20` filed that as unfixable from where it
    /// stood, because this window has exactly one focused view and any key bound beside it is a key
    /// the query box stops getting. The fold escaped that only because Enter already belonged to the
    /// list rather than to the field (`chat-search-me9.8.15`); nothing else is going spare, and
    /// left/right in particular are how a caret moves through a query that is a grammar. A menu
    /// resolves its key equivalents before the responder chain is consulted at all, so this is the
    /// one place a new key can be bound in this window without taking it off something.
    ///
    /// **Cmd-T, and it is free here in a way it is not in most apps.** The platform spends it on New
    /// Tab and on Show Fonts; this app has no tabs (nothing sets `newWindowForTab:`, so AppKit adds
    /// no tab items either) and nothing rich enough to want a font panel, so it is dead weight
    /// otherwise and it is the letter the drawer is named after. `--clipboard` presses it and reads
    /// the drawer's state back, which is what makes that a measurement rather than a hope.
    ///
    /// **The title is a verb and it flips**, Finder's arrangement for Show/Hide Status Bar. Which
    /// verb is right is a fact about the model and this file holds no model, so `AppHost` writes it
    /// during validation — the same moment AppKit decides whether the item is live at all. Built
    /// reading `Hide`, which `--no-timeline` makes momentarily wrong and the first validation
    /// corrects before anybody can see it.
    ///
    /// **What is not on it, and it is one thing.** The grouping control is mouse-only in exactly the
    /// way the drawer was, and a View menu is now the place it could live — but the axis is four
    /// states rather than a toggle, and four items with four keys is a different argument that
    /// wants making on its own. `chat-search-me9.8.40`.
    private static func view(toggleTimeline target: AnyObject, action: Selector) -> NSMenu {
        let menu = NSMenu()
        let item = NSMenuItem(title: "Hide Timeline", action: action, keyEquivalent: "t")
        // Explicit, for the reason `Settings…` is: this goes to the application's delegate, which
        // is not a link in the chain a `nil` target walks.
        item.target = target
        menu.addItem(item)
        return menu
    }

    /// Close and Minimize, and nothing that lists windows.
    ///
    /// This app had one window until `chat-search-me9.8.21` gave it a second, and the second is
    /// opened by a key and could until now be dismissed only with the mouse. That asymmetry is the
    /// same shape as the bug this bead fixes, and Cmd-W is its answer: `performClose:` goes to the
    /// key window, so it shuts the settings panel when the panel is in front and the app when the
    /// app is — which is the ordinary meaning of the key and, on the main window, exactly what its
    /// close button already does.
    ///
    /// **No window list, no Bring All to Front, no Zoom.** The list is what `NSApp.windowsMenu`
    /// would fill in, and its job is getting back to a window you have lost behind others; there are
    /// two windows here and Cmd-comma already raises the second one, so the list would be a second
    /// route to a place one key already goes. Zoom and Bring All to Front carry no key equivalent
    /// between them, which puts them outside the rule this menu is built on — the green button is
    /// the affordance for one and there is nothing for the other to gather.
    private static func window() -> NSMenu {
        let menu = NSMenu()
        menu.addItem(
            withTitle: "Close", action: #selector(NSWindow.performClose(_:)), keyEquivalent: "w")
        menu.addItem(
            withTitle: "Minimize", action: #selector(NSWindow.performMiniaturize(_:)),
            keyEquivalent: "m")
        return menu
    }
}
