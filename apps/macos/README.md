# chat-search for macOS

The Swift surface. A search field, a result list, a reader beside it, and a line saying what the
index is doing — that is all of it, and `chat-search-me9.8.2` onward is what makes it worth using.

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
private to the app. It is the repo's only non-Rust reader of [`docs/JSON-CONTRACT.md`], it is
written once, and `poc/swift` consumes it so that `cs-spike contract` checks the same decoder
this app is built on. The dependency points instrument at product; nothing here points back
into `poc/`.

**`Sources/CsTheme`** — the token layer, and a target of its own so that "a view may read a token
and may not author one" is a thing the compiler enforces rather than a thing a review notices.
See [the theme seam](#the-theme-seam) below.

**`Sources/ChatSearch`** — the window, the models and the views. One `cs search --json` per
keystroke with the previous one killed and no debounce, which is not an oversight:
`chat-search-me9.22` measured fork/exec at 0.3 ms and the whole process boundary at 5–13 ms, so
a debounce would be latency spent to save a cost that was measured and found small. With a
conversation open that is two processes per keystroke rather than one, both cancellable: the
median conversation is ~10 KB and the corpus's longest measured 50–90 ms end to end, against the
~50 ms the search beside it already costs. Rows and messages both go through `List` because it is
the only one of SwiftUI's three containers that recycles — 5.2 MB scrolling the whole corpus
against `LazyVStack`'s 65.6 MB and `VStack`'s 566 MB. That question is answered, so the app does
not offer the other two.

[`docs/JSON-CONTRACT.md`]: ../../docs/JSON-CONTRACT.md

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
carries.

### Seeing it

```bash
swift run -c release chat-search --shot --query "borrow checker" --out /tmp/reader.png
```

The third non-affordance flag, and the same argument as the other two: `--measure` answers with a
number and `--verify-theme` with an exit code, but whether a 923-message conversation comes out as
a readable column has no number in it. `cacheDisplay(in:to:)` renders the view hierarchy into a
bitmap with no window server and no screen-recording grant, so it runs from a script and on a
machine nobody is sitting at. It writes two frames — the second after typing on with the
conversation still open, which is the state a list-driven selection closes without being asked to.

The decoding half is checked where the rest of the contract is:

```bash
cd poc/swift && swift run -c release cs-spike contract
```

which now opens a transcript per phrase and holds every mark to landing on a character boundary
**in both the expanded and the collapsed form**. That second half is the one that matters: 388 of
the 563 drawn messages in a real agent session are collapsed, so a one-line form that shifted a
single byte would highlight the wrong word in most of what a reader sees — silently, because a
mark in the wrong place still looks like a mark.

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
