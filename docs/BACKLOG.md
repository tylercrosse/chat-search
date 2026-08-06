# Backlog

**Work items live in [beads](https://github.com/gastownhall/beads), not here.** Run `bd ready --exclude-type=epic` for what is actionable now, or `bd list --parent <epic>` to browse an area.
This file holds the framing that a task list cannot: what the MVP is and is not, the ordering
traps, and the live operational state.

Deferred **decisions** live in [DECISIONS.md](./DECISIONS.md) — anything still `open` or marked
`Open —` there is a decision, not a task, and is only cross-referenced from here so it does not
get tracked in two places and drift.

Items carry the ADR that justifies them, as a `adr-N` label in beads (`bd list -l adr-20`). If an
item and its ADR disagree, the ADR wins.

## Epics

| epic              | area                                                                          |
| ------------------- | ------------------------------------------------------------------------------- |
| `chat-search-a7k` | **Capture** — get transcripts into `~/.chat-archive` before they are pruned  |
| `chat-search-n58` | **Interpret** — parse archived transcripts into the conversation/message DAG |
| `chat-search-6eb` | **Index and search** — schema, indexing, ranking                             |
| `chat-search-me9` | **Clients** — CLI, TUI, Raycast, VS Code                                     |
| `chat-search-4ar` | **Cross-cutting** — fixtures, ADR revisit triggers, redaction                |

One epic has grown large enough to carry its own map: `chat-search-me9.8`, the macOS app, in
[apps/macos/ROADMAP.md](../apps/macos/ROADMAP.md) — the phases it fell into, the dependency graph
over what is left, and which files force two beads apart.

---

## MVP

> Find a conversation you half-remember, across ChatGPT, Claude Code and Codex, and open it
> where it lives.

Archive (done) + three importers + real schema + BM25 + a CLI that prints results with a
`resume_cmd`. Ordered so the one question only you can answer — is the ranking any good —
becomes answerable in days rather than weeks.

**Met as of 2026-07-28.** 2,935 conversations / 168k messages across all three sources,
grouped search with resume commands, 108 tests green. What remains is not features. It is the
three defects below that would make an eval measure the wrong thing, and then the eval itself.
Everything else is post-MVP regardless of how it is grouped.

`bd list -l mvp-blocker` returns these three plus the eval they gate, and the gating is wired
as real dependencies — `bd ready` will not offer the eval until all three are closed:

| blocker                                              | why it corrupts an eval                                     | state |
| ------------------------------------------------------ | ------------------------------------------------------------- | ------- |
| ChatGPT reaches the index only via`--chatgpt-export` | 69% of the corpus vanishes silently if the flag is omitted  | closed 2026-07-30 |
| `ymd()` renders UTC                                  | a wrong date makes "is this the one I meant" unanswerable   | closed 2026-07-30 |
| injected markup in titles                            | 42 conversations (1.4%) show garbage as their result header | closed 2026-07-30 |

**All three cleared as of 2026-07-30.** A bare `cs index` now reaches 2,963 conversations /
172k messages, and a source that contributes nothing gets a row saying why instead of
vanishing. The eval harness (`cs eval sheet` / `collect` / `run`, see `evals/README.md`) and a
24-query seed set are in; what remains is the judging pass, which is the part nobody else can
do. Until queries are judged the score is `—` rather than zero, because an unjudged set
reports as unscorable instead of reporting a number nobody should trust.

The corpus is 2,963 and not the 3,032 previously reported: `IndexStats.conversations` counted
conversation *objects* rather than rows, so 69 conversations that span several transcript
files (ADR 7) were each counted twice. The same bug made a re-delivered ChatGPT export report
4,022 for a corpus of 2,011 — dedup was correct throughout, only the number was wrong.

**Not in it:** embeddings, clustering, `library.db`, any GUI, OpenCode, Gemini, tool-output
search. Skipping `library.db` is safe only while nothing authored is written into
`index.db`. That invariant is easy to hold now and expensive to unwind later.

**Hard constraint:** `cs search --json` must emit exactly what a GUI would consume — stable
field names, no terminal-width truncation. That is what makes a Raycast extension a
weekend's work (~200 lines of TypeScript shelling out to the binary, whose 4.9 ms p95 cold
start fits a type-ahead budget) rather than a refactor — ADR 12.

## Ordering traps

These cost rework, and none of them are decisions. Where a trap is enforceable it is wired as a
beads dependency, so `bd ready` will not offer you the downstream item first.

1. Schema before importer #3, or every importer gets rewritten when the DAG lands. *(Schema
   landed 2026-07-28; trap discharged.)*
2. Importers read the archive from #1. The PoC reads live source dirs; inheriting that makes
   retroactive reparse silently not work — ADR 1. *(Wired: the port blocks on the archive
   reader.)*
3. Fixtures synthetic from the first test, or the repo can never be shared.
4. Query contract before client #2, or the schema is frozen by its consumers. *(Met: `me9.3`
   replaced `resume_cmd` with `destinations`, so `--json` hands a client variants to pick from
   rather than one pre-rendered string. Raycast is unblocked.)*

## Operations

Current live setup on this machine, recorded here because it lives outside the repo:

|          |                                                                     |
| ---------- | --------------------------------------------------------------------- |
| binary   | `~/.cargo/bin/cs` (`cargo install --path crates/cs`)                |
| config   | `~/.config/chat-search/config.toml`                                 |
| archive  | `~/.chat-archive/raw/tylers-macbook-air/`                           |
| schedule | `~/Library/LaunchAgents/com.chat-search.archive.plist`, every 300 s |
| logs     | `~/Library/Logs/chat-search/archive.{log,err}`                      |

To stop it: `launchctl bootout gui/$(id -u)/com.chat-search.archive`

## Decisions still open

Tracked in [DECISIONS.md](./DECISIONS.md), listed here only so they are visible.

ADR 14 (subprocess vs daemon) came off this list on 2026-07-29: an in-process query is
1.4–6.4 ms and spawning `cs` plus opening the index is ~3 ms, so a daemon buys about 3 ms for a
socket protocol and cache-staleness bugs. Rust clients link `cs-core`; everything else spawns
`cs search --json`. Still needs writing up in DECISIONS.md — `chat-search-me9.13`.

ADR 8 (in-app vs upstream rename precedence) came off this list on 2026-08-01: authored wins
unconditionally, because last-write-wins depends on two sources' clocks being comparable and
nothing establishes that. Written up in DECISIONS.md; `chat-search-6eb.15` carries the test.

| ADR | question                             | blocks                                                   | bead waiting on it |
| ----- | -------------------------------------- | ---------------------------------------------------------- | -------------------- |
| 15  | redact at import or at display       | sharing anything, or moving the archive off this machine | `4ar.4`, and `a7k.15` transitively |
| 16  | work/personal account separation     | nothing now — effectively forced to "one source"        | `n58.12` |
| 17  | clone/restore re-key policy          | second machine                                           | `a7k.17` |
| 18  | sync transport                       | second machine                                           | `a7k.15` |
| 19  | hash strength                        | nothing now                                              | `a7k.16` |
| 20  | compress per-file or per-bundle      | compression work                                         | `a7k.14` |

The last column was added on 2026-08-01, when a triage found that every one of these ADRs had
a bead sitting in `bd ready` with an empty description — empty *because* the decision above it
was open, not because nobody had written it up. Those beads are now deferred and each records
its gate, so they leave the ready queue without leaving the backlog. Reopening them is a
consequence of deciding the ADR, and this column is what makes that consequence visible from
the decision rather than only from the bead.

A second class was deferred the same day against a threshold rather than a decision: `6eb.16`
(ADR 10's rebuild-duration trigger), `a7k.14` (ADR 20's growth trigger) and `a7k.16` (ADR 19's
mutate-in-place trigger). Those thresholds are three of the four `chat-search-4ar.2` would
mechanise, which is what makes that bead worth more than its P3 suggests — until it exists,
a threshold-gated bead stays deferred because nobody checked, not because the condition is false.
