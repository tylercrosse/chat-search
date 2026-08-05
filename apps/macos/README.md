# chat-search for macOS

The Swift surface. A search field, a facet rail, a result list, and a way back into the
conversation — `chat-search-me9.8.2` onward is what makes the row itself worth reading.

```bash
cargo build --release                       # the app finds ./target/release/cs by itself
cd apps/macos && swift run -c release chat-search
```

No Xcode project, no asset catalog, no bundle: the Command Line Tools SDK and nothing else, the
same terms `poc/swift` was built on. When something here needs a bundle — a Dock icon, a login
item, a URL scheme — that is the moment to add one.

Flags, all of which exist so this can be exercised without touching real data:

```bash
swift run -c release chat-search --db /tmp/scratch.db --config /tmp/scratch-config.toml
swift run -c release chat-search --bin /path/to/cs --limit 30
```

## What it is made of

**`Sources/CsKit`** — the decoder and the transport, and a library product rather than something
private to the app. It is the repo's only non-Rust reader of [`docs/JSON-CONTRACT.md`] — both
replies, `cs search --json` and `cs facets --json` — it is written once, and `poc/swift` consumes
it so that `cs-spike contract` checks the same decoder this app is built on. The dependency points
instrument at product; nothing here points back into `poc/`.

**`Sources/CsTheme`** — the token layer, and a target of its own so that "a view may read a token
and may not author one" is a thing the compiler enforces rather than a thing a review notices.
See [the theme seam](#the-theme-seam) below.

**`Sources/ChatSearch`** — the window, the model and the view. One `cs search --json` per
keystroke with the previous one killed and no debounce, which is not an oversight:
`chat-search-me9.22` measured fork/exec at 0.3 ms and the whole process boundary at 5–13 ms, so
a debounce would be latency spent to save a cost that was measured and found small. Rows go
through `List` because it is the only one of SwiftUI's three containers that recycles — 5.2 MB
scrolling the whole corpus against `LazyVStack`'s 65.6 MB and `VStack`'s 566 MB. That question
is answered, so the app does not offer the other two.

[`docs/JSON-CONTRACT.md`]: ../../docs/JSON-CONTRACT.md

## Facets: the query text is the filter

Clicking a source in the rail does exactly one thing — it puts a string in the search box. There
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

Three things this does not do:

- **Only `agent:` has a rail.** `date:` and `dir:` already filter — they are in the grammar, they
  are applied, and you can type them — but `date:` needs a toggling rule of its own, since its
  tokens intersect rather than union, and `dir:` needs a corpus-true project list. `chat-search-1ld`.
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
the only judgements the query log has (docs/TUI-DESIGN.md §6). What it does *not* yet record is
the other half — quitting with a query and no pick, the abandonment signal — which is
`chat-search-pdw`.

## The theme seam

No view names a colour, a size or a face. Every one is a token read off the environment, and the
values live in exactly one generated file:

```bash
python3 poc/ui/tokens.py terminal -o apps/macos/Sources/CsTheme/Tokens.swift
cd apps/macos && swift run -c release chat-search --verify-theme
```

`Tokens.swift` is generated from `poc/ui/styles.css` and `poc/ui/directions.css` — the same files
the prototype renders, read through the same cascade `palette.py --verify` reads them through —
so there is one authored copy of this palette rather than two that agree until they don't. It is
checked in because the app must build from a checkout with no Python in it, and it is provenance
rather than a dependency: nothing in `apps/` reads `poc/` at build or at run time.

**Changing the whole palette is one file and no views.** Not asserted — measured, by generating
each of the other three directions over the shipped one:

| direction | `git diff --shortstat` | builds | `--verify-theme` |
| --- | --- | --- | --- |
| `paper` | 1 file changed, 75 insertions(+), 75 deletions(−) | 0 warnings | holds |
| `blueprint` | 1 file changed, 76 insertions(+), 76 deletions(−) | 0 warnings | holds |
| `ink` | 1 file changed, 74 insertions(+), 74 deletions(−) | 0 warnings | holds |

The generated file also declares which direction is `shipped`, which is why none of those touched
a view: the name of the direction in force appears in that file and nowhere else.

### Why `--verify-theme` and not a test

There are no tests to put it in. This package builds against the Command Line Tools SDK, where
neither `Testing` nor `XCTest` exists, so `swift test` cannot run at all — the same reason
`cs-spike contract` is a subcommand. It re-measures what shipped, in both themes: the kind ramp
at 2.2 / 4.0 / 7.2 / 13.0 against `--map-bg` with even ~1.8× steps, the three act shades ordered
inside the tool band, and every text tier against the 4.5:1 AA floor on **both** grounds it lands
on. That last one is not pedantry: `--ink-3` was fixed once against `--bg` and was still at
4.23:1 on `--panel`, which is where most of that text actually is.

It exists for the reason `palette.py --verify` exists. Solving a colour and writing it down are
two events, and generating adds a third — so what ships is now two steps from what was solved,
and neither step is checked by anything that reads Swift.

### Adding a theme — Solarized, Gruvbox, one of your own

Not yet possible without editing this app, and here is the honest shape of it.

A theme is not a list of hexes here. Half of one is *solved*: the four message kinds have to sit
on an even luminance ramp against the ribbon track, because hue is the channel that degrades
fastest at the ~2px those bands are drawn at, and the quiet tier has to clear 4.5:1 at 9–11px.
So the path for Solarized is to add its hues to `DIRECTIONS` in `poc/ui/palette.py`, let that
solve the eight fenced tokens, write the rest into `directions.css`, and run `tokens.py`. A theme
that skips the solve will fail `--verify-theme`, which is the check doing its job rather than
being in the way.

Three things this seam does **not** do yet, each filed:

- **One theme per binary.** `Tokens.swift` holds the direction in force; picking between several
  at runtime needs them to coexist, plus a flag and a preference. `chat-search-me9.8.9`.
- **Nothing loads at runtime.** Dialling in type and spacing is edit, regenerate, rebuild, relaunch.
  Reading a token set from a file would make it edit and relaunch. `chat-search-me9.8.10`.
- **Padding is mostly still literal.** The row's rhythm is tokenised because a direction moves it
  and it trades against rows-per-screen. The search bar's, the banner's and the footer's are not —
  they are literal in `styles.css` too — so dialling those is still a view edit.
  `chat-search-me9.8.11`.

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

**Give it a scratch `--config`.** Every named query it types appends to the archive's
`queries.jsonl`, which is authored data and cannot be reconstructed; `archive_root` pointed at a
temp directory plus `log_queries = false` is enough. Leave `--db` on the real index, which is what
makes the number worth taking. It runs as an accessory app and does not steal focus, which is also
how §1 was measured — a latency taken in a frontmost app and one taken in a background app are not
the same measurement.

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

## Known

AppKit logs `Application performed a reentrant operation in its NSTableView delegate` a few times
per typing run, and says it will become an assert. It predates this app — `cs-spike typing` logs
it identically — and comes from replacing a `List`'s contents while the table is mid-update.
Filed as `chat-search-9uu`.
