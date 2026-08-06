# Swift surface spike

> **Findings:** [`RESULTS.md`](./RESULTS.md) — the measurements, what they say about Swift versus
> the web front end, and the three defects in the JSON contract that only a second client could
> have found. That file is the durable asset; this code is not.

A search field and a result list, fed by spawning `cs search --json`. Nothing else — no reader
pane, no filter rail, no grouping. It exists to answer four questions that
[`poc/ui`](../ui/README.md) structurally cannot, because a clickable mockup has no keystrokes and
no process boundary.

```bash
cargo build --release            # cs-spike finds ../../target/release/cs by itself
cd poc/swift && swift run -c release cs-spike
```

Requires the Command Line Tools SDK and nothing else: no Xcode project, no bundle. Sits beside
`poc/rust` and `poc/ts` for the same reason they do — an instrument for answering a question, not
part of the product. `poc/` is outside the cargo workspace.

## The one dependency, and which way it points

The decoder and the transport are no longer here. They are `CsKit`, a library product of
[`apps/macos`](../../apps/macos/README.md), and this package consumes it (`chat-search-me9.8.1`).

That direction is the point. The decoder is written once, since every field added to the
contract after the first non-Rust reader ships would otherwise land twice. And the app cannot
depend on an instrument, so the instrument has to depend on the product. What it buys is that
the contract check below tests the decoder the app is actually built on rather than a copy that
has since drifted, which is the same class of failure the check exists to catch.

What is still the spike's own: the benches, the metrics, and `SearchView.swift` — a view with a
three-way container picker and a five-field footer in it, which is scaffolding for questions
rather than a surface, and is deliberately not what the app renders.

## The contract check

```bash
swift run -c release cs-spike contract --config /tmp/scratch-config.toml
```

Decodes both shapes of `cs search --json` out of a real index and **exits 1 if either stopped
decoding**. Run it after anything that touches `cs_core::answer` or the CLI's serialization.

This is here because being outside the cargo workspace has a price. `cargo test --workspace` was
green for an entire release while this — the repo's only non-Rust decoder, and the thing that
found the nullable `title` and the UTF-8 spans — could not read the first conversation of the
first response, because `Group.hits` had become `Group.matches` (`chat-search-me9.36`,
`chat-search-me9.8.7`). Nothing failed anywhere. A contract with one implementation is a struct.

What it checks, against real output rather than a fixture: both envelopes decode, including the
`--flat` one, which is a *separate shape* and not a flag on the other; `count` agrees with the
array it counts and `total` is not below it; every `snippet_spans` entry lands on a character
boundary in the units `mark_offsets` names — the encoding read off the wire rather than assumed,
which is the whole point of `chat-search-me9.33`; and a census of the nullable and routinely-empty
fields over the whole corpus, so a field the contract still tells clients to handle is one some
row still exercises.

**Give it a scratch `--config`.** Every named query it runs appends to the archive's
`queries.jsonl`, which is authored data and cannot be reconstructed. `archive_root` pointed at a
temp directory plus `log_queries = false` is enough; leave `--db` on the real index, which is what
makes the check worth running at all.

## The benches

Every number in `RESULTS.md` comes from one of these, so none of them has to be taken on trust.
All of them print the machine's load average first, because this is a laptop with a browser on it
and a p95 taken at load 40 means something different from the same number at load 2.

```bash
swift run -c release cs-spike transport   # cost of one `cs search --json` per keystroke
swift run -c release cs-spike states      # what a client can tell apart when there is no index
swift run -c release cs-spike rebuild --db /tmp/scratch.db   # query an index while it is rebuilt
swift run -c release cs-spike typing --interval 100          # keystroke → frame, windowed
swift run -c release cs-spike list --rows 4000               # 3,059 rows through three containers
swift run -c release cs-spike snapshot --query "borrow checker" --out /tmp/shot.png
```

`snapshot` draws the window to a PNG from inside the process, because `screencapture` needs a
screen-recording grant a background shell does not have — and without one there is no way to check
that a measured frame contained anything. It renders the list faithfully and the chrome not at all.

`transport` and `states` are headless. `typing` and `list` open a window as an accessory app, so
they do not steal focus, and quit when they are done. `rebuild` runs `cs index` against the
scratch database you name and **deletes and rewrites it** — never point it at the real index.

Flags: `--db`, `--config` and `--bin` override the index, the config and the `cs` binary;
`--limit` sets how many conversations to ask for; `--interval` the milliseconds between simulated
keystrokes; `--rows` the size of the list.

## What it deliberately does not do

**No reader pane.** `cs show --json` exists, but `tool_summary` and `recognition_line` still live
in `cs-tui` (`chat-search-me9.20`), so a Swift reader would either render raw message text or
reimplement them — and reimplementing them is the duplication `chat-search-me9.17` exists to stop.

**No debounce.** The whole question is what a process per keystroke costs, and a debounce answers
a different question by hiding that one. `RESULTS.md` §2 has the number that says whether one is
needed, and it is not this program's job to pre-empt it.

**No `cs serve --stdio`.** The transport follows the direction rather than leading it. A spike
that needs no new infrastructure is one that can be thrown away honestly.
