//! Colour (docs/TUI-DESIGN.md §7).
//!
//! Three layers, and only one of them is ours to pick:
//!
//! 1. **Structure** — selection, borders, dim text, warning, accent — comes from the
//!    terminal's *indexed* palette (0–15), never hardcoded RGB. The user has already themed
//!    their terminal; inheriting that is what makes this feel native rather than like it
//!    brought its own skin, and it is why a config-file theme system can stay P4. Hardcoded
//!    slate borders are invisible on a light background.
//! 2. **Source identity** — a discriminable categorical palette, *not* brand colours. Brand
//!    colours are picked for logos on white and collide in a terminal list: fast-resume
//!    renders four of its sources as near-identical white in the one column whose job is
//!    telling sources apart.
//! 3. **Match highlight** — deliberately the loudest thing on screen, and the one place an
//!    explicit high-contrast pair beats inheriting a theme colour that might land invisible.
//!
//! Nothing is ever encoded in colour alone. Age has a text label, source has a badge,
//! selection has a `▸` pointer as well as a fill. Colourblind users and 8-colour terminals
//! lose scanning speed, never information.

use ratatui::style::{Color, Modifier, Style};

/// Resolved styles for one render pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Theme {
    pub accent: Style,
    pub border: Style,
    pub dim: Style,
    pub warning: Style,
    pub error: Style,
    /// Full-width fill on the selected row. Sets both fg and bg, so it survives either
    /// terminal background.
    pub selected: Style,
    pub header: Style,
    /// Layer 3. Applied over whatever the underlying span was.
    pub match_hl: Style,
}

impl Theme {
    /// Indexed-palette theme. The only constructor that should exist until a real need for
    /// RGB appears.
    ///
    /// Layer 1 and layer 2 unavoidably share hues — sixteen slots, eight of them spent on
    /// source identity and one reserved for errors, leaves nothing disjoint. That is harmless
    /// because the two never occupy the same cell, and because the badge rather than the hue
    /// is what identifies a source.
    pub fn indexed() -> Self {
        Self {
            // Bright blue (12): with bright green it is one of only two saturated hues the
            // eight source slots and the reserved reds leave free. Plain blue (4) would be the
            // obvious pick and is the one indexed colour that routinely lands near-black on a
            // dark theme, so the accent — the thing that has to be *seen* — takes the bright
            // variant and leaves 4 to a badge that has its text to fall back on.
            accent: Style::new().fg(Color::LightBlue).add_modifier(Modifier::BOLD),
            // DIM rather than `Color::DarkGray`. DIM is an intensity applied to whatever
            // foreground the terminal already picked, so it recedes under both polarities;
            // index 8 ("bright black") is a light grey in many light themes, which is exactly
            // the unreadable-directory-cell failure §7 calls out.
            border: Style::new().add_modifier(Modifier::DIM),
            dim: Style::new().add_modifier(Modifier::DIM),
            // Yellow is claude-code's hue as well; per the note above, the header band and the
            // Agent column never share a cell, and the warning always carries its own words.
            warning: Style::new().fg(Color::Yellow),
            error: Style::new().fg(Color::Red).add_modifier(Modifier::BOLD),
            // 0 and 7 are the two slots whose *lightness* is dependable — 0 is the darkest
            // colour a theme defines and 7 the lightest non-bright one — so this pair keeps
            // its contrast whichever polarity the terminal runs, which no hue pair does.
            // Neutral also means the fill cannot swallow a source badge: none of the layer-2
            // hues is grey, so every badge stays legible against it.
            selected: Style::new().fg(Color::Black).bg(Color::Gray),
            // Weight, not hue: a heading reads as a heading on both polarities without
            // spending a colour, and an uncoloured top line does not compete with `accent`.
            header: Style::new().add_modifier(Modifier::BOLD),
            // §7 offers "reverse video or an explicit bg/fg pair" for layer 3; reverse video
            // is the one that also honours "no background except `selected`". It is strictly
            // better here anyway — it inverts whatever is *behind* it, so a match inside the
            // selected row inverts that row's fill instead of fighting it, and it cannot land
            // invisible the way a fixed pair can. BOLD is what keeps it distinct from the
            // selection fill, which is reverse video too once colour is gone.
            //
            // Reverse video makes the *foreground* the bar and the terminal's background the
            // lettering, so a colour here reads as a highlighter rather than as coloured text.
            // Blue (4) rather than bright blue (12) because 4 is the slot Solarized fills with
            // #268BD2: asking for the palette entry gets exactly that colour on a Solarized
            // terminal and the right idea everywhere else, which a hardcoded hex would not —
            // and it keeps layer 3 inside the rule the other two layers already obey. The
            // warning above about 4 landing near-black is about text drawn *in* it; reversed it
            // is a fill, and the knocked-out lettering is the terminal's background either way.
            //
            // DIM is subtracted rather than merely left unset. Both the snippet cell and a
            // fenced code block draw their text dim, and a match patched on top of either would
            // inherit it — so the one thing on screen that must never recede would recede
            // exactly where the eye is already working hardest.
            match_hl: Style::new()
                .fg(Color::Blue)
                .add_modifier(Modifier::REVERSED | Modifier::BOLD)
                .remove_modifier(Modifier::DIM),
        }
    }

    /// Everything unstyled, for `NO_COLOR` and `TERM=dumb`.
    ///
    /// Not literally everything: this is [`Theme::indexed`] with the hues stripped and the
    /// non-colour attributes kept, because BOLD/DIM/REVERSED are what a monochrome terminal
    /// still renders. Dropping them too would delete the emphasis rather than the colour.
    /// `selected` is the one slot that cannot simply lose its colour — an explicit fg/bg pair
    /// has no monochrome form, so it becomes reverse video.
    pub fn plain() -> Self {
        Self {
            accent: Style::new().add_modifier(Modifier::BOLD),
            border: Style::new().add_modifier(Modifier::DIM),
            dim: Style::new().add_modifier(Modifier::DIM),
            // The only slot that gives up its emphasis. Bold is the last weight available and
            // spending it on both warning and error would make the pair indistinguishable
            // while diluting what bold means everywhere else; a warning is always a sentence,
            // so it keeps its meaning as plain text.
            warning: Style::new(),
            error: Style::new().add_modifier(Modifier::BOLD),
            selected: Style::new().add_modifier(Modifier::REVERSED),
            header: Style::new().add_modifier(Modifier::BOLD),
            // Colourless, but DIM is still subtracted: a monochrome terminal renders DIM, so a
            // match inside dim text would recede here too.
            match_hl: Style::new()
                .add_modifier(Modifier::REVERSED | Modifier::BOLD)
                .remove_modifier(Modifier::DIM),
        }
    }

    /// Picks between the two by environment. `no_color` is the result of [`no_color_env`],
    /// passed in rather than read here so this stays testable.
    pub fn detect(no_color: bool) -> Self {
        if no_color { Self::plain() } else { Self::indexed() }
    }
}

/// True when `NO_COLOR` is set to anything, or `TERM` is `dumb`.
///
/// Per the NO_COLOR convention, *presence* disables colour regardless of value — including
/// when the value is empty.
pub fn no_color_env() -> bool {
    // `var_os`, not `var`: a non-UTF-8 value would make `var` return `Err` and read as absent,
    // which inverts the convention for the one case where the value is meant not to matter.
    disables_colour(std::env::var_os("NO_COLOR").is_some(), std::env::var("TERM").ok().as_deref())
}

/// The decision itself, with the environment lifted out.
///
/// Split off so it can be tested: env is process-global, and a test that sets `NO_COLOR`
/// races every other test in the binary.
fn disables_colour(no_color_set: bool, term: Option<&str>) -> bool {
    // Exact match, not a prefix or a case fold: `dumb` is a terminfo entry name, and
    // `dumb-emacs-ansi` — the one common relative — does support colour.
    no_color_set || term == Some("dumb")
}

/// Stable colour for a source id.
///
/// Assignment is by position in [`KNOWN_SOURCES`], so it never shifts as the corpus changes.
/// A source not in that list hashes into the same palette deterministically — the same id
/// always gets the same colour, on every machine and every run (ADR 7's instinct applied to
/// rendering).
///
/// Red is reserved for errors and never assigned here.
pub fn source_style(source: &str) -> Style {
    Style::new().fg(SOURCE_PALETTE[palette_index(source, &KNOWN_SOURCES)])
}

/// Layer 2, in [`KNOWN_SOURCES`] order (docs/TUI-DESIGN.md §7).
///
/// Eight hues is the ceiling for categorical discriminability; past it colour stops being an
/// encoding and the badge is the only identifier, which the "nothing in colour alone" rule
/// already assumes. Red (1) and bright red (9) are absent by construction — they mean error.
/// Grey and white are absent too, which is what keeps a badge legible against `selected`'s
/// grey fill, and is the specific failure fast-resume hits by rendering four of its sources
/// as near-identical white.
const SOURCE_PALETTE: [Color; 8] = [
    Color::Yellow,        // 3  claude-code
    Color::Cyan,          // 6  codex
    Color::Blue,          // 4  gemini-cli
    Color::Green,         // 2  chatgpt-export
    Color::Magenta,       // 5  claude-ai
    Color::LightYellow,   // 11 copilot
    Color::LightCyan,     // 14 opencode
    Color::LightMagenta,  // 13 cursor
];

/// Palette slot for a source, with the known list passed in so the positional guarantee can
/// be tested against a longer list without touching the real one.
///
/// The wrap matters once [`KNOWN_SOURCES`] outgrows the palette: a ninth source reuses the
/// first hue rather than panicking, and — the point of the whole function — the first eight
/// keep the slots they already had.
fn palette_index(source: &str, known: &[&str]) -> usize {
    let slot = match known.iter().position(|&s| s == source) {
        Some(pos) => pos as u64,
        // FNV-1a written out rather than `DefaultHasher`, whose output std explicitly does not
        // promise to keep across releases. The requirement here is that an unarchived source
        // id gets the same colour on every machine and every run, so a hash that may change
        // under a compiler upgrade fails it silently — the colours just quietly permute.
        None => fnv1a(source.as_bytes()),
    };
    // Reduce in u64: `slot as usize` first would truncate the hash on a 32-bit target and
    // change the answer there, which is the same portability bug in a different disguise.
    (slot % SOURCE_PALETTE.len() as u64) as usize
}

/// FNV-1a, 64-bit. Chosen for being short enough to read and pinned by test, not for quality:
/// the output space is eight buckets.
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &byte in bytes {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// Ordered so colour assignment is stable as sources are added. Append only; never reorder.
pub const KNOWN_SOURCES: [&str; 8] = [
    "claude-code",
    "codex",
    "gemini-cli",
    "chatgpt-export",
    "claude-ai",
    "copilot",
    "opencode",
    "cursor",
];

/// Short badge for the Agent column. The badge, not the colour, is the actual identifier.
///
/// Sized against §2's Agent column, which is 13 columns wide when the pane is split and 12
/// below that, so 12 is the real budget. Only the two ids that overflow it are shortened, and
/// both are shortened towards what the product is called rather than what the watched
/// directory is called: `chatgpt-export` (14) is an import path, not a name, and `claude-ai`
/// becomes `claude.ai` so it does not read as a second flavour of `claude-code` at a glance.
///
/// An unknown id is returned verbatim. It is clipped by the column's width-aware truncation
/// like every other cell, and inventing an abbreviation for a source we have never seen would
/// only make the one thing carrying the identity less recognisable.
pub fn source_badge(source: &str) -> &str {
    match source {
        "gemini-cli" => "gemini",
        "chatgpt-export" => "chatgpt",
        "claude-ai" => "claude.ai",
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use unicode_width::UnicodeWidthStr;

    use super::*;

    /// Every slot in a theme, paired with its field name so a failure names the offender.
    ///
    /// Destructured exhaustively on purpose: a field added to [`Theme`] without being added
    /// here fails to compile, rather than quietly escaping every rule enforced below.
    fn slots(theme: Theme) -> [(&'static str, Style); 8] {
        let Theme { accent, border, dim, warning, error, selected, header, match_hl } = theme;
        [
            ("accent", accent),
            ("border", border),
            ("dim", dim),
            ("warning", warning),
            ("error", error),
            ("selected", selected),
            ("header", header),
            ("match_hl", match_hl),
        ]
    }

    fn colours(style: Style) -> impl Iterator<Item = Color> {
        style.fg.into_iter().chain(style.bg)
    }

    /// Inherits the user's terminal theme — an ANSI name, a 0–15 index, or `Reset`. Truecolor
    /// and the 216-colour cube do not: both are absolute values that ignore the theme.
    fn inherits_terminal_theme(colour: Color) -> bool {
        match colour {
            Color::Rgb(..) => false,
            Color::Indexed(n) => n < 16,
            _ => true,
        }
    }

    fn strip_colour(mut style: Style) -> Style {
        style.fg = None;
        style.bg = None;
        style
    }

    // ---- layer 1 --------------------------------------------------------------------

    #[test]
    fn indexed_never_leaves_the_terminals_own_palette() {
        // The rule most likely to be broken by a later "just this one colour" edit, so it is
        // asserted against the returned styles rather than trusted to review.
        let offenders: Vec<_> = slots(Theme::indexed())
            .into_iter()
            .flat_map(|(name, style)| colours(style).map(move |c| (name, c)))
            .filter(|&(_, c)| !inherits_terminal_theme(c))
            .collect();
        assert!(offenders.is_empty(), "colour outside the indexed palette: {offenders:?}");

        let source_offenders: Vec<_> =
            SOURCE_PALETTE.into_iter().filter(|&c| !inherits_terminal_theme(c)).collect();
        assert!(source_offenders.is_empty(), "layer 2 left the palette: {source_offenders:?}");
    }

    #[test]
    fn only_the_selected_row_paints_a_background() {
        // A background anywhere else stops the terminal's own background showing through, and
        // on the full-screen widgets that means the app has brought its own skin after all.
        for (name, style) in slots(Theme::indexed()) {
            assert_eq!(style.bg.is_some(), name == "selected", "{name} bg = {:?}", style.bg);
        }
    }

    #[test]
    fn the_selection_fill_sets_both_halves_of_the_pair() {
        // A bg without an fg inherits the terminal's foreground, which is light on a dark
        // theme and dark on a light one — so exactly one of the two polarities renders the
        // selected row as unreadable, and it is never the one the author is developing on.
        let selected = Theme::indexed().selected;
        assert!(selected.fg.is_some() && selected.bg.is_some(), "{selected:?}");
    }

    #[test]
    fn the_match_highlight_composes_over_the_selection_rather_than_fighting_it() {
        // Layer 3 has to stay the loudest thing on screen *including* on the selected row.
        // Reverse video inverts whatever fill is underneath; a fixed fg/bg pair would have to
        // out-contrast a fill it cannot see, and would also break the no-background rule.
        for theme in [Theme::indexed(), Theme::plain()] {
            assert!(theme.match_hl.add_modifier.contains(Modifier::REVERSED));
            assert_eq!(theme.match_hl.bg, None);
            assert_ne!(theme.match_hl, theme.selected, "a match inside the selection vanishes");
        }
    }

    #[test]
    fn the_match_highlight_refuses_to_be_dimmed_by_whatever_it_lands_on() {
        // Snippet cells and fenced code blocks both draw their text DIM, and a mark is patched
        // over the style underneath it. Merely not setting DIM here would let it be inherited,
        // so the loudest thing on screen would recede in the two places it is needed most.
        for theme in [Theme::indexed(), Theme::plain()] {
            assert!(theme.match_hl.sub_modifier.contains(Modifier::DIM), "{:?}", theme.match_hl);
            let on_dim = theme.dim.patch(theme.match_hl);
            assert!(!on_dim.add_modifier.contains(Modifier::DIM), "dim survived: {on_dim:?}");
            assert!(on_dim.add_modifier.contains(Modifier::REVERSED));
        }
    }

    // ---- layer 1 without colour -----------------------------------------------------

    #[test]
    fn plain_emits_no_colour_at_all() {
        for (name, style) in slots(Theme::plain()) {
            assert_eq!((style.fg, style.bg), (None, None), "{name} is still coloured");
        }
    }

    #[test]
    fn plain_is_indexed_with_the_hues_removed_except_for_the_selection() {
        // Pins the intent of `plain`: it drops colour, not emphasis. BOLD/DIM/REVERSED all
        // render on a monochrome terminal, so dropping them would delete information the
        // colour was only reinforcing.
        for ((name, plain), (_, indexed)) in
            slots(Theme::plain()).into_iter().zip(slots(Theme::indexed()))
        {
            if name == "selected" {
                // The one slot with no monochrome form of itself: an explicit fg/bg pair
                // stripped of both is nothing at all, so it re-expresses the fill as reverse
                // video instead.
                assert_eq!(plain, Style::new().add_modifier(Modifier::REVERSED));
                continue;
            }
            assert_eq!(plain, strip_colour(indexed), "{name} lost its non-colour emphasis");
        }
    }

    #[test]
    fn detect_selects_by_the_flag_it_is_given() {
        assert_eq!(Theme::detect(true), Theme::plain());
        assert_eq!(Theme::detect(false), Theme::indexed());
    }

    #[test]
    fn no_color_is_presence_not_truthiness() {
        // The convention is that NO_COLOR set to *anything* disables colour. An empty value is
        // the case an `is_empty()` or a `== "1"` check gets wrong, and it is the value a shell
        // produces from `NO_COLOR=` — the common way people write it.
        assert!(disables_colour(true, Some("xterm-256color")));
        assert!(disables_colour(true, None));
        assert!(disables_colour(false, Some("dumb")));
        assert!(!disables_colour(false, Some("xterm-256color")));
        assert!(!disables_colour(false, None));
        // `dumb-emacs-ansi` is a real terminfo entry that does support colour, so the match
        // must not be a prefix.
        assert!(!disables_colour(false, Some("dumb-emacs-ansi")));
    }

    // ---- layer 2 --------------------------------------------------------------------

    #[test]
    fn every_known_source_gets_a_distinct_colour_and_none_of_them_is_red() {
        let mut seen = HashSet::new();
        for source in KNOWN_SOURCES {
            let fg = source_style(source).fg.expect("a source style is a foreground");
            assert!(
                !matches!(fg, Color::Red | Color::LightRed | Color::Indexed(1 | 9)),
                "{source} took the colour reserved for errors"
            );
            assert!(seen.insert(fg), "{source} reuses {fg:?}; the column cannot discriminate");
        }
    }

    #[test]
    fn a_source_keeps_its_colour_across_calls_and_across_runs() {
        for source in KNOWN_SOURCES.iter().copied().chain(["amp", "zed", "some-unwatched-tool"]) {
            assert_eq!(source_style(source), source_style(source), "{source}");
        }
        // Pinned literals, because "deterministic" here means the same on every machine and
        // every Rust release, which a self-consistency check alone cannot catch: `DefaultHasher`
        // would pass the assertion above and still repaint the whole column after a toolchain
        // upgrade. These two values come from the FNV-1a constants in this file.
        assert_eq!(source_style("amp").fg, Some(Color::Cyan));
        assert_eq!(source_style("zed").fg, Some(Color::Magenta));
    }

    #[test]
    fn an_unknown_source_lands_in_the_palette_whatever_its_bytes_are() {
        // Empty and non-ASCII ids are the inputs that would panic a byte-slicing or
        // `char`-indexing implementation; the hash consumes bytes, so they merely bucket.
        let unknown = ["", "amp", "zed", "windsurf", "日本語ツール", "a-very-long-unregistered-id"];
        for id in unknown {
            let fg = source_style(id).fg.expect("every source resolves to a foreground");
            assert!(SOURCE_PALETTE.contains(&fg), "{id:?} escaped the palette with {fg:?}");
        }
        assert_ne!(source_style("amp"), source_style("zed"));
    }

    #[test]
    fn appending_a_source_does_not_repaint_the_existing_ones() {
        // The reason assignment is positional rather than sorted: `amp` sorts before every
        // current entry, so a sorted scheme would shift all eight the day it is added. Tested
        // against a synthetic list because the real one must not be mutated to prove it.
        let extended: Vec<&str> = KNOWN_SOURCES.iter().copied().chain(["amp"]).collect();
        for (pos, source) in KNOWN_SOURCES.iter().enumerate() {
            assert_eq!(palette_index(source, &KNOWN_SOURCES), pos);
            assert_eq!(palette_index(source, &extended), pos, "{source} moved");
        }
        // A ninth source wraps onto the first hue instead of indexing past the palette. Note
        // that it does change `amp`'s own colour — Cyan while unknown, Yellow once listed —
        // which is the accepted cost of promoting a source, and why the badge is the identifier.
        assert_eq!(palette_index("amp", &extended), 0);
    }

    // ---- badges ---------------------------------------------------------------------

    /// §2's narrowest Agent column. The split layout allows 13; sizing to the narrow tier
    /// means no badge is ever the reason a row starts eliding.
    const AGENT_COL_MIN_WIDTH: usize = 12;

    #[test]
    fn every_badge_fits_the_agent_column() {
        for source in KNOWN_SOURCES {
            let badge = source_badge(source);
            // Display columns, not bytes: `claude.ai` is ASCII today but the bound has to hold
            // for whatever a future source is called.
            let width = UnicodeWidthStr::width(badge);
            assert!(width <= AGENT_COL_MIN_WIDTH, "{source} badge {badge:?} is {width} columns");
            assert!(!badge.is_empty(), "{source} has no badge, so the row has no identifier");
        }
    }

    #[test]
    fn the_two_overlong_ids_stay_legible_rather_than_being_clipped() {
        // The pair the column was sized against. `chatgpt-export` is 14 columns raw and would
        // otherwise render as `chatgpt-exp…`, and `claude-code` must not collapse to something
        // a reader confuses with `claude-ai`.
        assert_eq!(source_badge("chatgpt-export"), "chatgpt");
        assert_eq!(source_badge("claude-code"), "claude-code");
        assert_eq!(source_badge("claude-ai"), "claude.ai");
        assert_ne!(source_badge("claude-code"), source_badge("claude-ai"));
    }

    #[test]
    fn an_unknown_source_badges_as_its_own_id() {
        assert_eq!(source_badge("amp"), "amp");
        // Not shortened, and deliberately not clipped here either — the Agent column's own
        // width-aware truncation is the single place that decides what fits.
        let long = "a-very-long-unregistered-id";
        assert_eq!(source_badge(long), long);
        assert!(UnicodeWidthStr::width(long) > AGENT_COL_MIN_WIDTH);
    }
}
