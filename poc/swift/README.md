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

Requires the Command Line Tools SDK and nothing else: no Xcode project, no bundle, no
dependencies. Sits beside `poc/rust` and `poc/ts` for the same reason they do — an instrument for
answering a question, not part of the product. `poc/` is outside the cargo workspace.

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
```

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
needed; it is not this program's job to pre-empt it.

**No `cs serve --stdio`.** The transport follows the direction rather than leading it. A spike
that needs no new infrastructure is one that can be thrown away honestly.
