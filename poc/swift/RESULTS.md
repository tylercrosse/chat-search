# Swift surface spike — findings

`chat-search-me9.22`. Companion to [`README.md`](./README.md), which says how to run the thing.
This says what it measured and what the numbers decide.

Everything here is against the live corpus on 2026-08-04: **3,059 conversations / 184,099
messages**, a 324 MB index, Apple M3 with 8 cores, macOS 26.5, Swift 6.2.4 built `-c release`
against the Command Line Tools SDK. `cs` is `cargo build --release` at `0c84bf2`.

**On the numbers.** This is a working laptop with two browsers, a desktop agent and several CLI
agents on it. Load average was 5–12 for every table below and could not be brought lower, so each
one reports **min alongside p50 and p95**. Contention only ever adds time, so the minimum is the
closest available reading of the uncontended cost and the gap to p50 says how contaminated the
rest of the row is. Where a conclusion depends on a p95, that is said out loud.

---

## 1. Does typing feel fast when every keystroke spawns a process?

**Yes, and by more than expected — but only after a client bug that had nothing to do with the
seam was removed. See §2, which is the more useful half of this answer.**

Hardware keystroke to the frame that shows the result, measured off `NSEvent.timestamp` and a
`CADisplayLink`, typing four real phrases one character at a time with no debounce and one process
per character:

| rows asked for | typing speed | keystroke→frame p50 | p95 | keystrokes that rendered |
| --- | --- | ---: | ---: | --- |
| `--limit 10` | 100 ms/char | **29–40 ms** | 92–130 ms | 44 of 47 |
| `--limit 60` | 100 ms/char | 38–70 ms | 99–140 ms | 42 of 47 |
| `--limit 10` | 40 ms/char | 31–41 ms | 47–66 ms | 34 of 47 |

"Rendered" matters as much as the latency. At an unhurried 100 ms per character **almost every
keystroke produces its own frame** — 44 of 47, with three superseded by the next key. At 40 ms per
character, which is fast typing, 13 of 47 are killed before they answer and the list simply skips
those states. That is a debounce arrived at by killing work rather than by not starting it, and
the visible behaviour is the same.

The main thread was never the problem: lag behind vsync was **p50 0.6 ms** in every run, with 0–2
missed vsyncs out of ~100. Whatever the seam costs, it is not paid in dropped frames.

### Where the milliseconds go

Headless, 47 prefixes × 3 passes per cell:

| `--limit` | `--prefix` | total min | total p50 | total p95 | sqlite min | sqlite p50 | seam min | seam p50 | stdout |
| ---: | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 10 | yes | 4.6 | 12.6 | 70.6 | 1.0 | 6.9 | 3.6 | 5.5 | 34.3 KB |
| 10 | no | 3.7 | 9.1 | 26.6 | 0.5 | 3.8 | 3.2 | 4.9 | 25.9 KB |
| 30 | yes | 5.6 | 26.5 | 85.0 | 1.1 | 19.6 | 4.5 | 7.1 | 83.5 KB |
| 30 | no | 3.8 | 13.9 | 33.5 | 0.5 | 6.8 | 3.4 | 6.1 | 54.7 KB |
| 60 | yes | 6.6 | 28.6 | 87.4 | 1.5 | 21.6 | 5.2 | 8.1 | 111.0 KB |
| 60 | no | 3.7 | 17.6 | 50.9 | 0.5 | 10.8 | 3.2 | 6.5 | 54.7 KB |
| 120 | yes | 8.3 | 40.3 | 121.6 | 1.6 | 29.4 | 6.0 | 8.0 | 111.0 KB |
| 120 | no | 3.9 | 17.8 | 60.7 | 0.5 | 11.8 | 3.4 | 6.5 | 54.7 KB |

`seam` is `total − sqlite` differenced per sample — everything the process boundary costs, which is
exactly what `cs serve --stdio` or a C ABI would buy back. Broken down at limit 60:

```
fork/exec                  min    0.2  p50    0.3  p95    0.4  max    0.6
spawn→last byte            min    6.1  p50   28.5  p95   86.6  max  514.2
json decode                min    0.6  p50    1.2  p95    2.8  max    3.4
of which sqlite (cs ms)    min    1.5  p50   21.7  p95   80.5  max  504.9
TOTAL round trip           min    6.7  p50   29.2  p95   87.5  max  517.4

the seam alone             min    5.2  p50    8.0  p95   12.5  max   15.5
```

Three things fall out, and none of them is what ADR 14 was argued about:

- **The seam is 5–13 ms and it barely moves.** min 5.2, p50 8.0, p95 12.5 — a 2.4× spread across
  141 samples, while the total spread is 77×. It is the most stable term in the system and the
  smallest one that anybody proposed removing.
- **Spawning is free.** `fork/exec` is 0.3 ms p50 and never exceeded 0.6 ms. The daemon argument
  was always about ~3 ms of spawn-and-open; the spawn half of that is 0.3.
- **The cost is inside `cs`, and the client controls it.** `--limit` moves sqlite p50 from 6.9 to
  29.4; `--prefix` costs 2–3× at every limit (6.9 vs 3.8 at limit 10, 29.4 vs 11.8 at 120). Both
  are client-side decisions made per keystroke, and either is worth more than any transport change
  on the table.

Query length is the other lever, though it has to be read carefully. Over the seven lengths where
all four phrases are still present, p50 goes 7.7, 7.5, **45.4**, 27.5, 26.8, 26.2, 22.2 ms — cheap
at one and two characters, six times more expensive at three, then settling. That is the broad-term
pre-filter doing its job at the point where a prefix is barely a prefix. Beyond seven characters
the phrases drop out one by one, so those rows compare different query mixes and cannot be read as
a length effect at all.

**Consequence for `chat-search-me9.21` (`cs serve --stdio`).** It would buy 5–13 ms per keystroke
against a 30–40 ms keystroke→frame p50, and would leave the 20 ms of sqlite and the 90 ms p95
exactly where they are. ADR 14's decision B is confirmed by a second client in a second language:
not urgent, and the revisit trigger it names (spawn-plus-open p95 past ~20 ms) is not close.

---

## 2. The bug that made the answer wrong, and why it is the most useful finding here

The first run of the table above said keystroke→data was **~85 ms p50** and that at 40 ms/char
only the *final* keystroke of a phrase ever rendered — 13 of 14 killed. Read straight, that says a
process per keystroke does not survive real typing and `--stdio` is urgent. It was wrong, and
nothing about it was a fact about the seam.

The transport was written the obvious way: run the process on a background queue, read both pipes
to EOF, `waitUntilExit()`. That parks **three threads per invocation** — one for the wait, one per
pipe. `DispatchQueue.global()` is not an overcommit queue, so its width is the core count, eight
here. Three keystrokes in flight exhaust it and the fourth waits for a *thread*, not for `cs`.

Readability handlers plus a termination handler park nothing. The same phrases at a *higher* load
average went from 85 ms p50 to 24–69 ms, and from 11 of 15 keystrokes rendering to 13 of 15; the
settled run above is better again.

Worth recording for three reasons:

- The naive spelling is the one every example uses, and it fails in a way that reads as a property
  of the architecture rather than a bug in the client. A `--stdio` transport built on this evidence
  would have been built for nothing.
- ADR 13 flags concurrency, async and FFI as the parts of Rust this project has never exercised.
  Choosing Swift does not retire that risk, it relocates it: this is precisely that class of bug,
  found in Swift, in a 900-line program, in the first thing it does.
- It is an argument for measuring `min` as well as `p50`. The min was 15–19 ms throughout — the
  uncontended path was always fast, and only the distribution said something was wrong.

Two smaller things the compiler and the runtime had opinions about, recorded so they are not
rediscovered. Swift 6's strict concurrency flagged the shared-`var`-across-`@Sendable`-closures
spelling — a warning rather than an error, but pointing at the same data race the thread fix had to
reason about anyway, so it earned its keep. And updating the result array while `List` is mid-update
produces `NSTableView` reentrancy warnings, currently harmless and explicitly promised to become
assertions; they are loud during the typing bench and were left visible rather than silenced.

---

## 3. Does a 3,000-row list need virtualisation?

**Yes, decisively — and SwiftUI's `List` provides it for free, while both obvious `ScrollView`
spellings do not.** All 3,059 conversations, each a three-line row with a marked-up snippet:

| container | first frame | footprint after build | after scrolling the whole list | smooth scroll p95 | fling p95 |
| --- | ---: | ---: | ---: | ---: | ---: |
| `List` | **13 ms** | −1.3 MB | **+5.2 MB** | 3.0 ms | 26.9 ms |
| `ScrollView { LazyVStack }` | 32 ms | +0.7 MB | **+65.6 MB** | 2.9 ms | 23.6 ms |
| `ScrollView { VStack }` | **1,585 ms** | **+566 MB** | +4.1 MB | 21.1 ms | 20.3 ms |

- `VStack` is not a near miss. 1.6 seconds of frozen main thread and 566 MB to show a list that
  fits in 2.0 MB of JSON. It is the control case and it behaves like one.
- `LazyVStack` builds fast and then **never lets go**: 65.6 MB accumulated over a single pass down
  the list, because it is lazy but does not recycle. On a corpus that only grows, that is a leak
  with a polite name.
- `List` is backed by `NSTableView` and recycles. Scrolling the entire corpus cost 5.2 MB, and
  under a trackpad-sized scroll it missed **0 of 56 vsyncs**. Nothing had to be written to get it.

The whole app idles at 31 MB and sits at 35 MB with all 3,059 conversations listed.

Two caveats, both real. The fling — a screenful or more per frame, i.e. dragging the scrollbar —
misses frames in every container, `List` included (30 of 123). Nothing here says fling is solved.
And `List` lays the same row out 13 pt shorter than the stacks do (25.8 pt vs 39.0 pt per row), so
the three are not drawing pixel-identical output; the comparison is of containers, not of
rendering. That difference was not chased down.

For context on the alternative: `poc/ui/NOTES.md` §5 carries "61 DOM nodes per row × 655 results ≈
40,000 nodes — needs virtualisation" as an open item. That is the same problem, unbuilt, at a
fifth of the row count.

---

## 4. What is on screen when there is no index, or one is being built?

ADR 14 requires clients to treat "no index yet" and "index being rebuilt" as first-class states
rather than transport errors. **Measured from the client side, they cannot.** The contract does not
carry the distinction, and one of the two states does not announce itself at all.

### There is no error contract

A failure is exit 1 and one English sentence on stderr. No code, no JSON. So the spike's
`IndexHealth.classify` greps another program's prose — which is as fragile as it looks, and is not
even sufficient, because three different conditions produce the identical sentence:

```
--db points at a path that does not exist  → "index records no importer version, so it
                                              predates this schema — run `cs index` to rebuild"
--db points at a zero-byte file            → the same sentence
--db points at 4 KB of the letter A        → the same sentence
```

A corrupt index reports as an old one, and a first-run user is told their brand-new index predates
a schema they have never had. Also: **`cs` creates an empty database file at a `--db` path that
does not exist**, so a typo becomes a new empty index rather than an error.

### A rebuild is silent, and its partial answers look complete

Querying a scratch index every 250 ms while `cs index` rewrote it underneath, over an index that
already existed:

```
t = 0.0 s          818 conversations — complete
t = 1.8 – 4.4 s    exit 1, "index records no importer version…"
t = 4.7 – 8.0 s    exit 0, 489 conversations
t = 8.5 – 9.3 s    exit 0, 678 conversations
after (9.9 s)      818 conversations
```

The first window is bad and the second is worse. For ~2.6 s the index reports as *missing* rather
than as rebuilding, pointing the user at the command already running. Then for ~5 s queries
**succeed and return silently incomplete answers** — a user is told "nothing matched" about a
conversation that is indexed and will be found thirty seconds later, and there is no field any
client could branch on to say otherwise. A first build from nothing shows the same shape with the
first window stretched to ~11 s.

This is the mirror image of the failure ADR 14 rejected the daemon over. The daemon's hazard was a
long-lived cache serving stale results from a deleted inode; subprocess-per-query has its own
version — a fresh reader of a database being rewritten in place. Neither is a caching bug. Both are
what happens when a disposable index is rebuilt without an atomic swap.

Filed as `chat-search-me9.28` (P1) and `chat-search-me9.29`. The spike renders both states as
first-class screens rather than errors, which is the *client* half of the answer, but the client
half is not the part that is missing.

---

## 5. What a second client found in the contract

The spike is the first thing to decode `cs search --json` without reading the Rust structs that
produce it, which is the only way this class of defect surfaces.

**`title` and `ended_date` are nullable and nothing says so.** `title` is null for 11 of 3,059
conversations and `ended_date`/`ended_at` for 2. The obvious Swift type for a title is `String`,
and it works on every hand test — because no untitled conversation appears in the first ten rows
of anything. It threw at `results[54]` of a `--limit 60` query: exactly the size a GUI asks for,
and exactly the size nobody checks by hand. Filed as `chat-search-me9.27`.

**`snippet_spans` are UTF-8 byte offsets.** Correct, and the only sane choice coming out of Rust —
but a Swift client that reads them as `Character` offsets mis-highlights every snippet containing
an em-dash, and this corpus is made of em-dashes. Nothing in the contract says which. One line of
documentation would retire it.

**`--json` is pretty-printed.** 97,606 bytes where the compact form is 62,547 — **1.56×** — on a
path carrying 111 KB per keystroke at limit 60. Folded into `chat-search-me9.29`.

None of these is expensive to fix. All three are the predicted cost of ADR 14's "the JSON contract
is now load-bearing for real", arriving on schedule.

---

## 6. What does the FFI story feel like?

Not built — the bead scopes this spike to argv, and `poc/rust` plus ADR 14 option C already
describe the shape. What the measurements add is the price tag:

**A C ABI would buy back 5–13 ms per keystroke and nothing else.** That is the whole seam (§1),
and it is the smallest and steadiest term in the round trip. It would not touch the 20 ms of
sqlite, the 90 ms p95, or the 30–40 ms keystroke→frame p50 that a user actually experiences. In
exchange it costs an ABI to design and keep stable, a per-platform build matrix, and — per ADR 14 —
a panic in the core taking the host process down with it.

The one genuine FFI-shaped signal the spike produced is in §2, and it points the other way: the
process boundary is doing real work. A `cs` that segfaults, hangs or is killed mid-query costs the
Swift client one dead `Task` and a message on screen. In-process, it costs the window.

Recommendation: leave C in reserve exactly where ADR 14 left it. Revisit if the seam's p95 passes
~20 ms, which it currently does not approach (12.5 ms).

---

## 7. Swift or the web front end?

**Recommendation: build the client in Swift, and keep `poc/ui` as the design instrument it already
is.** These are not competing, and the framing that they are is the thing worth correcting.

The bead asks this as "whether the prototype's ~3,700 lines of front end survive". On inspection
that is the wrong quantity. `poc/ui` is a mockup over a 13 MB baked export, generated by a 40 KB
Python script. It has no transport, no client state, and — its own `NOTES.md` §5 — "no empty, error
or loading states anywhere" and a result list that "needs virtualisation". It was never a client
and it does not become one by being kept. What the prototype actually produced is `NOTES.md` and
`DESIGN-BRIEF.md` — the measurements, the rejected options and the reasons — and **that transfers
to either language unchanged**, because it is written in constraints rather than in CSS.

So the real question is which language is cheaper to reach a *working* client in. The measurements
say Swift, on four counts and not on speed:

1. **Virtualisation is free and correct** (§3): 13 ms and +5.2 MB for the entire corpus, recycling,
   zero lines written. The web path carries the same problem as unbuilt work at a fifth of the row
   count.
2. **The transport already exists.** `Process` is in the standard library; the seam measured 8 ms
   p50 with no infrastructure added. A browser-hosted front end needs a transport built *for* it —
   a local HTTP server or a Tauri-style IPC bridge — which is precisely the infrastructure ADR 14
   declined to build, arriving through a side door.
3. **Footprint.** 31 MB idle, 35 MB with 3,059 conversations listed, from a 0.6 MB binary linking
   the system's own SwiftUI. Any browser shell starts an order of magnitude above that, for a tool
   whose whole appeal is a 4.5 MB `cs` over a disposable index.
4. **The native affordances are not incidental.** Focus, scroll physics, text selection,
   system-follow appearance and accessibility all arrived free, and every one of them is bespoke
   work in a web shell.

Speed is explicitly *not* the argument. Both clear a type-ahead budget through a subprocess, as
ADR 13 already found for the core, and this spike found the client's own language contributed
1.2 ms of JSON decode against 21.7 ms of sqlite. Swift is not fast enough to matter; it is
*equipped* enough to matter.

**The strongest counter, stated fairly.** The design is still moving — `poc/ui/NOTES.md` says
"still iterating" — and the browser is a better medium for iterating on a colour ramp measured to
two decimal places, ten-to-five typographic scales, and 2 px bands. That is true and it is not a
reason to write the client in it. Keep prototyping in the browser and port settled decisions; the
prototype is a design instrument and should go on being one after the client is Swift.

**What would overturn this.** If a settled design turns out to need a graphic SwiftUI genuinely
cannot draw well — the fused ribbon is the candidate, and `Canvas` was not tested here — or if the
project acquires a reason to run anywhere but macOS. Neither is in evidence today.

**Cost of the evidence:** 919 non-comment lines of Swift, including the entire bench harness. The
client itself — transport, model, view, rows — is about 500.

---

## 8. What this did not test

Recorded so the recommendation is not read as covering more than it does.

- **No reader pane.** Deliberate: `tool_summary` and `recognition_line` are still in `cs-tui`
  (`chat-search-me9.20`), so a Swift reader would either render raw text or duplicate them.
- **No `Canvas` drawing.** The ribbon, the minimap and the gutter spine are the parts of the design
  brief most likely to be hard, and none of them was attempted.
- **No theming, no accessibility audit, no localisation, no packaging.** No bundle, no signing, no
  notarisation, no update path — all of which are real work that a `swift run` binary hides.
- **No `cs serve --stdio` and no FFI**, per scope. §1 and §6 price them; neither was built.
- **One machine, one corpus, one user.** Every number is an M3 with 3,059 conversations and a load
  average of 5–12.
- **`--limit` above 120 and result sets above 111 KB** were not swept; the browse path (3,059 rows,
  2.0 MB, 35 ms to decode) is measured but was not typed into.
