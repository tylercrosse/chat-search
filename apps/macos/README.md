# chat-search for macOS

The Swift surface. A search field, a grouping control, a facet rail, a result list, a reader beside
it, a timeline under all of it, and a way back into the conversation — `chat-search-me9.8.2` onward
is what makes it worth using. Two views, because three of the prototype's four were the same list
cut differently.

```bash
cargo build --release                       # the app finds ./target/release/cs by itself
cd apps/macos && swift run -c release chat-search
```

This file is the implementation record: how each piece works, and what it measured.
[ROADMAP.md](./ROADMAP.md) is the map above it — what is built, what is next, and why the
remaining beads are ordered the way they are.

No Xcode project, no asset catalog, no bundle: the Command Line Tools SDK and nothing else, the
same terms `poc/swift` was built on. When something here needs a bundle — a Dock icon, a login
item, a URL scheme — that is the moment to add one.

Flags, most of which exist so this can be exercised without touching real data:

```bash
swift run -c release chat-search --db /tmp/scratch.db --config /tmp/scratch-config.toml
swift run -c release chat-search --bin /path/to/cs --limit 30
swift run -c release chat-search --size 720x480
swift run -c release chat-search --group project --folded
swift run -c release chat-search --no-timeline
swift run -c release chat-search --settings
swift run -c release chat-search --theme paper
swift run -c release chat-search --appearance light --theme-light paper --theme-dark terminal
swift run -c release chat-search --theme-file ~/.config/chat-search/theme.css
swift run -c release chat-search --write-theme
```

`--size` opens the window at a stated size, `--group` opens the list already cut along an axis,
`--folded` opens every group of it shut, `--no-timeline` opens the bottom drawer closed, and
`--settings` opens the settings window beside it. All five are verification affordances rather than
preferences — the row has to hold at several widths, a grouped list rebuilds sections where an
ungrouped one rebuilds rows, a folded one draws heads where an open one draws heads and rows, an
open drawer is a third `cs` per keystroke, and the settings window is behind a Cmd-comma no script
can press, so `--measure` and `--shot` need a way into each of those modes. A window that always
opens at one size, always ungrouped, or always open, makes checking any of them a manual drag
nobody repeats. `--folded` moves the *default* rather than folding what is on screen at the time,
so a group arriving on a later keystroke is folded too: an instrument that measured a list
unfolding itself as it typed would not be measuring anything.

The theme flags are the ones that are *not* instruments. They choose among the six directions
compiled into the binary and which side of one you are looking at, and they stick, which is why
they are the only flags here that change anything about the next launch. `--theme-file` is the
exception to the exception: it draws a token set that is not in the binary at all, and it sticks
without being remembered, because the file is the memory. See [the theme seam](#the-theme-seam).

## What it is made of

**`Sources/CsKit`** — the decoder and the transport, and a library product rather than something
private to the app. It is the repo's only non-Rust reader of [`docs/JSON-CONTRACT.md`] — all three
replies, `cs search --json`, `cs facets --json` and `cs timeline --json` — it is written once, and
`poc/swift` consumes it so that `cs-spike contract` checks the same decoder this app is built on.
The dependency points instrument at product; nothing here points back into `poc/`.

**`Sources/CsTheme`** — the token layer, and a target of its own so that "a view may read a token
and may not author one" is a thing the compiler enforces rather than a thing a review notices.
See [the theme seam](#the-theme-seam) below.

**`Sources/ChatSearch`** — the window, the models and the views. One `cs search --json` per
keystroke with the previous one killed and no debounce, which is not an oversight:
`chat-search-me9.22` measured fork/exec at 0.3 ms and the whole process boundary at 5–13 ms, so
a debounce would be latency spent to save a cost that was measured and found small. With a
conversation open that is two processes per keystroke rather than one, both cancellable: the
median conversation is ~10 KB and the corpus's longest measured 50–90 ms end to end, against the
~50 ms the search beside it already costs. With the [bottom drawer](#the-bottom-drawer) open it is
three, on the same terms and for the same reason — see there for what that one costs and what
shutting it buys. Rows and messages both go through `List` because it is the only one of
SwiftUI's three containers that recycles — 5.2 MB scrolling the whole corpus against
`LazyVStack`'s 65.6 MB and `VStack`'s 566 MB. That question is answered, so the app does
not offer the other two.

[`docs/JSON-CONTRACT.md`]: ../../docs/JSON-CONTRACT.md

## Facets: the query text is the filter

Clicking anything in the rail does exactly one thing — it puts a string in the search box. There
is no filter state anywhere in this app, so there is nothing that can fall out of step with what
is typed, and a filter arrived at by clicking is one you can then edit, copy out, or paste into
`cs explain`. That is docs/TUI-DESIGN.md §5, where the tool this was lifted from kept a selection
beside its query and paid six reconciliation methods for it.

Which means the app needs the answer to "what does clicking this produce", and it may not work it
out. The rules — widen an existing `agent:`, drop a standing exclusion, put a new token in front
of the free text — live in `cs_core::query` with the grammar, and a client assembling `agent:`
tokens itself would be the second, partial parser §5 costs out. So it asks:

```bash
cs facets "borrow checker agent:codex" --json
```

Every chip comes back carrying **the whole query text clicking it produces**, plus what the query
currently says about it. [`docs/JSON-CONTRACT.md`] has the shape and why it is a command of its
own rather than a key on the search envelope.

**The rail is a census, not a list of what matched.** A source with no rows still gets a row, and
a configured source at zero gets a `!`: that is a broken importer or an archive run that never
happened, and a bar built from the index alone cannot draw it at all — you search, get nothing,
and conclude you used a different tool (`chat-search-a7k.29`). A source that is on this machine
and configured by nothing is drawn dim and does not offer to be clicked, because its conversations
are not being captured and filtering to it would return an empty list.

**Three sections, and the app cannot tell them apart.** A second source widens the `agent:`
token; a second span *replaces* the `date:` one, because two date tokens intersect and the
overlap of two spans is the smaller of them or nothing; one `dir:` fragment lights every
directory beneath it, because `dir:` is a substring match. Every one of those rules is in
`cs_core::query` with the grammar, so this side is three shapes and no rules
(`chat-search-1ld`).

**A section head is its label and nothing else.** `poc/ui` puts what the section is a facet of at
the right margin of the head — `agent: · config ∪ index` beside `SOURCES` — and a browser column
is not 232pt. Twelve points of padding either side leaves about 208, which holds roughly 28
characters of the micro face at 1.4 tracking, and those two strings are 30: the meta wrapped under
its own label and every populated section read as two competing lines rather than one head. It is
on hover now, which is the trade the group key above the list already makes for the same reason
(`chat-search-me9.8.35`).

They are ordered by *coverage* rather than by how interesting the facet is, which is `poc/ui`'s
ordering: `ended_at` answers for every conversation, a source for every conversation, a `cwd` for
a quarter of them. The `dir:` section says both halves of that last number — it draws the busiest
12 of 128 directories, which its head says on hover, and its final line is the 3,303 conversations
that record none at all. Only the agent sources have a working directory, so `dir:` cannot reach a
ChatGPT conversation, and a section that showed only the directories would read as a complete
account of where the work happened. Its chips are paths rather than project names: deriving one collapsed seven unrelated
directories onto a single label, which is why `chat-search-6eb.26` was closed.

A directory whose click cannot be written is not offered at all. No `dir:` token can carry a path
with a space or a comma, so `cs facets` reparses each click and drops the ones that do not come
back naming the directory — `chat-search-me9.8.16` is the quoting that would make them reachable.

Two things this does not do:

- **No source colour.** The five `--src-*` hues are in the token layer and the rule that maps eight
  source ids onto five of them is not written yet; it belongs with the row's agent badge
  (`chat-search-me9.8.2`), and writing a second copy here is what the epic's sequencing exists to
  prevent. `chat-search-g6u`.
- **A filter you can see is not a filter you can read.** The TUI highlights the query as you type
  and strikes through a value that selects nothing. Here that value is reported after the fact,
  in the banner below the box.

### `unapplied_filters`, which is why the banner is not an error

`agent:notathing` parses as a filter and then selects nothing, so the search comes back **wider
than the query asked for, with exit status 0**. A client that ignores that field shows unfiltered
results for a filtered query, and it looks like it worked (`chat-search-6eb.11`). So it gets a
line under the search box, in the same quiet register as the rebuilding banner — nothing failed.

## Opening a conversation

Enter on the search field, double-click a row, or the row's context menu. All three take
`Group.destinations`, which arrives as *data* — `terminal(argv:)` or `web(url:)`, best first —
so nothing here greps a string for `https://`.

| destination | what happens |
| --- | --- |
| `web` | `NSWorkspace` opens the URL, which is what "hand it to the platform opener" means |
| `terminal` | `cs pick --in terminal` returns the shell line; it is written to a `.command` file and opened |
| empty list | a sentence saying this source has no way back in, which is a fact and not a failure |

**The line is never composed here.** Quoting a directory and a session id into one shell line is a
rule with exactly one home — `cs pick`, `cs tui` and the fzf script before them each grew a
version of it — so the app asks for it. A `.command` file rather than `osascript … tell app
"Terminal"`: that names one emulator, wants an automation grant, and its AppleScript escaping
would be a *second* quoting rule this app owned. The file carries no shebang on purpose, so the
line resolves against the `PATH` the user has rather than the one a GUI process inherits.

**Every path records the pick**, including the two that open nothing. A conversation that was
wanted and could not be reached is as much of a relevance judgement as one that was, and picks are
the only judgements the query log has (docs/TUI-DESIGN.md §6).

### And the other half: quitting without opening anything

Close the window with something in the search box and nothing opened, and the log gets one
`Search`. That is the abandonment signal — *the ranking showed me nothing worth opening* — and it
is the only thing `queries.jsonl` ever learns that is not a success. Without it a ranking is
measured against the answers it did produce, which cannot find it wanting.

| moment | event |
| --- | --- |
| a keystroke | **nothing** |
| Enter, a double-click, a context menu | one `Pick`, through `cs pick` |
| quit with a non-blank query and no pick | one `Search`, through `cs abandon` |

`cs abandon` is a verb this bead added, because there was nothing to call. `cs search` records a
`Search` on the non-`--prefix` path only — one line per keystroke would bury the handful of real
queries under every prefix of each — and that is every path this app takes. It re-ranks the
finished query on the far side rather than writing down what was on screen, for the reason
`cs pick` recomputes a rank: the list here is a typeahead list of whatever was half-typed at the
time, and a `Search` and the `Pick` it might have been are only comparable if both describe the
same ask. It also decides what counts as a query with a need behind it, so nothing here re-derives
that: whitespace, `??` and a bare filter record nothing.

A pick answers the query it was made under and nothing after it. Type on and the box holds an
unanswered query again, which is the ordinary way this gets used — you found one conversation and
went looking for the next — so quitting there records that second search as abandoned.

**The scripted runs are excluded at the time rather than afterwards.** `--measure` and `--shot`
quit with their last phrase still in the box and nothing opened, which is precisely the shape of a
person giving up, so each would otherwise append a need nobody had to a file that cannot be
rebuilt from anything. Both run every `cs` with `CS_LOG_QUERIES=0` and say so in their output.
That is ADR 22's convenience, set by the flag that made the run scripted rather than by somebody
remembering to export it — and it is why nothing downstream has to tell a benchmark from a need,
which ADR 22 is clear nothing can.

## The menu bar

This executable creates `NSApplication` by hand and never built an `NSMenu`, so until
`chat-search-me9.8.21` there was no menu bar at all — not an empty one, none. macOS delivers a key
equivalent through a menu and through nothing else, which is why Cmd-Q did nothing in an app that
plainly knew how to quit, and why Cmd-C did nothing in an entirely ordinary text field until
`chat-search-me9.8.24`. Neither the app nor the field was ever broken. The bar was absent.

**One rule decides what is on it**, and it is `chat-search-me9.8.21`'s argument for Quit carried to
the end rather than a template copied: an item is here because a key would otherwise reach nothing.
That has two halves, and while every item on the bar was AppKit's only the first was visible —
*a key somebody already presses is dead without it* (Quit, and then all of Edit), or *this app can
do a thing the keyboard cannot reach at all* (the drawer, below).

```
chat-search                 Edit                      View                 Window
  About chat-search           Undo             ⌘Z       Hide Timeline ⌘T     Close      ⌘W
  Settings…            ⌘,     Redo            ⇧⌘Z                            Minimize   ⌘M
  Quit chat-search     ⌘Q     Cut              ⌘X
                              Copy             ⌘C
                              Paste            ⌘V
                              Select All       ⌘A
```

**Edit is the one with a use in the ordinary path.** The search box is where you paste a path or a
phrase you copied out of a terminal, and `dir:chat-search agent:codex` is a grammar people paste
rather than retype. Every action on it goes to `nil` and up the responder chain, which is the whole
mechanism: in the query box AppKit hands `copy:` to the window's field editor, an `NSTextView` that
has implemented all of these since before this app existed, and in the reader to whatever
`textSelection(.enabled)` puts behind the transcript. Nothing here implements a clipboard, and after
this menu nothing has to — which is why the fix is a file about menus and not a file about text.

**Undo is on it because Paste is.** Cmd-V into a box with a phrase already in it destroys the
phrase, and the key that gets it back is the one people reach for without looking. The stack is the
field editor's, so it costs nothing to maintain and is wrong only if the field editor is.

**View is the rule's other half, and the first item here that is this app's own act.** Everything
above resolves into AppKit — a text system, an application, a window — where Cmd-T calls the same
method the drawer's own `hide` button calls. It is on the bar because `chat-search-me9.8.20` closed
saying the drawer could not have a key: this window has one focused view, the query box, so a key
bound beside it is a key the box stops getting, and the fold escaped that only because Enter already
belonged to the list rather than to the field. A menu resolves its key equivalents before the
responder chain is consulted, which is what makes it the one place a key can be added here without
taking it off something. That argument was not available when the drawer shipped, because there was
no bar; it is the same shape as Cmd-C, and it arrived the same way.

**Cmd-T is free in this app in a way it is not in most.** The platform spends it on New Tab and on
Show Fonts; there are no tabs here (nothing implements `newWindowForTab:`, and `--clipboard`'s
listing of the bar confirms AppKit added no tab items of its own — only a `Close All` alternate
beside Cmd-W), and nothing in a one-line query box wants a font panel. **The title is a verb and it
flips**, which is Finder's arrangement for the status bar. `MainMenu` holds no model on purpose, so
`AppHost.validateMenuItem` writes *Hide* or *Show* at validation — the callback that runs both
before the menu is drawn and before the key fires, so the verb cannot be read stale.

**Window is what the app owes its second window.** Cmd-comma opens the settings panel and until now
only the mouse could dismiss it — a window reached by a key and left by a click, which is the same
asymmetry as a text field you can type into and not copy out of, one window over. `performClose:`
goes to the key window, so Cmd-W shuts the panel when the panel is in front and the app when the app
is, which on the main window is exactly what its close button already does.

**What is not on it, each for its own reason.** An item that greys out the moment you look at it
says this app has a capability it does not have, and says it in the one place people go to find out.

| left off | why |
| --- | --- |
| Delete | the key already deletes, through the field editor's own bindings; the item is a mouse route to a keyboard act |
| Paste and Match Style | there is no style in a plain query and nothing here is rich text |
| Find | Cmd-F belongs to a document you search *inside*, and this window **is** a search — the box takes focus at launch and takes it back on any click that loses it, so the key would either restate the state the app is in or bind a second, narrower search beside the real one |
| spelling, substitutions, transformations, speech | a query is a grammar (`agent:`, `after:`, a quoted `dir:`), and the machinery that autocorrects prose is the machinery that would quietly rewrite one |
| Zoom, Bring All to Front | no key equivalent between them, which puts them outside the rule — the green button is the affordance for one and there is nothing for the other to gather |
| the window list | what `NSApp.windowsMenu` would fill in, and its job is finding a window lost behind others; there are two here and Cmd-comma already raises the second |
| Hide Others, Show All | both act on *other* applications, so neither is a key this app owes anybody, and Show All has no key equivalent to deliver |
| the grouping axis | it passes the rule — the four chips are mouse-only in exactly the way the drawer was — and it is left off anyway, because an axis is four states rather than a toggle, and four items with four keys is an argument worth making on its own rather than smuggling in beside one. `chat-search-me9.8.40` |

**And Hide, which was on this menu until the pass measured it.** Cmd-H is the most reflexive key on
this platform after Cmd-Q, so it arrived on exactly the argument that carried Quit. Then
`--clipboard` read the bar back: AppKit validates `hide:` as **disabled** in this process, with an
explicit `NSApp` target and without one, while `NSApp.hide(nil)` called directly hides the app
perfectly well. The capability is there; the menu route is the part AppKit refuses. The suspect is
the one already on record for the defaults domain being named after the executable — this is a plain
binary and not a bundle — but the reading governs either way, and a permanently grey item advertises
a key that will never fire. So Cmd-H is now the *negative control*: the pass presses it, gets
`matched false`, and that is what every key above it looked like before this bead.

**The bottom of the Edit menu is not this app's, and there is no version of this where it is.** The
moment a menu is titled `Edit`, AppKit inserts Writing Tools, AutoFill, Start Dictation and Emoji &
Symbols into it. Two of those have documented switches — `NSDisabledDictationMenuItem` and
`NSDisabledCharacterPaletteMenuItem`, both verified to work here — and two do not, so throwing them
would buy a menu that is still the system's at the bottom while taking away two working ways of
*entering* text into a search box. They are left alone and printed beside the six items this app
authored, so what is here on purpose stays legible and what the platform added is visible rather
than quietly attributed to this app.

### `--clipboard`, the one run that takes the front

```bash
swift run -c release chat-search --clipboard --config /tmp/scratch-config.toml
```

There is no picture of this: a box reading `borrow checker` looks the same however the text arrived.
So the keys are pressed the way AppKit presses them — on `NSApp.mainMenu`, through
`performKeyEquivalent` — and the field editor and the model are read back after each one.

It is a mode rather than a section of `--shot`, and the reason is the subject of the check. A key
equivalent is resolved against the key window's first responder, and an inactive application has no
key window, so a pass that presses keys has to be frontmost — which is exactly what `--measure` and
`--shot` must *not* be, since a latency taken in a frontmost app and one taken in a background app
are not the same measurement, and §1 was taken the second way. Measured rather than assumed: under
`--shot`'s `.accessory` policy every item on this bar validates grey and every press moves nothing
while still reporting that the menu matched it.

**That failure looks exactly like the bug being checked for**, which is why the front is reclaimed
before each press and the reclaims are counted — a build finishing or a terminal being scripted
takes key status away, and from that moment the run is measuring the background. On the run below it
was taken back twice.

```
the menu with an empty box:
  Edit: Undo [grey], Redo [grey], Cut [grey], Copy [grey], Paste, Select All, Writing Tools, …
  View: Hide Timeline
⌘A → matched true, selected 27 of 27 characters
⌘C → field "dir:chat-search agent:codex" · query "…" · pasteboard "dir:chat-search agent:codex"
⌘X → field "" · query "" · pasteboard "dir:chat-search agent:codex"
⌘V → field "dir:chat-search agent:codex" · query "…" · pasteboard "…"
⌘Z → field "" · query ""
⇧⌘Z → field "dir:chat-search agent:codex" · query "…"
the menu with the phrase selected:
  Edit: Undo, Redo Cut, Cut, Copy, Paste, Select All, …
  View: Hide Timeline
⌘H, which this bar deliberately does not carry → matched false, the app is hidden: false
  NSApp.hide(nil) called directly → the app is hidden: true
⌘M → matched true, the window is in the Dock: true
⌘T → matched true, the drawer is open: false, the item now reads "Show Timeline"
⌘T again → matched true, the drawer is open: true, the item now reads "Hide Timeline"
```

**Cmd-T is pressed twice**, and that is the shape of a toggle rather than thoroughness: one press
proves a key matched an item and shut a drawer, and cannot tell that from a drawer that was already
shut. The second press is what says it is a switch. It also puts the drawer back the way the run
found it, which is the courtesy the pasteboard gets two paragraphs down. The title beside each is
read after asking the menu to validate, since the verb is written at validation and reading it
without asking would report the one the bar was built with.

The bar is read **twice**, with an empty box and with the phrase selected, because half these items
are *supposed* to be grey in the first reading — a Copy that offered itself with nothing selected
would be the same lie in the other direction. The field and the query are both printed because they
are two facts: an edit the field editor performed and SwiftUI never heard about would leave the box
reading one thing and the search answering another, which is the way this can be wrong while looking
right.

**It puts the pasteboard back**, and it does it *before* Cmd-W rather than on the way out. A scripted
run that ate somebody's clipboard would be the same mistake as one that appended to `queries.jsonl`
— a benchmark writing over something a person put there on purpose (ADR 22) — and the first version
of this made exactly that mistake, because `NSApp.terminate` unwinds no scope and the restore lived
in a `defer`. What comes back is the string, so a run that interrupts a copied image or a promised
file leaves the string standing in its place; the general pasteboard cannot be snapshotted whole.

The run ends by pressing **Cmd-W**, for the reason `--shot --settings` ends on Cmd-Q: closing this
window is what quits this app, so if the key reaches the item the process ends there and the line
after it is only ever printed when it did not.

**What it cannot reach is the transcript.** A selection there is made by dragging; posting synthetic
mouse events would need the Accessibility grant this app has never asked anybody for, and calling
into the view directly is the same wall the fold pass names from the other side —
[what this cannot drive is the pointer](#seeing-it). With nothing selected `copy:` correctly resolves
nowhere, so there is not even a negative to report. That half is **reasoned rather than read**, and
the run says so on the line: it is this same item on this same chain, and `copy:` against `nil` is
what SwiftUI's own `TextEditingCommands` installs.

## The theme seam

No view names a colour, a size or a face. Every one is a token read off the environment, and the
values for every direction the app carries live in exactly one generated file:

```bash
python3 poc/ui/tokens.py terminal paper blueprint ink gruvbox-derived solarized-derived \
    -o apps/macos/Sources/CsTheme/Tokens.swift
cd apps/macos && swift run -c release chat-search --verify-theme
```

`Tokens.swift` is generated from `poc/ui/styles.css` and `poc/ui/directions.css` — the same files
the prototype renders, read through the same cascade `palette.py --verify` reads them through —
so there is one authored copy of these palettes rather than two that agree until they don't. It is
checked in because the app must build from a checkout with no Python in it, and it is provenance
rather than a dependency: nothing in `apps/` reads `poc/` at build or at run time.

### Six directions in one build, and `--theme` picks one

The first name on that command line is what the app draws when nobody has chosen; the rest it
carries and offers. **The list is generated as well as the values**, and that is the part worth
defending. The other shape was a file per direction plus a hand-written file binding them together,
and the binding is the one piece that has to agree with the set — so it is the piece that rots.
Generated, a direction that is compiled in *is* a direction the app can draw and *is* a direction
the gate measures, with no second list anywhere to disagree.

Switching is not a rebuild, and was never a view change. The whole of it on the app's side is one
`.environment(\.theme, theme)` at the root, because `Theme` is a plain value behind an environment
key with a default — which is what the seam was put in before the views to buy. Demonstrated rather
than asserted, six pictures out of one binary:

```bash
for d in terminal paper blueprint ink gruvbox-derived solarized-derived; do
  swift run -c release chat-search --shot --theme $d --out /tmp/theme-$d.png
done
```

`paper` comes out set in a serif reading face on warm stock with a blue selection where `terminal`
is sans on slate with a teal one, and nothing under `Sources/ChatSearch` differs between the two
runs.

### Two axes: an appearance, and a direction per side

A direction says what both sides look like; the appearance says which side you are looking at.
Until `chat-search-me9.8.22` there was only the first and macOS decided the second, so "GitHub
Light in the day and Solarized Dark at night" was not a sentence this app could hear.

```bash
swift run -c release chat-search --appearance light      # light, whatever macOS is doing
swift run -c release chat-search --theme-light paper --theme-dark terminal
```

**The appearance is one line — `NSApp.appearance` — and that is the whole mechanism.** Every colour
is a dynamic `NSColor` whose provider is called with the appearance in force *where the colour is
being drawn* (`Theme.dynamic`), so an override at the application reaches every token with no
palette re-resolve, no view change, and no second code path for "the app is forcing dark". `nil` is
`system`, which is a thing somebody chooses rather than the absence of a choice — the difference
shows the moment it has to be written down.

**A side override is colour and nothing else.** `--theme-light paper` puts paper's light palette on
the light side and leaves the type scale and the row metrics alone; those come from `--theme`, once,
for both sides. That is a decision and not a limitation of the seam, and the reason is the flip
nobody is present for: macOS switches at sunset, possibly mid-read, and colour changing then is the
point of the setting where the reading measure changing under someone's eyes is a bug.
`poc/ui/DESIGN-BRIEF.md` names rows-per-screen first on its list of what would actually break this.
The cost avoided is a real number rather than a worry — the corpus's 923-message conversation in the
default 900×620 window, `--shot` reporting the same drawer twice:

| | document | on screen at rest |
| --- | ---: | --- |
| paper's light colours in terminal's metrics | 14,486.7 pt | messages 179–192, 10 rows |
| the whole of paper | 14,613.6 pt | messages 177–190, 9 rows |

So a whole-direction flip would move the document 127 pt and change which messages are in front of
you, at sunset, with nobody watching.

The alternative is recorded rather than dismissed: a whole direction per side is more expressive and
more honest about what a direction *is* — paper's serif reading face by day as well as its stock —
and it is one line in `Theme.composed` on the day it is chosen. What it would owe first is an answer
to what happens to scroll position across the flip.

**Mixing cannot produce an unmeasured palette.** `ThemeCheck` fences each side on its own and names
the side in every failure it prints, so a light half and a dark half taken from two fenced
directions are each already measured: the mix is closed under the fence, and `--verify-theme` does
not have to know that mixing exists.

**And the override is probed rather than assumed.** An `NSApp.appearance` that reached only the
window's own chrome would draw a dark frame around a light window, and a picture of that reads
exactly like a picture of a light theme — so `--shot` and `--measure` print what the app's *own view
tree* resolved:

```
appearance: light → the view tree draws in NSAppearanceNameAqua, --bg #f6f1e6,
which is the light side of terminal with paper's light.
```

Taken on a Mac in dark mode, which is what makes it evidence rather than a restatement: `#f6f1e6` is
`paper`'s light `--bg`, sampled back out of the same token a view would have asked for.

```bash
for a in system light dark; do
  swift run -c release chat-search --shot --appearance $a \
      --theme-light paper --theme-dark terminal --out /tmp/theme-$a.png
done
```

### Where the choice lives

All four stick, which is what separates them from `--size` and `--group`. They stick in
`~/Library/Preferences/chat-search.plist`:

| key | what it holds |
| --- | --- |
| `theme` | the direction: both sides' colours unless a side says otherwise, and both sides' type and geometry always |
| `theme-light` | a direction whose light *colours* the light side wears, and nothing else of it |
| `theme-dark` | the same for the dark side |
| `appearance` | `system`, `light` or `dark` |

```bash
defaults read chat-search            # what the next launch will draw
defaults delete chat-search theme    # back to the direction the build ships
```

`--theme` writes the first key and clears the two side keys, because "this direction on both sides"
is what it says and a leftover override would contradict it.

**A `theme` written before this build is read, not converted.** It meant one direction on both
sides, following the system, which is exactly what it still means here — so nothing is rewritten,
and what is owed is the sentence, said once on stderr with the flag that records the reading and
stops it:

```
theme: paper on both sides, appearance following the system — that is what a `theme` set before
this build means. … `--appearance system` records this reading so this line stops.
```

Converting it would be this app changing a preference nobody asked it to change, at launch, which is
the thing the scripted-run rule below exists to prevent.

`UserDefaults` without a bundle was probed rather than assumed: a non-bundled executable gets a
defaults domain named after the executable, and the value is written and read back across runs. The
catch worth knowing is that the domain **is** the executable name, so the day this app grows a
bundle identifier the preference moves and the old one is orphaned rather than migrated. That costs
one re-pick of a theme, which is why it did not buy a hand-rolled file under
`~/.config/chat-search/`: that directory is `cs`'s, this is the client's own state, and a plist is
inspectable with `defaults read` where a new dotfile format would be inspectable with nothing.

Two rules around all four, both of which exist to stop a flag lying. A name this build does not
carry never quietly becomes another one — on a flag or in the plist, per side or for the appearance
— it says so on stderr and leaves the previous answer standing, because a flag that silently does
nothing is the failure this is most likely to have. And a scripted run writes none of the four:
`--measure` and `--shot` draw in whatever direction and appearance a script names them, and neither
is somebody choosing a theme. Same rule and the same reason as both staying out of the query log.

A flag is still the affordance a script reaches for, and a terminal is still this app's front door —
no bundle, no Dock icon. But a flag is no longer the *only* way in, because
[the menu bar](#the-menu-bar) it was waiting on now exists.

### The settings window at Cmd-comma

`Settings…` is the one item on that bar that is this app's rather than AppKit's, and the reason
`chat-search-me9.8.21` had to build a menu bar at all: there was nowhere to hang it. Cmd-comma opens
a window carrying the three settings above:

```
Appearance   ( ) System   (•) Light   ( ) Dark
             Drawing the light side.
Light theme  [ paper · direction     v ]
Dark theme   [ terminal · direction  v ]
             Colour only — the type scale and the spacing come from paper, on both sides.
```

A window and not a `View > Theme` submenu, because a submenu with one checkmark can express one
list, and this is three settings where two of them are lists and the third governs which list you
are currently looking at. The caption under the appearance says which side is *actually* on screen
rather than which one was asked for — under `system` the setting does not know and the view does,
which is the same reading `--shot`'s probe takes and for the same reason.

Each direction menu names the class beside the direction, which is what `chat-search-me9.8.12` built
the class for: a build can carry a direction read off disk (`chat-search-me9.8.10`), and a menu that
lists a shipped direction and a user theme without distinguishing them is telling you the two are
equally fenced when ADR 25 says only one of them is.

**Preview on selection, not on confirm.** There is no OK button and nothing to apply — the app
redraws under the window as each control moves, which is the only preview worth having, and the
whole of it is one observed value going back into `\.theme`.

**And both menus go quiet while a token set off disk is in force**, with the caption saying which
file and that they are choosing what comes back when it is gone. Disabled rather than absent, and
disabled rather than listing the user theme as a seventh entry: a user theme is drawn *whole* (ADR
25 rule 3), so it is not a thing a side can be, and a menu offering it per side would offer exactly
the half-a-palette-from-each merge the rule forbids. See [a token set off
disk](#a-token-set-off-disk).

**Both menus on one direction means that direction whole.** There are four settings and three
controls: nothing on the window stands for where the type scale and the geometry come from. So the
two side menus agreeing is read as `--theme NAME` — that direction on both sides, side keys cleared
— and menus that disagree write the two side keys and leave the layout direction exactly as it was.
Without that rule, picking `paper` in both menus would draw paper's colours in `terminal`'s metrics
while `--theme paper` draws the serif face, and the same choice said two ways would give two
results. The caption under the menus says which direction it currently is, because a setting that
changes the window and appears nowhere on it is worse than a fourth control.

**The window is drawn in stock AppKit controls**, which is the one view in this app that does not
read its colours off `\.theme`. Two reasons: the preview is the window *behind* this one, so a panel
that also repainted itself would put the sample beside the swatch and make neither readable; and a
radio group painted in a direction's tokens is a theme authoring a control, which is the direction
this surface is under the most pressure to drift in. What it must not become is the rest of that
list — a colour well, a slider on a token, a "customise…" button. `docs/DECISIONS.md` ADR 25 turns
on the app not being the author of a theme, and selecting a direction is selecting. A token that
needs dialling is `chat-search-me9.8.10`'s file-off-disk route, where the authoring happens in a file
you own and the result is still measured on load.

**And it is checkable from a script**, which a window normally is not:

```bash
swift run -c release chat-search --settings                    # open it at launch
swift run -c release chat-search --shot --settings --out /tmp/s.png
```

`--shot --settings` presses Cmd-comma through `NSMenu.performKeyEquivalent` — the call AppKit makes
for a real keystroke — then moves all three controls and reads the app's own view tree back after
each one, photographs both windows at each appearance, checks that a scripted run wrote none of the
four keys, exercises the writes against a scratch defaults domain, and quits by pressing Cmd-Q. The
run ending is the evidence for that last one: reaching the line after it is the failure.

### Why `--verify-theme` and not a test

There are no tests to put it in. This package builds against the Command Line Tools SDK, where
neither `Testing` nor `XCTest` exists, so `swift test` cannot run at all — the same reason
`cs-spike contract` is a subcommand. It re-measures **every direction the build carries**, in both
themes: the kind ramp at 2.2 / 4.0 / 7.2 / 13.0 against `--map-bg` with even ~1.8× steps, the three
act shades ordered inside the tool band, and every text tier against the 4.5:1 AA floor on **both**
grounds it lands on. That last one is not pedantry: `--ink-3` was fixed once against `--bg` and was
still at 4.23:1 on `--panel`, which is where most of that text actually is.

Every direction and not just the default, because measuring only the default would fence the one
palette least likely to be wrong — it is the one somebody is looking at — and leave the other five
offered and checked by nobody. One miss anywhere fails the run, and the verdict names which
direction missed: "FAILED" over six palettes says nothing about which one to go and re-solve.

**Every direction, and nothing outside the binary.** A file at `~/.config/chat-search/theme.css`
is never part of this run, because a gate that read `$HOME` would pass or fail on whose machine it
ran. `--theme-file X --verify-theme` measures that file instead and is a different question —
somebody checking a candidate before they load it — so it prints the same table with `user theme`
beside the name and `UNFENCED` instead of `FAILED`. The status reports the readings and never the
policy: 1 for a token set that missed, 2 for a file that is not a token set at all.

It exists for the reason `palette.py --verify` exists. Solving a colour and writing it down are
two events, and generating adds a third — so what ships is now two steps from what was solved,
and neither step is checked by anything that reads Swift.

### Adding a theme — Solarized, Gruvbox, one of your own

A theme is not a list of hexes here. Half of one is *solved*: the four message kinds have to sit
on an even luminance ramp against the ribbon track, because hue is the channel that degrades
fastest at the ~2px those bands are drawn at, and the quiet tier has to clear 4.5:1 at 10–12px.
So the path for a palette is to add its hues to `DIRECTIONS` in `poc/ui/palette.py`, let that
solve the eight fenced tokens, write the rest into `directions.css`, and name it on the `tokens.py`
line above — at which point the app offers it to `--theme` and the gate measures it, both because
it is in the generated list and for no other reason. A theme that skips the solve will fail
`--verify-theme`, which is the check doing its job rather than being in the way.

**Which is a real cost, because the published palettes miss.** Solarized's kinds read
2.79 / 3.43 / 4.75 / 5.61 against `base03` where the ramp asks for 2.20 / 4.00 / 7.20 / 13.00, and
`base01` — the tier Solarized itself designates for secondary content — is 2.42:1 on `base02`,
under half the AA floor. That is what those palettes *are*, not a mistake in porting them: every
assignment of Solarized's sixteen published colours was measured and the most even ramp available
is 1.46× 1.38× 2.18×, because nothing sits between `base1` at 5.61 and `base2` at 12.25. Gruvbox
comes far closer — its dark side has an even ramp in `bg3 gray green fg0` at 1.77× 1.78× 1.82×,
sitting ~1.11× above the targets — and its light side is 0.09 out.

**So a theme belongs to one of two classes, and the class decides what a miss costs**
(docs/DECISIONS.md ADR 25). A **direction** is compiled in and offered by the app: fenced, and one
that misses does not ship. A **user theme** is one you load off your own disk: measured by the same
code, and then drawn whatever the readings say, with the misses on stderr and the class beside the
name. `--verify-theme` prints the class beside every name it measures, and its last line says which
of those two consequences applies.

That leaves three routes for a named palette, and none of them pretends to be another: load it as
a user theme exactly as published; ship it re-solved through `palette.py`, with `-derived` in the
name; or find a palette that holds as authored, which neither of these two is.

#### The two that took the middle route

Both ship, both are fenced, and `chat-search-me9.8.17` is the port. Every hue in them is Gruvbox's
or Solarized's; not one lightness is. What each cost, measured rather than estimated — the ramp is
what `--verify-theme` reads back out of the build, and the move is how far the worst of the eight
solved tokens travelled in HSL lightness against the value its palette publishes:

| | ramp on the track, dark | ramp on the track, light | worst token moved | port cost |
| --- | --- | --- | --- | ---: |
| `gruvbox-derived` | 2.21 4.01 7.20 13.05 · 1.81 1.80 1.81× | 2.21 4.00 7.27 13.13 · 1.81 1.82 1.81× | −11.4 pt dark, −13.3 pt light | 105 CSS + 95 Swift |
| `solarized-derived` | 2.22 4.01 7.25 13.00 · 1.81 1.81 1.79× | 2.21 4.01 7.27 13.10 · 1.81 1.82 1.80× | **+26.7 pt** dark, −21.6 pt light | 105 CSS + 95 Swift |

The last column is the whole port as `git diff --shortstat` reports it, which is the claim the
generated seam has been making all along: `3 files changed, 479 insertions(+), 7 deletions(-)`
across `palette.py`, `directions.css` and `Tokens.swift`, and the 95 Swift lines a direction costs
are the same 95 `terminal` costs, because nobody wrote them. No view, no list and no binding was
touched.

**Gruvbox is a nudge and Solarized is a rebuild, and the middle column is where that shows.** Every
Gruvbox band lands within 14 points of the lightness Gruvbox publishes, and its three dark grounds —
`bg0_s`, `bg0`, `bg0_h` — go in unaltered as the page, the drawer and the track. Solarized's
brightest kind travels 27 points, from `base1` at L 60% to L 87%, which is brighter than any colour
Solarized puts on a dark ground; its designated secondary tier travels 22 points the other way,
because 2.42:1 is under half the floor; and its ribbon track had to be invented, since the darkest
colour it publishes is the page itself.

**Two-thirds of a theme is neither published nor solved, and that is where the work was.** A theme
is 30 colour tokens per side against Gruvbox's 19 and Solarized's 16, so the panels, the rules, the
selection and match grounds and five source hues are invented either way. They are invented from
each palette's own greys and accents at that palette's hues — taking them from `terminal` is how a
port ends up reading as the incumbent wearing a costume. `directions.css` names the published ones
in a trailing comment and marks the solved eight, so what was taken, what was computed and what was
made up are told apart in the file rather than in a commit message. The one place Solarized needed
an extra invention is a middle foreground tier per side: it publishes two per ground where this
interface reads three, and its comments tier solved to the floor comes back brighter than its own
body text.

Neither sets a type, radius or rhythm token. A colour port cannot move rows-per-screen, so the
density argument every other direction has to make does not arise, and `--shot --theme
gruvbox-derived` differs from `--shot --theme terminal` in colour and in nothing else.

### A token set off disk

Everything above is compiled in. Dialling the look in was therefore edit `styles.css`, run
`tokens.py`, `swift build`, relaunch — a loop, which is more than there was before the seam, but a
compile per iteration on exactly the values somebody wants to nudge twenty times in an afternoon.
A file makes it edit and relaunch:

```bash
chat-search --write-theme                       # ~/.config/chat-search/theme.css, from the
                                                # direction this launch would have drawn
$EDITOR ~/.config/chat-search/theme.css         # --fs-body: 12.5px → 13px
chat-search                                     # it is drawn
chat-search --theme-file /tmp/candidate.css --verify-theme    # the readings, before loading it
chat-search --no-theme-file                     # the shipped direction, file left where it is
```

**It is CSS custom properties, and that was the decision.** TOML would have matched `cs`'s own
config and that is the whole of its case. These values are *already* authored as custom properties
— in `poc/ui/styles.css`, in `poc/ui/directions.css`, in `ColorToken`'s raw values — so CSS is the
one syntax where a line moves between the mockup, the generator and this file without being
rewritten, and where the name in an error message is the name in all three. TOML would have needed
a table of its own keys against these, which is the translation table the raw values exist to
avoid. What it costs is a scanner in `ThemeFile.swift`, and that is bounded because it is not a CSS
engine: `:root` is the dark side with the type scale and the spacing, `:root.light` is the light
side, and anything else is a sentence with a line number on it.

**The file has to be complete, and `--write-theme` is what makes that affordable.** ADR 25 rule 3
says a user theme is drawn as authored and entire, never merged with a direction to patch what
missed — half a palette from each is a palette nobody designed. So a file with a hole in it is not
a theme. Asking that of a hand-typed file would be cruel, so the app writes one: 82 declarations,
every value already solved by `palette.py` and compiled in. It refuses to overwrite, because the
file it would replace is somebody's afternoon.

**It is measured, and drawn either way.** That is ADR 25 and this bead decides none of it. The
readings come from `ThemeCheck` and not a second copy of the rules; the misses go to stderr as
whole sentences with no modal and no banner, because the only person who can be nagged here is the
one who wrote the file. What it costs when a palette is unfenced is bounded — nothing in this
client is encoded in colour alone — except the ribbon, which is 2px with no second channel, and is
the honest cost.

```
theme: nightshift · user theme, from /tmp/nightshift.css.
  nightshift, dark: quiet tier on the page is 1.95:1, under the 4.5 AA floor for text this size
  nightshift, dark: quiet tier on the drawer is 1.69:1, under the 4.5 AA floor for text this size
  Drawn anyway, because you loaded it and it is your screen — what that costs is
  docs/DECISIONS.md ADR 25. `--theme-file /tmp/nightshift.css --verify-theme` prints the whole table.
```

**Unreadable is not unfenced.** A file that will not parse, names a token this build has no name
for, gives a colour in `rgb()` or a length in `pt`, or is simply missing something, is not a theme
at all: the app draws the direction it would have drawn, starts normally, and says every reason it
found rather than the first, each with the line it was on. `Palette`'s precondition treats an
incomplete set as a programmer error — right for a generated file and fatal for a typed one — so
the loader validates before it constructs.

**A scripted run reads no file unless a flag names one.** `--shot`, `--measure` and `--clipboard`
draw what the script named them, and a frame that changed because of a file in somebody's home
directory is a frame of that home directory. Same rule and the same reason as their refusing to
write the four preference keys. `--theme-file PATH` *is* a script naming it, so that case loads.

**And it is not a fifth preference.** Nothing about it is remembered, because the file is the
memory — it is there or it is not. The direction and the two side overrides keep resolving
underneath it, so removing the file draws exactly what was on screen before it arrived.

Two things this seam still does **not** do, each filed. Picking a direction used to be the third —
the flags chose at launch and changing your mind while looking at the window did not — and [the
settings window](#the-settings-window-at-cmd-comma) is what closed it.

- **Nothing watches the file.** Load is at launch, so it is edit and *relaunch*. Watching it is a
  different set of problems — SwiftUI invalidation, partial writes, a half-saved file arriving as a
  theme — and `chat-search-me9.8.10` deliberately did the first half.
- **Padding is mostly still literal.** The row's rhythm is tokenised because a direction moves it
  and it trades against rows-per-screen. The search bar's, the banner's and the footer's are not —
  they are literal in `styles.css` too — so dialling those is still a view edit, file or no file.
  `chat-search-me9.8.11`.

## The reader

Select a row and the conversation opens beside it, from `cs show --json`. Every message that a
reader draws, with its band as a 3pt spine and its kind as a sigil; prose in the reading face and
tool traffic in the quiet monospaced tier; failed tool results kept and successful ones gone,
because the call already implies the result and the failure is often what makes a conversation the
one you were looking for.

**Four things about a conversation arrive on the wire and none of them is decided here.**

| the question | the field | why not in the client |
| --- | --- | --- |
| is this message drawn at all? | `drawn` | it was already worked out twice, in Rust and in the prototype's JavaScript |
| which band is it? | `band` | `system` prose is the agent's side, and a call and its result are one stretch — two decisions that are easy to get wrong and impossible to notice wrong |
| how does it fold **by default**? | `fold` | the fold is what makes a 900-message agent session legible; two clients folding differently is two different conversations. The reader may then move it — see [the four knobs](#the-four-knobs) — because what somebody has opened is session state and core says so |
| may a match claim it ranked? | `mark_kind` | a `reasoning` hit carries no postings, so marking it like a prose hit states something false in the one place a reader went to check |

The last one is drawn as a *form* rather than a hue — a filled `--hit-bg` ground for a match that
ranked, an underline for one that could not — for the reason the TUI spends a text modifier on it.
`--hit` and `--hit-bg` are one colour family, and a claim that consequential should not rest on
which shade of amber it happens to be.

The drawer opens on the first message that matched rather than at the top, anchored on the marks
and not on `match_seqs`: that list counts positions in one order and the transcript arrives in
another, so a position resolved against the wrong one lands on an unrelated message.

Three departures from `poc/ui`'s drawer:

- **The fold lives on the sigil, not on the text.** The prototype toggles by clicking the message
  because a prototype has nothing to select. A transcript you cannot copy out of is one you cannot
  quote, so the text stays selectable and the glyph is the affordance.
- **The drawer sits outside the empty-results case.** Typing on with a conversation open is
  ordinary — you have found it and are now looking for the next one — so a query that matches
  nothing empties the list and leaves the reader alone.
- **A collapsed message is truncated, not summarised.** `⚙ Bash ls -la` is a *form* core states,
  and it is not on the wire yet, so a collapsed tool call reads as its raw argument. Honest, and
  not pretty. `chat-search-me9.20`.

Not built, and for a reason rather than for time: the mockup's **work summary**, which needs facts
— topics, touched files — that nothing on the wire carries. The **minimap** was in that list until
`chat-search-me9.8.18` and the **fidelity chips** until `chat-search-me9.8.36`; both are below.

### The four knobs

One fidelity for a whole corpus is one fidelity too few. A 2,553-message Codex session and a
seven-message ChatGPT thread arrived on the same screen at the same density, and the only gesture
against that was opening messages one at a time — `poc/ui/NOTES.md` costs that at "211 toggles on a
211-message conversation".

`poc/ui` had already iterated the control five times with the rejected arrangements written down,
so `chat-search-me9.8.36` is a port and the paragraphs below are mostly its reasons rather than
new ones.

**Four knobs, over `cs_core::blocks::Band`.** You, agent, reasoning, tools — the same four the spine
beside the text is coloured by, the minimap encodes as width and `ThemeCheck` fences on a ~1.8×
luminance ramp. The vocabulary is not new, which is the point: nobody has to learn a fifth
categorisation to use the control, and `me9.41` re-keyed `Density` onto exactly these so that
`{ user: expanded, agent: collapsed }` became sayable at all — both are `prose`, so the old
kind-keyed table could not say it.

**Three levels: off, brief, full.** The wire has two. `hidden` is the client's, and stays the
client's, for the reason the fold override does: core answers how much of a message to show and
refuses to hold what a reader has done with the answer.

**Two axes on one chip, deliberately.** Hiding a band is "is it on screen"; brief against full is
"how much of it". Conflating them is why every arrangement of this control felt wrong — with a
single three-cycle, one of the six transitions always costs two clicks. So the chip's body cycles
off → brief → full → off, which is the path you actually walk (peek at the tools, read them, put
them away), and its dot toggles visibility outright and restores whatever detail that band last
had. Label, state and dot live in **one box**, because the 2×2 grid that preceded it put `you`'s
control nearer to `agent`'s *label* than `agent`'s own control was: proximity pointed at the wrong
thing on every read, and that was most of the fiddliness rather than the cycling. The state is a
word and not a glyph — `○ ◐ ●` is a legend you have to learn, on an 18pt target.

**Four presets, and no all-buttons.** There were three plus `expand all` and `collapse all`, of
which two were the same command — `outline` set every kind to collapsed and so did `collapse all` —
while `full` was *not* full, since it left reasoning and tools collapsed, and `expand all` was.

| preset | what it says | on the corpus's longest conversation, 1,479 drawn messages |
| --- | --- | --- |
| `segments` | runs summarised | 84 rows, 41 of them summaries · 43 full, 345 brief, 1,091 off |
| `outline` | one line per message | 1,479 rows · 0 full, 1,479 brief |
| `read` | prose in full, the rest brief | 1,479 rows · 388 full, 1,091 brief |
| `everything` | all of it | 1,479 rows · 1,479 full |

Two of those four are `Density`'s two named points spelled in Swift: `read` is `Density::Full` and
`outline` is `Density::Outline`. **That is the one rule this client spells twice**, so it is the one
place the two can drift, and `--shot` checks it rather than leaving it to a comment — it reports
`read` against the `fold` on every drawn block, 1,479 of 1,479 today. The other two have no
counterpart and cannot: `everything` needs a level `Density` has no reason to name, and `segments`
needs `hidden`, which is not a fold at all.

**The drawer opens at `read`, which is what the wire already said.** The prototype opens at
`segments` and then corrects per conversation with `defaultZoomFor`, and that is the one piece of
this model deliberately left behind: it is broken there, and its prose > 0.5 test is ~87% predicted
by the source badge already on the row — chatgpt 0.88, claude-code 0.30, codex 0.23 — so it is close
to a source rule wearing a content costume. Without it, `segments` on a seven-message ChatGPT thread
draws a summary line after every single turn, which is the defect `poc/ui/NOTES.md` names when it
says the segment fold "is for agentic runs and actively harms conversational ones". So the drawer
opens where the wire points and `segments` is one click away.

**Opening another conversation does not move the knobs.** They are the reader's, not the
conversation's; the per-message overrides and the open segments are the conversation's and go with
it. `--shot` drives that too, because the absence of `defaultZoomFor` is invisible in every frame.

#### Segments, and why this file is meant to be deleted

`Segment` is a steer and the run it caused, summarised as `→ 12 calls · 2 failed · asked you 1×` —
richer than a count, which is the difference between "something happened here" and "this is where
it went wrong". A closed segment carries a `●` when the query matched inside it, and the drawer
opens on that summary rather than on the message behind it, because a scroll to a row `List` does
not have is a scroll that silently does not happen.

**It is computed client-side, which is wrong, and it is filed.** The run rule belongs in core for
the reason every rule in this neighbourhood does — the row's ribbon and this transcript draw the
same conversation, and two derivations of "where does a run start" is the local-date bug's shape.
`chat-search-me9.45` is that bead; when it lands, `Segments.swift` goes and the reader reads the
answer. Until then, three things in it are this client's guesses and are labelled as such in the
source: a run breaks on a user turn *and on a change of thread* (without the second, every subagent
lands in whatever segment happened to be open, and where sidechains appear at all they average 52%
of the conversation); calls are counted off `band` rather than `kind`, which works only because a
successful `tool_result` is never drawn, so the drawn tool traffic is the calls plus the failures;
and "asked you" is agent prose ending in a question mark, which is 4.6% of assistant prose in this
corpus.

#### What the knobs cost the minimap

The map's scrub targets used to be the messages core draws. A band switched off leaves rows `List`
no longer has, so a drag would have resolved onto a message and scrolled nowhere — silently, which
is the worst way for a gesture to fail. `MinimapLayout` now takes the ids that have a row *right
now*, which is also what dims the bands the knobs took away: dim rather than drop, so the scrollbar
goes on describing the whole conversation while a knob takes two thirds of it off the screen.

That merges two dims into one. A successful tool result and a band you switched off are drawn at
the same 0.22, because they say the same thing to somebody looking at the map — there is a message
here and you are not being shown it.

### The marked text, built once rather than once a frame

`BlockRow.marked` was a computed property on a `View`, so every body evaluation cut the whole
message into runs and concatenated a fresh `AttributedString` of it. That is the shape
`cs_core::blocks` refused one layer down — marks are held on the block rather than located at
render time "because locating them means tokenizing the message … and a renderer runs on every
frame" — so core paid once and the client reintroduced the cost a layer up (`chat-search-me9.8.29`).

`MarkedText` keeps one entry per message, replaced rather than duplicated when its fold toggles,
and thrown away whole when the conversation, the terms it was marked against, or the direction
drawing it changes. It hangs off `ReaderModel` beside the folds rather than off the view, for the
reason the folds are there. **Appearance is deliberately not part of the key**: every token is one
dynamic `NSColor` that picks its own side where it is drawn, so a light/dark flip repaints these
without rebuilding any of them, and only a change of *direction* has to.

**The clock cannot check this and two counters can**, which is why `--shot` prints them. `builds`
is what was assembled, `reuses` is what was handed back already assembled, and their sum is the
number of times a row asked — so the second number is work this run did not do, counted rather
than inferred off a percentile. On the run below:

| | messages built | evaluations | answered from the table |
| --- | ---: | ---: | ---: |
| after the fling | 401 | 1,112 | 711 — 64% |
| by the end of the pass | 1,325 | 7,603 | 6,278 — 83% |

The fling is close to the table's worst case, which is why its share is the lower of the two: it
travels 319pt every 8 ms in one direction, so nearly every row it prepares is one nobody has drawn
yet. The drag and the keyboard steps are where a reader actually revisits messages, and they are
where the 83% comes from.

**The frame statistics do not separate**, and saying so is the point of printing counts instead.
Five alternating pairs — 2026-08-06, live index, the corpus's longest conversation at 2,431
messages on the head path, window 1100×760, `--theme terminal --appearance dark` pinned so neither
arm inherits a remembered direction, driven half a document in 60 steps:

| | main-thread lag p50 | p95 | max | vsyncs missed | footprint |
| --- | ---: | ---: | ---: | --- | --- |
| before | 0.6–0.7 ms | 22.2–29.1 | 29.4–431.0 | 4–8 of ~66 | 97.7–122.0 MB |
| after | 0.6–0.7 ms | 4.2–25.9 | 9.0–418.6 | 0–6 of ~67 | 98.2–125.3 MB |

p95 and the missed-vsync count are better in four of the five pairs and the ranges overlap in
both, which on this machine is the same reading the minimap comparison below arrived at: it cannot
be told apart from what else the laptop was doing. The footprint is the answer to the obvious
objection — a table of 1,325 `AttributedString`s over a conversation whose text is already
resident costs nothing this can measure.

Two things worth knowing before taking these again. **Both arms need the same executable name**:
`UserDefaults` keys off it (see "Where the choice lives"), so a build copied to `/tmp/before` reads
an empty preference domain and draws a different theme than the one beside it — the first attempt
at this compared `terminal` against `terminal with paper's light`. And **the pictures are the check
on "nothing moved"**: across the pair, every frame is identical outside a 162×16 px patch of the
footer holding the query's own millisecond readout, which differs by as much between two runs of
one build.

### The minimap

Beside the transcript, the whole conversation as a column. Every message on the head path is a
band: its height is `log10` of the message's length normalised over the conversation, its colour is
the same band the spine beside the text uses, and its width is the role — 18pt for a user turn
against 28pt for everything else. `poc/ui/app.js` is the reference and three of its rules are the
reason it works at all.

**Height is length, not one row per message.** A 2,431-message agent session has to fit a drawer,
and equal-height bands make a transcript of mostly short tool calls look like one of mostly prose.

**Dim rather than drop.** The 952 successful tool results the reader folds away are on the map at
0.22, because a map that omitted what the fold is hiding would be a map of the current view, and
there is already one of those — it is the screen. Off-path messages get 0.3 for the same reason,
and never appear today: `cs show` returns the head path only.

**A match is a tick over the band, not a recolour of it**, so a hit on a tool call still reads as a
tool call.

Two departures, both forced by density rather than chosen. **Paint order puts user turns and
failures last**: at 2,431 messages in a 500pt column the bands overlap, so in transcript order every
pixel goes to tool traffic — 2,043 of those messages — and the 43 user turns vanish under it. The
width encoding is what makes it safe, since an 18pt turn painted over a 28pt band leaves the band
showing beside it. And **a tick is half width when its match cannot claim to have ranked**, because
at 2.5pt there is no room for the second colour the transcript spends on that distinction, and
drawing an unranked hit exactly like a ranked one would state in the map what the text refuses to
say.

#### The container question, and what route 1 cost

Against macOS 14, `List` gave back neither of the two numbers the prototype reads off the DOM —
`scrollTop` and `scrollHeight` — and `ScrollViewReader` only went the other way, and only to an id.
That was the whole problem, and `chat-search-me9.8.18` costed three answers to it. What ships is
the reversible one: **keep `List`, drive by id**. The rows report themselves, and a drag resolves
to a message rather than to an offset.

Raising the floor to macOS 15 (`chat-search-me9.8.27`) did not change that shape and did change
what the rows are able to say. [What the floor bought](#what-the-floor-bought) is the current
state; this section is the reasoning that got here and the numbers the container question was
settled on, both of which still stand.

So the reader did not leave `List` and `chat-search-me9.22`'s container numbers still stand. What
needed measuring was what the *relationship* costs, which is a number about this app and not about
containers. Taken against a build of this app with no minimap in it, alternating runs so that the
comparison is not a reading of what else the laptop was doing — 2026-08-05, live index, the corpus's
longest conversation at 2,431 messages on the head path, window 1100×760, driven half a document in
60 steps, which is 319pt every 8 ms and far past what a hand can produce:

| | main-thread lag p50 | p95 | max | vsyncs missed |
| --- | ---: | ---: | ---: | --- |
| with the minimap | 0.5–0.6 ms | 19.6–24.4 | 285–633 | 4–6 of ~69 |
| without it | 0.6–0.7 ms | 20.6–30.1 | 102–297 | 4–6 of ~68 |

p50, p95 and the missed-vsync count cannot be told apart. The maximum is worse with the map in
three of the four pairs and the same statistic swings 3× between runs of the *same* build, so this
machine at this load cannot separate it from its own noise — which is why the range is printed
rather than an average that would look settled. Footprint is the same story: 115–135 MB with the
map against 106–118 MB without, measured at the same point with `phys_footprint`, ranges that
overlap.

Two costs were not noise, and `chat-search-me9.8.18` recorded both as permanent against that floor:

| what it cost | why, against macOS 14 | now |
| --- | --- | --- |
| **The box was a report, not a measurement.** Drawn from which rows `List` had, and `NSTableView` prepares rows past the visible rectangle, so it said where you were and not what you could see. `--shot` sized it: after a drag to 75% the map asked for message 1812 and three keyboard steps later for 1818, and the box did not move for any of it. The transcript moved each time. | `List` published no scroll offset, so the rows self-reported through `onAppear`/`onDisappear` — which is a statement about *existence*, and changes only when a whole row crosses an edge. | **Fixed.** `onScrollGeometryChange` and per-row `onGeometryChange`, below. The same drag and the same three steps now move the box 74.93% → 75.16%. |
| **A drag landed on a message boundary**, because `ScrollViewReader` scrolls to an id. On this conversation a fifth of a point of slop; on a short one with tall blocks, the block. | Nothing took a pixel. | **Fixed where it was worth anything**, by an anchor rather than by an offset — a message that fits on screen has no interior to land in. Below. |

Both were listed as things route 2 — `ScrollView` + `LazyVStack` with `GeometryReader` — would
buy, at 65.6 MB against `List`'s 5.2 MB over the corpus. That trade was never taken and does not
need to be: the floor was the cheaper half of it.

One optimisation was measured and *removed*, and the argument still holds for the geometry that
replaced it. `onAppear` fires from inside `NSTableView`'s own row preparation, so the rows first
buffered into an unobserved set and published once per turn of the run loop, which is the standard
defence against re-entering an AppKit update. Against writing straight through it measured p95
16.8–18.4 ms versus 17.3–29.2 and produced the same two reentrancy warnings per run — the same two
a build with no minimap produces. SwiftUI already defers a dirty view's body to the next frame, so
it was a hand-rolled copy of the framework, and the simpler version is what is here.

The bands are a view of their own so that a scroll does not redraw them, and that one *is* worth
keeping: `--shot` reports **1–2 canvas renders** over a fling that moves the viewport box sixty
times. It prints the count for the reason `--verify-theme` exists — the day an innocent capture
puts `reader` back inside that view, the number reads in the hundreds instead of the claim quietly
becoming false.

#### What the floor bought

`Package.swift` declares macOS 15 (`chat-search-me9.8.27`), which is one release below what this
machine runs and the last one before Liquid Glass. Two APIs are why, and once either is called,
going back below the floor means `#available` guards:

**`onScrollGeometryChange(for:of:action:)` publishes a `List`'s document height and scroll offset**,
which is `scrollHeight` and `scrollTop` at last. `--shot` prints them beside AppKit's numbers for
the same `NSScrollView` — 39191.0 pt of document and 335.0 pt of viewport, from both — because the
box is drawn from the SwiftUI side and the fling is driven from the AppKit side, and a disagreement
would mean the box is measuring some other view.

**`onGeometryChange(for:of:action:)` per row** is the other half, and it is what actually moves the
box. A row reports its rectangle rather than its existence, so the visible span is arithmetic: the
messages whose rectangles overlap the list's own, plus the fraction of the first and last that the
edges cut through. That last part is the whole difference. `--shot` says it two ways on the corpus's
longest conversation:

```
box over the fling  0.00% → 10.98%, moved on 59 of 60 steps, largest single move 0.7056%
box over 20 × 6 pt  10.98% → 10.99%, moved on 20 of 20 steps, largest single move 0.0002%
```

The second line is the one that means anything. The fling moves a third of a viewport per step and
would move a box that could only sit on message boundaries too; twenty nudges of 6 pt are finer
than any row in that conversation, and the box tracks every one of them, by two ten-thousandths of
a percent at a time.

**The coordinate space is the window's, and that is measured rather than preferred.** Both
`.scrollView` and a `.coordinateSpace(.named:)` declared on the stack around the list put the top
row of a 352 pt viewport at y=150 — the chrome above the drawer — so neither resolves inside a
`List`'s rows, and under both, every row reports a positive `minY` and every fraction comes out
zero. That is the old bug wearing a new API, and it survived a build and a screenshot — the second
line above read `moved on 2 of 20 steps`, which is what gave it away, and is the reason that line
is printed rather than inferred from the first. So the rows and the list are measured in the one
space they agree on.

**A drag carries an anchor.** `scrollTo(id:anchor:)` aligns a row's anchor point with the
viewport's, so a row of height *h* in a viewport of height *H* lands at `rowTop + a(h - H)`, and
putting fraction *f* of the row at the top wants `a = f·h / (h - H)`. That is inside `0...1`
exactly when the message is taller than the viewport — which is the only case where a boundary is
somewhere you can see the difference from, since a message that fits on screen is entirely on
screen once its top is. `--shot` drags 60% into the longest drawn message of the conversation,
16,342 pt against a 335 pt viewport, and lands at 2041.60 rather than at 2041's top, 16 thousand
points away. When the drag names a message the list has not laid out, the height is not known yet
and the request is re-issued once, the frame the row reports one.

**`ScrollPosition` was the plan and does not work.** `chat-search-me9.8.27` was filed on the basis
that macOS 15's `ScrollPosition` with `.scrollPosition()` scrolls a list to an offset, which would
have made the drag a pixel gesture outright. Measured on macOS 26.5.2 with Swift 6.2.4, against a
`List` in this window and against a bare 400-row `List` in a spike: `scrollTo(y:)`, `scrollTo(point:)`,
`scrollTo(edge:)` and `scrollTo(id:)` all leave the content offset exactly where it was, while
`ScrollViewReader.scrollTo(_:anchor:)` on the same list in the same run moves it. `position.viewID`
reports the id it was asked for, so the binding takes the request and the list ignores it. The same
spike drives a `ScrollView` + `LazyVStack` and `scrollTo(y: 1234)` lands on 1234.0, so it is `List`
that is not wired up rather than the API being misused. `chat-search-me9.8.42` holds what is left
of the pixel drag, and `chat-search-me9.8.33` — which was filed expecting `ScrollPosition` to be
how an appended page keeps its place — needs another answer.

A keyboard step is anchored on the first message whose *start* is on screen, which is not the top
of the box. Scrolling to a message leaves 10 pt of the one above it showing — `contentMargins` —
so the box's top edge is inside the previous message and a step anchored there resolves to the same
message forever. `me9.8.18` worked around the same shape by anchoring on the message the map last
asked for; a fact about the screen is the better anchor now that there is one.

#### What the minimap cannot draw

The same table the row keeps for the ribbon, and for the same reason: absent rather than invented.

| mark | why not |
| --- | --- |
| a pause | `poc/ui`'s `mm-notch`. A gap inside a conversation is not on the wire; `Sitting.gapMs` measures the silence *between* the records folded into a sitting. Thresholding timestamps here would be the local-date bug's shape — one rule, three clients, disagreeing at the edges. `chat-search-me9.39` |
| a compaction boundary | `poc/ui`'s `mm-cut`, and the one `poc/ui/NOTES.md` argues hardest: "a compaction says the earlier half stopped being verbatim, so the agent past it knows different things … at seq 924 of 1,323 you would never find it by scrolling". claude-code transcripts carry it explicitly — `isCompactSummary`, `compact_boundary` — so it is an importer and contract question rather than an inference. Same bead |

The map also costs the transcript 50pt of its width, which is 422pt of extra document on that
conversation. `poc/ui` pays the same — `.pv` is `grid-template-columns: 1fr 50px` — and the drawer's
reading measure is the number `poc/ui/NOTES.md` says was arrived at rather than inherited, so it is
a real trade and not a free column.

### Seeing it

```bash
swift run -c release chat-search --shot --query "borrow checker" --out /tmp/reader.png
swift run -c release chat-search --shot --query "the" --longest --limit 400 --out /tmp/long.png
```

The third non-affordance flag, and the same argument as the other two: `--measure` answers with a
number and `--verify-theme` with an exit code, but whether a 923-message conversation comes out as
a readable column has no number in it. `cacheDisplay(in:to:)` renders the view hierarchy into a
bitmap with no window server and no screen-recording grant, so it runs from a script and on a
machine nobody is sitting at.

It writes fifteen frames, and one line before any of them: what a row costs and how many of them
the window holds, read off the list while nothing is open — [the row](#the-row) is where that
reading is explained. The first frame is the drawer as it opens; the next two are the minimap's
relationship checks — the drawer is driven half a document, then the map is dragged to 75% — and
the fourth is after typing on with the conversation still open, which is the state a list-driven
selection closes without being asked to. The last seven are two per grouping axis, open and folded,
plus Library. Each relationship frame prints where the transcript ended up and where the box went,
to a fraction of a message; on the corpus's longest conversation that is 0.00–0.58 at rest,
273.36–273.52 after the fling and 1812.00–1818.09 after the drag, run to run, plus the box's travel
over the fling and over twenty 6 pt nudges and a drag into the longest message there is. The
grouped frames print group and residue counts beside them,
reading is explained. The first frame is the drawer as it opens; the next two are the minimap's
relationship checks — the drawer is driven half a document, then the map is dragged to 75% — and
the fourth is after typing on with the conversation still open, which is the state a list-driven
selection closes without being asked to. The last seven are two per grouping axis, open and folded,
plus Library. Each relationship frame prints where the transcript ended up and where the box went;
on the corpus's longest conversation that is messages 0–8 at rest, 300–330 after the fling and
1810–1818 after the drag, run to run. The grouped frames print group and residue counts beside them,
reading is explained. The first frame is the drawer as it opens; the next four are one per
[preset](#the-four-knobs); the two after those are the minimap's relationship checks — the drawer
is driven half a document, then the map is dragged to 75% — and the eighth is after typing on with
the conversation still open, which is the state a list-driven selection closes without being asked
to. The last seven are two per grouping axis, open and folded,
plus Library. Each relationship frame prints where the transcript ended up and where the box went;
on the corpus's longest conversation that is messages 0–8 at rest, 300–330 after the fling and
1810–1818 after the drag, run to run. The grouped frames print group and residue counts beside them,
which caught a group head clipping `39` to `3` and a residue head eliding `no working directory` to
`n…y`.

**The fold's other half has no picture, so the same pass drives it.** `chat-search-me9.8.15` is two
claims — a group folds, and a folded group cannot hold the cursor — and only the first shows up in
a PNG. So for each axis the pass puts the cursor on a row, shuts the group around it, then walks
every line from the top by the key an arrow sends and counts the rows it reached that nobody can
see. It switches axis and back too, because clearing the accordion on a switch is the third thing
that bead promised. On `borrow checker`, 58 rows:

```
fold, by project: 1 of 12 shut, 31 lines the cursor can reach
  folding under the cursor moved it to the head: true
  rows reached inside a folded group: 0 of 39 hidden
  after switching axis and back: 0 folded
```

with `run` at 51 lines over 17 hidden and `source` at 4 lines over 56. It is a printed pass rather
than a test for the reason [`--verify-theme` is a flag](#why---verify-theme-and-not-a-test): the
Command Line Tools SDK carries neither `Testing` nor `XCTest`.

**What this cannot drive is the pointer.** The pass calls the fold where a click would, so what it
checks is everything downstream of the gesture and not the gesture itself — the mirror of the
minimap pass, whose keyboard half has no pointer to drive it and says so. That is why the head is a
`Button` rather than a tap gesture on a section header: a hit test nobody can script is one to leave
to the framework.

**The fidelity model has the same shape: four pictures, and three claims no picture makes.** So the
preset pass prints those three beside the frames. On the corpus's longest conversation:

```
presets, over 1479 drawn messages of claude-code:4579afb5-…:
  segments   84 rows, 41 of them run summaries · 43 full, 345 brief, 1091 off
    of those, 39 name calls, 9 failures, 10 questions, 7 carry the match dot
    the fullest of them reads: → 5 calls · 1 failed · asked you 1×
  outline    1479 rows, 0 of them run summaries · 0 full, 1479 brief, 0 off
  read       1479 rows, 0 of them run summaries · 388 full, 1091 brief, 0 off
  everything 1479 rows, 0 of them run summaries · 1479 full, 0 brief, 0 off
  read is `Density::Full` spelled in Swift, and agrees with the fold on the wire for 1479 of 1479
  one message opened by hand: it is expanded while its user knob says brief, 1 override in hand
  cleared: it is collapsed again
  opened claude-code:5b4f1d3c-… with segments in force: the knobs are where they were left
  the pass itself: 1868 messages built and 7 canvas renders, put back before the next pass reads either
```

The two indented lines are the other thing a frame cannot carry: it shows four summaries and the
transcript holds forty-one, so the counts say how many of them are richer than a bare number and
the fullest one is printed verbatim. A corpus where no run ever failed or asked anything would let
"calls, failures and questions rather than a count" pass untested, and this is what says it did not.

The first of the three checks below them is the drift check on the one rule this client spells
twice. The second is
"a per-message override beats the band", driven against `outline` so that one message going full is
unambiguous. The third is the absence of `defaultZoomFor`, driven with a preset the wire does not
answer — a reader that re-derived the knobs from each transcript would land somewhere else and the
line would say so. None of the three is visible in a PNG, and the first two are the kind of thing
that stops being true quietly.

**And then it puts two counters back.** Turning a knob rebuilds every message on purpose — that is
what a fold change *is* — so this pass leaves `MarkedText`'s counters and `MinimapBands.renders`
reading exactly like the regressions they exist to detect, and the minimap pass below them reads
both. It restores them rather than the passes downstream having to know it ran, and reports the
difference as a number of its own, which is the only thing here that measures the machine rather
than the model: four presets, an override and a conversation opened and shut cost 1,868 rebuilt
messages and 7 canvas renders on the corpus's longest conversation.

**`--longest` opens the biggest conversation the query returned rather than the best one.** The
map's hard case is length, no query puts the corpus's 2,431-message conversation first, and a
verification affordance that cannot reach the thing being verified is not one.

The decoding half is checked where the rest of the contract is:

```bash
cd poc/swift && swift run -c release cs-spike contract
```

which now opens a transcript per phrase and holds every mark to landing on a character boundary
**in both the expanded and the collapsed form**. That second half is the one that matters: 388 of
the 563 drawn messages in a real agent session are collapsed, so a one-line form that shifted a
single byte would highlight the wrong word in most of what a reader sees — silently, because a
mark in the wrong place still looks like a mark.

## The row

`poc/ui/index.html` is the structure: line 1 metadata, line 2 the title full width, line 3 the best
match — and line 3 only when there is one, which the contract decides rather than the view, since
`matches` is empty for an empty query. Line 1 is two clusters split by a gutter, `source` and size
on the left, directory and age on the right, because `cwd` answers *which of my worlds was this*
rather than *what is it*.

What it draws is what the wire carries: source, size, directory, age, the title, and the best match
with its highlight runs. Four of the mockup's cells are not on the wire at all and one is held back
on purpose. All five are absent rather than blank — a cell with nothing behind it is not a column to
reserve space for:

| cell | why not |
| --- | --- |
| model | in the index, not in the contract. `chat-search-me9.8.14` |
| forks | `thread_count`, same |
| total edit lines | nowhere. `export.py` sums them by parsing `Edit`/`Write` tool arguments |
| topics | a `poc/ui` clustering over the corpus, not an index fact |
| the ribbon | deliberate: it needs `kind_runs` and `match_seqs` in one coordinate space, which `chat-search-me9.25` says they are not, and nothing stands in its place |

**The agent badge is the word, not an icon, and that is a re-measure rather than a disagreement.**
The mockup dropped the word because its list column was a fixed 706px and the word cost the 66px
`cwd` needed to stop collapsing. This row does not spend that: with no model cell and no ribbon it
costs about 200pt less, so at the window's 720pt floor `cwd` still gets ~470pt against the mockup's
17-character budget. It is also the only channel there is — no asset catalog means no per-source
icon, and SF Symbols has no glyph for a vendor — so the row carries colour and text where the
mockup carried shape and colour. Text is the channel that survives greyscale.

**The column plan is in characters of the meta face, not in points.** `source` 14, size 9, the
gutter 4 and `age` 4, with the directory taking what is left up to 44. One character is measured
off the theme's own size and design, so a direction that moves `--fs-meta` moves the columns with
it instead of leaving the text to outgrow its cell. The terminal states its own plan in character
cells for a different reason — it has nothing else — and the two are the same idea.

The directory's ceiling is the corpus's: 88% of directories are 35 characters or fewer, 91% are 44
or fewer, and the tail is worktree paths, which middle-elide to their leaf. Elision is middle and
never tail, because tail-elision discards the leaf, which is the discriminating token
(`docs/TUI-DESIGN.md` §2). Titles elide the other way for the mirror-image reason.

Source labels, `$HOME` collapsing and the age buckets are second implementations of
`cs-tui/src/text.rs` and `cs-tui/src/theme.rs`, named after their originals. The seam is JSON over
argv, so a Swift process cannot share that code — but what the terminal calls a source and what the
window calls it have to be the same word.

**What a row costs, and what the type scale spends.** `--shot` reads it off the list before
anything is open and prints it beside the frames:

```
density at 900×620: 65.0 pt per row, 423.0 pt of list → 6 rows on screen, 7 counting the one the edge cuts
density at 720×480: 65.0 pt per row, 283.0 pt of list → 4 rows on screen, 5 counting the one the edge cuts
```

Both counts, because at the floor the last row is cut by the window edge and whether a half-drawn
row counts is a judgement made by whoever is looking. The height is a mean over the rows in the
viewport rather than the document divided by its rows: a `List` estimates the height of everything
it has not laid out yet, the estimate is a larger share of a small window's document, and taking
the document at face value read as *shorter rows at 720 than at 900* — a difference in the
instrument reported as a difference in the row.

It exists because `poc/ui/directions.html` fences rows-per-screen **between** directions and says
so — it stands 800px in for a viewport nobody has — which leaves nothing measuring the cost at a
size somebody opens. `chat-search-me9.8.34` is what asked: raising the whole type scale a point
took the row from 62.0 to 65.0pt and the list's viewport from 430 to 423, since the chrome above
and below grew with it, and cost nothing at either size — 6 rows at 900×620 and 4 at the floor,
before and after, because neither 3pt crossed a row boundary. It will cost one the next time
somebody spends 3pt, which is the argument for having the number rather than the argument that
type is free.

Rendered against the live index at 720, 900 and 1400pt. Two things only real data at a real width
could show:

- The message count was locale-formatted. `Text("\(count) msgs")` is a `LocalizedStringKey`, an
  `Int` interpolated into one gets thousands separators, and the corpus's largest conversation
  rendered `2,553 ms…`.
- `styles.css` makes `cwd` the flexible track while its own comment says the gutter is. At 1400pt
  an unbounded `cwd` strands the directory mid-row with the age against the far edge and the two
  clusters stop being clusters, so the ceiling above is what the comment was describing.

Not here, and not this bead's: selection and hover, which are what a row is for once there is a
reader pane to select into (`chat-search-me9.8.3`); and `deleted_upstream`, which is on the wire and
has nowhere to be said yet.

## Grouping: one list, cut

Grouping is a **dimension on the list**, not a place to go. `chat-search-4ar.10` found that Search,
Projects and Sittings drew the same rows, from the same filters, into the same drawer — two of the
three were `GROUP BY` wearing the costume of a destination, and nothing was in one and not the
others. So the axis is a control that leads the query it modifies, reading in the order it applies:
group by project, *then* narrow with the query. The rows, the row component, the selection and the
reader are identical in every arrangement, and one `List` holds all of them, because the container
question was answered once (`chat-search-me9.22`) and sections do not un-answer it.

| axis | key | coverage, from `poc/ui/NOTES.md` §1 |
| --- | --- | --- |
| `none` | — | the ranking, ungrouped |
| `project` | `cwd`, verbatim | 34% of conversations and **92% of all messages** |
| `run` | `ended_at`, clustered at 12h | the widest there is — every conversation but the 2 with no last message |
| `source` | `source` | archetype is ~87% predicted by it |
| `topic` | — | drawn, and not offered |

**The residue group is the point of this bead.** `cwd` is 100% on codex and claude-code and 0% on
chatgpt and gemini-cli, so a project grouping that quietly kept only the rows it could place drops
two thirds of the corpus and looks complete doing it. Every axis that can fail to place a row has a
residue group with a name that says what is missing — `no working directory`, `no last message` —
counted in the open and last in the list. `source` has none, because a conversation always names
the source it came from; that is an absence rather than an omission. The prototype makes exactly
this mistake on `project` and gets it right on `topic`, and this is its own rule applied across.

**Nothing is re-ranked.** `project` and `source` gather in the order each key first appears, so the
group holding the top row leads and the rows inside keep the order they arrived in: the same
answer, regathered. `run` is the one axis that re-orders, because a cluster in time cannot be found
in rank order, and an axis whose labels are dates says that on its face. The residue is last
wherever there is one — it makes no rank claim, and two thirds of a list at the top would bury the
axis you asked for.

**12h is measured, not picked.** The 3-day rule left chat-search, meety-local and dev/career each
as one undivided run; 12h gives 1–15 and tracks the day count exactly (chat-search 6 of 6,
personal-site 4 of 4, ga 7 of 7). It is applied to `ended_at` as a *duration between instants*,
which has no timezone in it. The local day is a different question and is not answered here:
`ended_date` arrives rendered by the core, and `Display.day` reads the fields out of that string —
`Aug 5`, `Oct 25 ’25` — so the rule the local-date bug produced stays in its one place.

It asks a calendar exactly one thing, which year is *this* year, and it asks a **Gregorian** one by
name. `Calendar.current` honours the region's calendar setting, so on a Japanese, Buddhist, Hebrew
or Islamic calendar it answers 8, 2569, 5786 or 1448 — none of which ever equals the `YYYY` on the
wire, and every label in the app would then carry the year suffix that comparison exists to
suppress. Inheriting a calendar is how a client ends up deciding a calendar question for itself,
which is the shape of the bug this paragraph is about.

The group head is `poc/ui`'s `.pj-head` minus one cell. **No source badges**: the prototype puts a
strip of tiny source icons on every header, and with no asset catalog and no SF Symbol for a vendor
the honest version here would be colour alone, which `poc/ui/NOTES.md` §7 rules out — the source is
in every row underneath anyway. What is left is the twisty, the name, a `cross-tool` tag where a run
used more than one, the count, the day span and a 12-bar sparkline of when. The reader pane leaves
that column ~400pt at the window's floor, so the cells give way in a stated order: the twisty and
the count never move — the twisty holds a fixed cell so the name starts at the same x folded and
open, and a count that lost a digit would be a wrong number rather than an elided one — the name
elides in the middle where a path survives it, and **the day span leaves whole rather than
eliding**, because `Oct 8 ’2…` is a worse cell than an empty one.

**Counts are over the rows in hand.** The prototype prints corpus-true counts on a project row
(`114 · 30 here`) because it groups an export of the whole index; nothing on this wire carries the
size of a `cwd` or a run, so the key line above the list states the set — `12 groups · 58 rows in
hand` — rather than a header implying a number the client does not have. The residue count sits
beside it in `--hit`, with the sentence that explains it on hover.

**A grouped list does not ask `cs` for more rows than an ungrouped one**, and the fold is why it
does not have to. The other way to make grouping worth more is a bigger window — raise `--limit`,
or have the grouped path quietly ask for a few hundred rows and show the top of each group — and
both spend the answer's latency on rows nobody asked to see. A larger window would also need a
count on the wire to stay honest, since `12 groups · 58 rows in hand` would become `12 groups · 500
rows in hand` and say less rather than more. Folding buys the same headroom out of rows already
paid for. The corpus-true count is a question of its own: `chat-search-me9.8.23`.

### The fold, and the keyboard that had to come with it

A group folds — click its head, or press <kbd>⏎</kbd> with the cursor on it — and **the list opens
with every group open**. That default is a statement about this window rather than about grouping.
`poc/ui` opens every axis folded and is right to: it groups a corpus-scale export, where thirteen
heads carrying a count, a span and a sparkline are a project index and one open group buries the
other twelve. This groups the `--limit` window of a ranked answer, 60 rows, so folding by default
would hide the answer behind its own heads on every keystroke. **The trade flips when that window
grows** — either because `--limit` is raised or because grouping learns to ask for more rows than
the list shows — which is why the default is one boolean on the model and not an assumption spread
through the view. `--folded` already flips it.

`chat-search-me9.8.4` shipped the sections without a fold *and named the reason it could not ship
one alone*: the prototype's own note says the fold left it leaning on a keyboard affordance nobody
had wired — "the footer offers <kbd>→</kbd> to expand and no key is bound" (`poc/ui/NOTES.md` §5).
So the two land together. The cursor moves through `SearchModel.lines`, which is **every line the
list draws, in the order it draws them**: a head per group and, under each open one, its rows.
A folded group contributes its head and nothing else, and that is the only place in the class where
the fold is honoured — there is no second piece of arithmetic that has to remember what is on the
screen, so a cursor inside a shut section is not a bug that was fixed but a state that cannot be
constructed. The one gesture that could strand it — clicking a head with the cursor already inside
that group — moves it to the head, which is both the line still on screen and the line that opens
the group again.

**A head is a place the cursor can rest, because it has to be.** Fold everything and there is no
row left to stand on; the key that opens one back up has to be pressed somewhere. So Enter means
"act on the line the cursor is on": a conversation opens, a group head folds.

**And it is Enter rather than the prototype's <kbd>→</kbd>**, which is not a shortage of keys. The
only focused view in this window is the query box — that is the TUI's arrangement and the whole
reason arrows move a cursor in the list beside it — so a key this app binds is a key the box stops
getting. Left and right are how a caret moves through a query that is a *grammar*: `agent:codex`,
`after:2026-01`, a quoted `dir:` with a space in it (`chat-search-me9.8.16`). Taking them for the
accordion would buy a fold by breaking the editing of the thing being folded. Enter already belongs
to the list rather than to the field, so the fold costs nothing that was in use.

There is a third way out of that argument now and it does not change this one: a menu resolves its
keys outside the responder chain, which is how the drawer got Cmd-T (`chat-search-me9.8.26`). It
is no use to the fold. A menu item is a single act with no idea where the cursor is, and *fold the
group the cursor is on* is a sentence about a line — so Enter, which is already delivered to the
list, is still the only thing that can say it.

The footer says what Enter would do whenever the cursor is on a head, and how many groups are shut
(`12 groups by project · 12 folded`). That is the same defect `poc/ui/NOTES.md` §5 complains about,
approached from the other side: a footer that draws a key nobody wired and a fold no footer
mentions are the same mistake. What the head itself does *not* change when it folds is anything it
says — the count, the day span and the sparkline are drawn shut exactly as they are open, because a
folded group and a group that is not there have to look different.

**Switching axes clears the accordion** rather than restoring it, which is the prototype's rule and
its reasoning: groups are *ranked*, so the set you left open is rarely the set at the top when you
come back. Clicking the axis already in force is inert, so the reset is a consequence of switching
and never a surprise. Nothing else clears it — a keystroke that narrows the list until a group is
empty and a keystroke that brings it back leave the fold alone, because a list rearranging under a
cursor that never asked it to is worse than a stale fold.

The cost of folding, stated: on `borrow checker` the `source` axis has two groups, so shut it is
two heads and a lot of empty column. That is honest about the axis rather than a fault of the fold,
and `--shot` now captures every axis shut as well as open so it is a thing you look at.

Three things this does not do:

- **A group head does not offer to narrow to itself.** The prototype's does, by writing `dir:` into
  its query state. Here the text a click produces is `cs facets`'s to compose — a client
  assembling tokens itself is the second, partial parser docs/TUI-DESIGN.md §5 costs out — and
  `dir:` has no rail yet. `chat-search-1ld`.
- **`project` is the `cwd` column, not a project.** So the worktrees of one repo are separate
  groups (chat-search was 11 directories) and each of Codex's per-conversation scratch directories
  is a group of one. `chat-search-6eb.26` measured what deriving a project costs — basenames
  collide, and the nearest `.git` ancestor reads the live filesystem and so breaks ADR 1 — and
  closed saying it has to be captured at import time or not at all.
- **`topic` is drawn dashed and cannot be clicked.** A seeded taxonomy is a `poc/ui/export.py`
  derivation over the corpus and not an index fact, so nothing on this wire carries one. Hiding the
  chip would say this corpus has four axes; drawing it live would need a grammar that does not
  exist. `chat-search-me9.18`, and `chat-search-6eb.18` for the discovery half.

## The bottom drawer

A track under all three panes: everything the filters keep, stacked by source below a baseline,
and where this query landed raised above it. `poc/ui/DESIGN-BRIEF.md:59` asks for "a timeline of
whatever survives the current filters, with a scrubber", and its own comment states the claim —
it answers *when was I working on this* and *when did this query land* at once.

**Nothing here counts anything.** That is the decision the bead (`chat-search-me9.8.20`) required
be made and written down before a bar was drawn, and it is the only interesting thing about the
drawer's plumbing. This window holds a `--limit 60` page; ranking is not chronological; so the
page is a *biased sample of exactly the axis being drawn*, and the resulting picture would look
like a picture. `cs timeline --json` counts over the whole matching set and hands back a
fixed-size histogram — a picture's worth of numbers whatever the archive does, on a path that
runs per keystroke. [`docs/JSON-CONTRACT.md`] is the wire; `cs_core::timeline` is the counting.

### The drag is a keystroke

Dragging the track calls `SearchModel.scrub`, which ends in `apply(chip:)` — the same call a
facet chip makes. So the window lands in the *query box* as `date:2026-07-28..2026-08-02`, and the
selection rectangle is then derived from the parsed query rather than kept beside it. Typing that
range by hand moves the rectangle, which is the test that proves there is no second filter state.

`poc/ui/NOTES.md` §17 is why that is worth this much care: the prototype kept its timeline range
in a tuple that never appeared in the query line, and two rounds of the projects view were written
against that state before anyone noticed. `docs/TUI-DESIGN.md` §5 and DESIGN-BRIEF both make it a
bug condition — "a filter that narrows the list without appearing here would be a bug".

**This app does not spell the `date:` token.** A rail can hand each chip the query text clicking
it produces; a drag is two instants out of a continuum and cannot be enumerated that way, so
`cs timeline --drag FROM..UNTIL` makes the trade backwards — instants over, finished text back.
What that keeps on the far side is `Window::value_in`'s two lossy rules (each edge rounds outward
to a whole second; an edge on a midnight writes as a bare date), and two behaviours fall out of it
being `Query::toggling` underneath: dragging the window already in force takes it back off, and a
drag that never leaves the bar it started in clears the filter rather than writing an empty one.
That second one is `poc/ui`'s "a drag under 1% of the span clears the selection", reached through
the grammar instead of through a magic fraction.

The only thing this side contributes to the gesture is **snapping the two ends to bucket edges**.
The picture is bars, so a selection finer than a bar is a selection nobody can see.

### Four departures from the mockup

- **A bar per bucket, not a mark per conversation.** The prototype drew 3,059 marks because it
  had 3,059 conversations in memory.
- **The bars are drawn with `date:` left out of them**, which the prototype also does
  (`visible(true)`) and which is the whole reason a scrubber is worth having: a timeline narrowed
  by its own selection draws a solid block and can never say what widening would get you. Only
  the header's numbers move when you drag.
- **Each half of the track is scaled to its own tallest bar.** 58 matches under 3,617
  conversations on one shared scale is a row of nothing, and where the matches are is the question
  the top half exists to answer. The absolute numbers are in the header, where a scale cannot
  mislead about them.
- **No 30d/90d/1y/all presets.** The facet rail's *When* section already offers four relative
  spans, and a second vocabulary of them in the same window is one rule written twice. Clicking a
  rail chip lights the selection here anyway — the selection is read out of the query text like
  everything else — and "all" is the rail's All chip.

**Log10 and not linear, measured rather than inherited.** Over this archive the tallest bucket
holds 222 conversations and the median non-empty one holds 16, so a linear scale puts the median
at 1.5 pt of a 21 pt half and draws three years of history as a flat line under two months of
spike — `poc/ui/NOTES.md` §14's complaint about a sparkline, one level up. The cost is real and
is stated where the scale is: a log axis understates magnitude, and 222 against 16 reads as
roughly two to one.

**Source hue comes off `Display.sourceHue` and nowhere else.** `chat-search-g6u` settled that a
source id becomes a `--src-*` token in exactly one place; the row and the rail already read it,
and a source the palette has no hue for is drawn in `--ink-3` rather than given a wrong one. That
is why `google-takeout` — 1,280 conversations, a third of the corpus — is the grey band.

### What it costs, and what `hide` buys

Undebounced, on the same keystroke as the search, and that is a measurement rather than a habit.
Warm, best of seven against the 358 MB index, the whole reply costs **1.2× what settling the same
query's total costs** — 56 ms for the worst broad prefix (`the`) against 45 ms, and under 3 ms for
anything narrower. Cold through a spawn it is **140–240 ms where the `cs search --prefix` on the
same keystroke is 280–340 ms**. So a debounce would be staleness bought to save the smaller of two
costs, which is the trade `SearchModel` already refuses for the search itself.

What is offered instead is `hide`. A hidden drawer asks `cs` for nothing, and `--no-timeline`
opens that way — an instrument affordance in the same family as `--group` and `--folded`, so
`--measure` can take the keystroke number both ways. It is not remembered between launches, for
the same reason those are not.

**Cmd-T is the same switch from the keyboard**, through `View ▸ Hide Timeline`
(`chat-search-me9.8.26`). The button in the corner and the menu item call one method, so there is
one act with two routes rather than two behaviours; and since a menu resolves its keys outside the
responder chain, the third `cs` per keystroke can now be stopped without the mouse and without the
query box giving up a key. [The menu bar](#the-menu-bar) is why that became possible after the
drawer shipped rather than with it.

**Shutting it cannot change the result set**, because it never narrowed one: the window it draws
is a `date:` token in the query box, and hiding the drawer does not touch the box.

### Seeing it

`--shot` drives the drawer, because none of the three claims about it is a picture:

```bash
./.build/debug/chat-search --shot --query "borrow checker" --out /tmp/cs.png --size 1200x760
```

It drags the right-hand third of the track, prints what the query box then reads, clears it,
retypes the same window by hand, and compares the bar heights before and after. Against the real
index that is:

```
  timeline: 161 bars of 8d, 2023-02-02 → 2026-08-13 · 3617 in range · 58 selected · 4 undated
    dragged → the box reads "date:2025-05-30..2026-06-02 borrow checker"
    …which parsed back to 2025-05-30..2026-06-02, so the rectangle is derived from the text
    bars unchanged by the window: true
    in range 3617 → 1556, selected 58 → 0
    cleared to no window: true; typed back by hand: 2025-05-30..2026-06-02
    hidden → 58 rows against 58 open
```

Three frames beside the ordinary ones: `-scrubbed.png` with the selection standing, and
`-no-timeline.png` with the drawer shut.

### What it does not do

- **No key moves the selection, and the box is the keyboard route to a date window.** Hiding has
  Cmd-T now; dragging has nothing, and typing `date:2026-07-28..2026-08-02` into the query box is
  the answer rather than a workaround. That is the whole point of the drag being a keystroke: the
  window is a token in the box either way, so the rectangle a drag produces and the rectangle
  typing produces are the same rectangle derived from the same text — [the round trip
  above](#the-drag-is-a-keystroke) is `--shot` checking exactly that. What a binding would buy is
  nudging an *existing* window without retyping it, and nothing is free to bind it to: the arrows
  move the cursor through the results list, and left and right move a caret through a query that
  is a grammar. A menu item is what Cmd-T rides in on and it does not help here — a menu is a
  discrete act, and *wider by one bucket* wants a key held down. So this is stated rather than
  fixed, and the mouse half of the same complaint — no handles, no pan — is
  `chat-search-me9.8.32`. `chat-search-me9.8.26`.
- **`undated` is a number and not a mark.** Four of 4,426 conversations have no ending and can be
  in no bucket; the header says so and the track cannot show them.
- **A negated `date:` draws no rectangle.** The complement of a window is not a rectangle, and
  drawing it as one would put the selection over exactly the stretch the filter threw away. The
  counts beside it are still right.
- **`cs search --tools` has no timeline.** `cs timeline` takes `--prefix` and no other search
  knob, so a query asked against tool traffic cannot be drawn — which on this corpus is where most
  message-level matches land. `chat-search-me9.8.25`.

## Library: the half that is not derived

Everything in Search is a projection of the archive and survives `rm index.db && cs index`. Nothing
in Library would: a collection, a pin, a project merge, a dismissal are all things a person said,
and ADR 3 puts those in `library.db` rather than in the index for exactly that reason. That is why
Collections kept failing to find a tab in the prototype — it was the only authored thing on screen,
competing with derived views for space — and it is why the tab that grouping freed goes here.

It is empty, and it says so four times over, because there is nowhere to author into: `library.db`
is `chat-search-6eb.14` and is not built. This is the third bead to press on it, after `6eb.15`'s
title override and `6eb.25`'s generated summaries.

| shelf | what it will hold | waiting on |
| --- | --- | --- |
| Collections | `matches(query) + pinned − excluded`, so it goes on answering after a reindex | `chat-search-6eb.14` |
| Proposed | seeded topics offered as collections, accept or dismiss | topics are not on the wire — `chat-search-me9.18` |
| Project merges | `/dev/career` (89) + `/dev/projects/career` (65) = 154, suggested and never applied | a corpus-true directory list — `chat-search-1ld` |
| Pinned | the conversation a rule would not have found | `chat-search-6eb.14` |

Library hides the search field, the group control and the facet rail rather than drawing them
inert, which is the defect Sittings had: a view that facets do not narrow should not draw the
facets. The footer says what the view under it is made of, and here that is the one sentence that
separates the two — `authored, not derived — survives a reindex`.

## The four index states

They arrive on two paths and the app puts them back together in one `IndexHealth`, because a
client that models only one path can say "there is no index" and cannot say "this answer is one
build behind".

| state | arrives as | what the app draws |
| --- | --- | --- |
| `ready` | `index_state` on an answered envelope | the results, nothing else |
| `rebuilding` | `index_state` on an answered envelope | the results, plus a dim line and **Ask again** |
| `building` | `error.code` on a refusal | "index is being built", results arrive on their own |
| `no_index` | `error.code` on a refusal | "no index yet — run `cs index`", a first-run state |

Plus `index_stale` — bytes at the path this build of `cs` cannot read — and the two genuinely
transport failures, `cs` missing and an exit nobody has a reading for. Only those last three are
drawn as errors.

`rebuilding` is the one this bead existed for. Both it and `ready` mean the results are
**complete**: since `chat-search-me9.28` a rebuild assembles a sibling index and swaps it in
whole, so there is no such thing as a partial answer. All `rebuilding` adds is that a newer
index is on its way, which is exactly what lets the app offer to ask again instead of presenting
the answer as final. It is not an error and is not styled as one.

To see it without waiting for a real rebuild, hold the claim file a builder would hold:

```bash
cp -f ~/.chat-archive/index.db /tmp/scratch.db
python3 -c 'import fcntl,sys,time; f=open(sys.argv[1],"w"); fcntl.flock(f,fcntl.LOCK_EX); time.sleep(300)' \
    /tmp/scratch.db.building.lock &
swift run -c release chat-search --db /tmp/scratch.db --config /tmp/scratch-config.toml
```

An `index_state` this build has no reading for gets the same line with its own name in it, for
the reason `Destination` has an `unsupported` arm: an added value should cost a line of prose on
screen, not a coordinated release.

## Keystroke to frame, on this target

`poc/swift/RESULTS.md` §1 measured 29–70 ms p50, but it measured the *spike* — a window with a
three-way container picker and a five-field bench footer in it. The app has neither, so the
number was taken again on what actually ships:

```bash
swift run -c release chat-search --measure --config /tmp/scratch-config.toml
```

**Give it a scratch `--config`.** Belt and braces rather than the mechanism: a scripted run
already switches query logging off for every `cs` it spawns and prints a line saying so, but
`archive_root` is where a stray write would land and `queries.jsonl` is authored data that cannot
be reconstructed. A temp `archive_root` plus `log_queries = false` is enough. Leave `--db` on the
real index, which is what makes the number worth taking. It runs as an accessory app and does not
steal focus, which is also how §1 was measured — a latency taken in a frontmost app and one taken
in a background app are not the same measurement.

2026-08-05, live index of **3,617 conversations**, `--limit 60`, 100 ms per character, no
debounce, one `cs search --json` per keystroke, 8-core M3 at load 4.6:

| phrase | rows | keystroke→frame p50 | p95 | keystrokes that rendered |
| --- | ---: | ---: | ---: | --- |
| `borrow checker` | 58 | 100.1 | 127.4 | 9 of 14 |
| `ratatui preview` | 27 | 73.5 | 114.2 | 13 of 15 |
| `sqlite fts5` | 44 | 77.8 | 115.5 | 9 of 11 |
| `launchd` | 34 | 62.2 | 119.9 | 4 of 7 |

Main-thread lag was p50 0.6 ms in every run with 0–1 missed vsyncs out of ~100, which is what §1
found and is the part that has not moved: whatever this costs, it is not paid in dropped frames.

**The facet rail added a second process per keystroke, and the table above was taken before it.**
`cs facets` is ~9 ms on this index and runs beside the search rather than in front of it — same
cancellation, so typing does not queue eight of them either — but it is another `cs` competing for
a core. Two runs on the same day with the rail in, at load 4.6–5.2 rather than the 4.6 above:

| phrase | keystroke→frame p50, run 1 | run 2 |
| --- | ---: | ---: |
| `borrow checker` | 116.5 | 120.0 |
| `ratatui preview` | 86.5 | 80.0 |
| `sqlite fts5` | 69.7 | 110.1 |
| `launchd` | 88.4 | 50.3 |

The spread between the two runs is as large as the difference from the table above, so this
machine at this load cannot separate the rail's cost from its own noise — which is the honest
statement, and it is why the number is recorded twice rather than averaged into one that would
look settled. Main-thread lag did not move: p50 0.6 ms, 0–1 missed vsyncs, in both runs. That is
the part worth holding, because it says the second process is paid in wall clock and not in
dropped frames.

**That is slower than §1, and the promotion is not why.** The spike was run back to back with the
app against the same index in the same minute and came back at 97.7 / 73.2 / 76.6 / 104.3 ms
p50 — the same numbers. What changed is underneath both. On the same day, `cs-spike transport`
at `--limit 60 --prefix` reports sqlite p50 **44.8 ms** and the seam p50 16.7 ms, against §1's
21.6 and 8.0. The corpus grew 18% and the query roughly doubled in cost; the process boundary is
still the small term. Filed as `chat-search-tpf`.

Fewer keystrokes render than §1's 42 of 47, for the same reason: a query that takes longer is a
query more likely to be killed by the next character. The list skips those states, which is a
debounce arrived at by killing work rather than by not starting it.

**Grouping rebuilds sections rather than rows, and it is not paid in frames.** Two runs back to
back against the same index in the same minute, at load 5.6 and 5.3 — one ungrouped, one `--group
project`, the axis that also has to gather a residue:

| phrase | p50 ungrouped | p50 by project |
| --- | ---: | ---: |
| `borrow checker` | 140.1 | 154.2 |
| `ratatui preview` | 83.9 | 97.7 |
| `sqlite fts5` | 109.9 | 70.0 |
| `launchd` | 101.8 | 66.6 |

Two go up and two go down, which is the same reading the rail got: at this load the difference is
inside the machine's own noise, and both runs are slower than the table above because the load is
higher. What is not noise is that main-thread lag stayed at p50 0.6 ms with 0–2 missed vsyncs in
both — so whatever regathering 60 rows and rebuilding a dozen sections costs on every keystroke, it
is not dropped frames.

**A folded list is cheaper, and not by enough to be a reason.** The same pair again with
`chat-search-me9.8.15`'s fold in, back to back at load 6.0 and 5.4 — `--group project`, then
`--group project --folded`, which draws twelve heads and no rows at all:

| phrase | p50 by project | p50 by project, folded |
| --- | ---: | ---: |
| `borrow checker` | 173.3 | 164.4 |
| `ratatui preview` | 99.9 | 80.4 |
| `sqlite fts5` | 148.5 | 135.9 |
| `launchd` | 84.7 | 66.9 |

All four go down, which is the direction to expect when a keystroke lays out twelve heads instead
of twelve heads and sixty rows — and every gap is smaller than the spread the two runs above showed
against each other, so one pair going the same way four times is a hint rather than a finding. It
is recorded because the *absence* of a cost matters more than the size of a saving: main-thread lag
was p50 0.6 ms with 0 missed vsyncs in all eight runs, so folding is not something the list has to
be protected from, and the default being open is a judgement about reading rather than about
frames.

## Known

AppKit logs `Application performed a reentrant operation in its NSTableView delegate` a few times
per typing run, and says it will become an assert. It predates this app — `cs-spike typing` logs
it identically — and comes from replacing a `List`'s contents while the table is mid-update.
Filed as `chat-search-9uu`.
