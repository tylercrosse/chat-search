# Backlog

Deferred **work**. Deferred **decisions** live in [DECISIONS.md](./DECISIONS.md) — anything
still `open` or marked `Open —` there is a decision, not a task, and is only cross-referenced
from here so it does not get tracked in two places and drift.

Items carry the ADR that justifies them. If an item and its ADR disagree, the ADR wins.

Status legend: `[ ]` not started · `[~]` partial · `[x]` done.

---

## Now

- [x] `git init` — done 2026-07-28
- [x] Run the archiver for real — 1,022 files / 2,220 MB cloned into `~/.chat-archive`
- [x] Schedule it — launchd agent, every 5 minutes (see [Operations](#operations))
- [ ] **Quiet mode for scheduled runs.** `cs archive` prints a full table every run, so the
      log grows ~150 KB/day of "nothing changed". Only report when something was captured,
      or rotate.
- [ ] **`bytes captured` overstates disk cost.** An append re-clones the whole file, so the
      reported figure is file size, not delta. Report allocated blocks instead.

## Operations

Current live setup on this machine, recorded here because it lives outside the repo:

| | |
| --- | --- |
| binary | `~/.cargo/bin/cs` (`cargo install --path crates/cs`) |
| config | `~/.config/chat-search/config.toml` |
| archive | `~/.chat-archive/raw/tylers-macbook-air/` |
| schedule | `~/Library/LaunchAgents/com.chat-search.archive.plist`, every 300 s |
| logs | `~/Library/Logs/chat-search/archive.{log,err}` |

To stop it: `launchctl bootout gui/$(id -u)/com.chat-search.archive`

## Capture

- [x] Mirror capture with APFS clone, change detection, manifest — ADR 18, 19, 20
- [ ] **OpenCode bundling** — ADR 18. 170k files, `layout = "bundle"` is reserved in config
      but unimplemented, so OpenCode is currently **not archived at all**. Dormant since
      2026-02, which is why it was deferred, not because it is unimportant.
- [ ] **ChatGPT export ingest** — 2,011 conversations, 2023-01 → 2026-07, the single biggest
      chunk of the corpus and reachable no other way (the desktop app cache is encrypted).
      An existing export sits at `~/dev/sandbox/chat-history/data/`.
- [ ] **Claude.ai capture** — no local files at all; needs export or API.
- [ ] **Recurring export reminder** — every day without one accrues unrecoverable gaps in the
      two server-side sources. The existing ChatGPT export already ends 2026-07-10.
- [ ] **Watch vs scan** — decides archiver latency and whether a daemon is needed.
- [ ] Compression, when clone divergence starts costing real space — ADR 20
- [ ] Encrypted offsite tier — ADR 20 option D. Solves *disaster*, which cloning does not.
- [ ] `--verify` full-hash pass for periodic paranoia — ADR 19
- [ ] Machine re-key detection after a clone or restore — ADR 17
- [ ] `forget` operation: remove from raw + index, write a tombstone so a rescan cannot
      resurrect it — ADR 9

## MVP

> Find a conversation you half-remember, across ChatGPT, Claude Code and Codex, and open it
> where it lives.

Archive (done) + three importers + real schema + BM25 + a CLI that prints results with a
`resume_cmd`. Ordered so the one question only you can answer — is the ranking any good —
becomes answerable in days rather than weeks.

**Not in it:** embeddings, clustering, `library.db`, any GUI, OpenCode, Gemini, tool-output
search. Skipping `library.db` is safe only while nothing authored is written into
`index.db`; that invariant is easy to hold and expensive to unwind.

**Hard constraint:** `cs search --json` must emit exactly what a GUI would consume — stable
field names, no terminal-width truncation. That is what makes a Raycast extension a
weekend's work (~200 lines of TypeScript shelling out to the binary, whose 4.9 ms p95 cold
start fits a type-ahead budget) rather than a refactor — ADR 12.

**Ordering traps** — these cost rework, and none of them are decisions:

1. Schema before importer #3, or every importer gets rewritten when the DAG lands.
2. Importers read the archive from #1. The PoC reads live source dirs; inheriting that makes
   retroactive reparse silently not work — ADR 1.
3. Fixtures synthetic from the first test, or the repo can never be shared.
4. Query contract before client #2, or the schema is frozen by its consumers.

## Interpret

- [ ] **Port the PoC importers to read the archive, not live source dirs.** They currently
      read `~/.codex/sessions` directly, which breaks reproducibility and makes it impossible
      to reparse conversations that no longer exist upstream — ADR 1
- [x] Codex importer against the real model — 651 conversations, 119,020 messages, zero id
      collisions on the reference corpus
- [ ] Claude Code importer against the real model
- [ ] **Codex legacy pre-`payload` format** — 18 files (2.6%) written before ~2025-12 have no
      `payload` wrapper and no `event_msg` at all, so 121 prose messages across 18
      conversations are silently dropped today. Needs a disjoint code path gated on "line has
      no payload object", which cannot regress the current format.
- [ ] **8 Codex subagent files cannot be linked to their parent** — older rollouts carry
      neither `session_id` nor `parent_thread_id`, so they surface as standalone
      conversations. Recovering them needs a cross-file pass.
- [ ] Lift the duplicated RFC3339 → epoch-millis parser out of `codex.rs` and
      `claude_code.rs` into a shared module
- [ ] OpenCode importer (blocked on bundling above)
- [ ] Gemini CLI importer
- [x] ChatGPT export importer — 2,011 conversations, 14,390 messages; first real exercise of
      the DAG model — ADR 4
- [ ] Archive reader so layout policy never reaches importers — ADR 18
- [ ] Strip slash-command markup from titles (`<command-message>find-skills</command-message>…`
      currently leaks into conversation titles)
- [ ] Derived `surface` and `account` columns — ADR 16
- [ ] Cursor and Antigravity importers — protobuf, 52 conversations, poor value/effort ratio

## Index and search

- [ ] **Real schema**: `parent_id`, `head_id`, `thread` table, `deleted_upstream_at` — ADR 4, 9.
      The PoC index is flat and has none of it.
- [ ] `library.db` and the authored event log — ADR 3
- [ ] Title resolution fold, including the authored override — ADR 8
- [ ] Incremental / tail indexing for the live session — ADR 10
- [ ] Embeddings and hybrid ranking (BM25 + vector, reciprocal rank fusion)
- [ ] Clustering and topic discovery — explicitly a nice-to-have, falls out of embeddings

## Clients

- [ ] CLI search (the PoC has one; it needs rebuilding against the real schema)
- [ ] Quick surface — Raycast first to validate ranking before investing in chrome
- [ ] Deep surface — full window, timeline, conversation reader with tool calls collapsed
- [ ] VS Code extension
- [ ] Theming

## Cross-cutting

- [ ] **Mechanise the "Revisit when" triggers.** 17 ADRs carry one, and nothing checks any of
      them — a trigger with no monitor is a wish. Four are mechanisable today because the
      tools already measure the inputs:

      | ADR | trigger | who could check it |
      | --- | --- | --- |
      | 1 | rebuild exceeds ~60 s | indexer |
      | 6 | corpus exceeds ~50 GB | archiver |
      | 20 | free space < 10 GB, or growth > 1.5 GB/month | archiver |
      | 19 | a source mutates in place without changing size | `--verify` pass |

- [ ] Synthetic golden-file fixtures — a Codex subagent thread, a declared fork, a renamed
      session, a ChatGPT edit-branch. Must be synthetic: real transcripts contain secrets.
- [ ] Keep the archive out of any cloud-synced directory, and decide redaction — ADR 15

## Decisions still open

Tracked in [DECISIONS.md](./DECISIONS.md), listed here only so they are visible:

| ADR | question | blocks |
| --- | --- | --- |
| 14 | subprocess vs daemon | client work |
| 15 | redact at import or at display | sharing anything, or moving the archive off this machine |
| 8 | in-app vs upstream rename precedence | title fold |
| 16 | work/personal account separation | nothing now — effectively forced to "one source" |
| 17 | clone/restore re-key policy | second machine |
| 18 | sync transport | second machine |
| 19 | hash strength | nothing now |
| 20 | compress per-file or per-bundle | compression work |
