# chat-search for macOS

The Swift surface. A search field, a result list, and a line saying what the index is doing —
that is all of it, and `chat-search-me9.8.2` onward is what makes it worth using.

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

**`Sources/ChatSearch`** — the window, the model and the view. One `cs search --json` per
keystroke with the previous one killed and no debounce, which is not an oversight:
`chat-search-me9.22` measured fork/exec at 0.3 ms and the whole process boundary at 5–13 ms, so
a debounce would be latency spent to save a cost that was measured and found small. Rows go
through `List` because it is the only one of SwiftUI's three containers that recycles — 5.2 MB
scrolling the whole corpus against `LazyVStack`'s 65.6 MB and `VStack`'s 566 MB. That question
is answered, so the app does not offer the other two.

[`docs/JSON-CONTRACT.md`]: ../../docs/JSON-CONTRACT.md

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
