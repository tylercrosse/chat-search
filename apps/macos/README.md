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
| how does it fold? | `fold` | the fold is what makes a 900-message agent session legible; two clients folding differently is two different conversations |
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

Not built, and each for a reason rather than for time: **outline mode**, which needs the same
collapsed forms (`chat-search-me9.20`), and the mockup's **fidelity chips, segment summaries and
work summary**, which need facts — segments, topics, touched files — that nothing on the wire
carries. The **minimap** was in that list until `chat-search-me9.8.18`; it is below.

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
