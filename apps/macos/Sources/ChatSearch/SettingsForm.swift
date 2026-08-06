import CsTheme
import SwiftUI

/// Three settings on two axes, in the place macOS puts settings.
///
/// A `View > Theme` submenu with one checkmark can express one list. It cannot express three
/// settings where two of them are lists and the third governs which list you are currently looking
/// at (`chat-search-me9.8.22`), which is why this is a window at Cmd-comma rather than a menu of
/// directions. The arrangement carries that relationship: the appearance is first because it
/// decides which of the two menus under it you are seeing the effect of, and the caption under it
/// reports which side is *actually* being drawn rather than which one was asked for — the same
/// reading `AppearanceProbe` takes, for the same reason, since `system` is an answer you cannot get
/// from the setting alone.
///
/// **Preview on selection, not on confirm.** There is no OK button and nothing to apply: the app is
/// redrawing under this window as each control moves, which is the only preview worth having and
/// costs nothing, because swapping the theme in the environment already redraws every view
/// (`chat-search-me9.8.8`).
///
/// **It is drawn in system colours, and that is deliberate.** Every other view in this app reads
/// its colours off `\.theme` and none of them may name one; this one draws in stock AppKit controls
/// on the system's own ground, the way the title bar does. Two reasons. The preview is the window
/// *behind* this one, so a settings panel that also repainted itself would put the sample next to
/// the swatch and make neither readable. And a radio group and two pop-up menus painted in a
/// theme's tokens are a theme authoring a control, which is the direction this window is under the
/// most pressure to drift in — `docs/DECISIONS.md` ADR 25 turns on the app not being the author of
/// a theme, and selecting a direction is selecting.
struct SettingsForm: View {
    let settings: ThemeSettings

    /// Which side is on screen, taken from the appearance in force *here* rather than derived from
    /// the setting. Under `system` the setting does not know, and this does — and it follows a
    /// macOS flip at sunset without anything being told.
    @Environment(\.colorScheme) private var scheme

    var body: some View {
        Form {
            Picker("Appearance", selection: appearance) {
                ForEach(Appearance.allCases, id: \.self) { appearance in
                    // `system` is a thing somebody chooses rather than the absence of a choice
                    // (`Appearance`), so it is a radio button beside the other two and not a
                    // checkbox that turns them off.
                    Text(appearance.rawValue.capitalized).tag(appearance)
                }
            }
            .pickerStyle(.radioGroup)
            .horizontalRadioGroupLayout()

            caption(drawing)

            Picker("Light theme", selection: side(\.light, choose: settings.choose(light:))) {
                directions
            }
            Picker("Dark theme", selection: side(\.dark, choose: settings.choose(dark:))) {
                directions
            }

            caption(
                "Colour only — the type scale and the spacing come from \(settings.layout.name), "
                    + "on both sides.")
        }
        .formStyle(.grouped)
        .frame(width: 420)
        .fixedSize(horizontal: false, vertical: true)
    }

    /// Every direction this build carries, each named with its class beside it.
    ///
    /// `chat-search-me9.8.12` built the class so that it could be shown here. A build can carry a
    /// direction read off disk (`chat-search-me9.8.10`), and a menu that lists a shipped direction
    /// and a user theme without distinguishing them is telling you the two are equally fenced when
    /// ADR 25 says only one of them is. Named rather than defaulted, the same way `--verify-theme`
    /// names it: `Theme.directions` is what is compiled in, and being compiled in is exactly what
    /// makes those measurements binding.
    private var directions: some View {
        ForEach(Theme.directions, id: \.name) { direction in
            Text("\(direction.name) · \(ThemeClass.direction.label)").tag(direction.name)
        }
    }

    /// What is on screen right now, which is not always what was asked for.
    private var drawing: String {
        let side = scheme == .dark ? "dark" : "light"
        return settings.appearance == .system
            ? "Following macOS, which is drawing the \(side) side."
            : "Drawing the \(side) side."
    }

    private func caption(_ text: String) -> some View {
        Text(text)
            .font(.callout)
            .foregroundStyle(.secondary)
            .frame(maxWidth: .infinity, alignment: .leading)
    }

    private var appearance: Binding<Appearance> {
        Binding(get: { settings.appearance }, set: { settings.choose(appearance: $0) })
    }

    /// A side's menu, as the direction name it shows.
    ///
    /// The name and not the `Theme`, because a `Picker` tags its items with something hashable and
    /// a theme is a bag of resolved colours. `Theme.direction(named:)` is the one place a name
    /// becomes a theme, so this goes back through it rather than closing over the value the row was
    /// built from — and it silently does nothing for a name that missed, which cannot happen from a
    /// menu built out of that same list.
    private func side(
        _ current: KeyPath<ThemeSettings, Theme>, choose: @escaping (Theme) -> Void
    ) -> Binding<String> {
        Binding(
            get: { settings[keyPath: current].name },
            set: { name in Theme.direction(named: name).map(choose) })
    }
}
