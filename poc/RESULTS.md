# PoC results

Two implementations of `SPEC.md` — TypeScript and Rust — built the same index from the
same frozen corpus and were diffed against each other. Machine: Apple M3, 8 cores,
macOS. SQLite 3.51 / FTS5, contentless index, identical pragmas and DDL on both sides.

Corpus: 973 JSONL files, 1.8 GB (666 Codex rollouts + 286 Claude Code sessions),
snapshotted with APFS clones so both implementations saw byte-identical input.

Result: **952 conversations, 150,895 messages, 29,135 prose, 448 MB index.**

## The headline: latency

Twenty runs per variant after three warmups. `startup` is `total - query`, i.e. runtime
boot; `query` is db-open plus the FTS query, which is the floor a daemon would reach.

| variant | total p50 | total p95 | query p50 | startup p50 | viable for type-ahead |
|---|---|---|---|---|---|
| node + `.ts` (type stripping) | 73.9 ms | 174.6 ms | 2.75 ms | 71.1 ms | no |
| node + bundled `.mjs` | 33.0 ms | 36.8 ms | 1.70 ms | 31.3 ms | yes |
| bun + bundled `.js` | 17.9 ms | 20.5 ms | 2.59 ms | 15.3 ms | yes |
| bun `--compile` binary | 17.9 ms | 18.8 ms | 2.50 ms | 15.4 ms | yes |
| rust `--release` | **3.8 ms** | **4.9 ms** | 1.05 ms | 2.7 ms | yes |

Two things matter here and neither was obvious up front:

- **Running `.ts` directly costs ~40 ms per invocation.** The first measurement pass used
  Node's on-the-fly type stripping and made TypeScript look disqualified. Bundling more
  than halves it; Bun halves it again. Anyone benchmarking a TS CLI without bundling
  first will reach the wrong conclusion.
- **The query itself is 1–3 ms in every runtime.** SQLite dominates, so language choice is
  irrelevant to query cost. The entire spread is process startup.

So the subprocess seam survives type-ahead in every variant except unbundled `.ts`.
Rust buys headroom, not feasibility.

## Index build and size

| | build (best of 2) | binary | index |
|---|---|---|---|
| rust | 7.06 s | 2.1 MB | 448 MB |
| bun | 7.15 s | 61 MB | 448 MB |
| node | 7.77 s | 16 KB + 111 MB runtime | 448 MB |

Within 10% of each other — ingestion is I/O and SQLite bound, not CPU bound. Rust's
advantage largely disappears here.

Distribution is the real gap: 2.1 MB versus 61 MB for a self-contained binary.

## The prose/tool split, measured

| kind | messages | text |
|---|---|---|
| `tool_result` | 51,949 | 181.8 MB |
| `tool_call` | 52,823 | 36.9 MB |
| `prose` | 27,670 | **20.1 MB** |
| `reasoning` | 10,941 | 1.2 MB |

Prose is **8.4% of the text**. Indexing prose and tool traffic into one FTS table would
mean 92% of the index is tool sludge competing for BM25 rank against the content you
actually want. The split is worth doing on day one.

## Effort and ergonomics

512 lines of TypeScript versus 646 of Rust for identical behaviour — 26% more, less than
folklore suggests. The Rust version compiled clean on the first attempt and needed no
borrow-checker fights, because the shape is a straightforward pipeline: read bytes, parse
to `serde_json::Value`, push owned `String`s into a `Vec`, write to SQLite.

Where Rust cost real time:

- `serde_json::Value` navigation is verbose next to optional chaining. `p.get("summary")
  .or_else(|| p.get("content"))` is also subtly *wrong* where JS `??` is right: `.get()`
  returns `Some(Value::Null)` for an explicit null, so the fallback never fires.
- `chrono` was needed for what `Date.parse` does natively.
- Default `serde_json` sorts object keys; matching `JSON.stringify` required the
  `preserve_order` feature.

Where Rust was better: `Kind` as an enum made illegal states unrepresentable, and the
match on `(dtype, ptype)` tuples is more readable than the TS if/else chain.

## What the differential test caught

This is the part that justified building it twice. Three real bugs, none of which a single
implementation would have surfaced — each looked like correct code in isolation.

**1. Codex message IDs collided across subagent threads.** A Codex subagent runs in its own
rollout file but *shares its parent's `session_id`* — 13 session_ids span 28 files, and in
every case the extra files are subagents (`thread_source: "subagent"`, with
`source.subagent.thread_spawn.parent_thread_id` naming the parent). Ordinal-based message
IDs therefore collided: **7,637 messages (5%)** silently overwrote each other, and which
copy survived depended on directory iteration order. Scoping the ordinal by rollout
filename dropped collisions to 193.

Forks are a *separate*, cleanly-declared mechanism: 32 rollouts carry `forked_from_id`,
each pointing at a parent that resolves on disk, and each fork gets its own new
`session_id`, so forks never collide. Lineage in Codex is declared, not inferred.

**2. Directory enumeration order leaked into the output.** Rust's `Path: Ord` compares
path *components*, so `<id>/subagents/x.jsonl` sorts before `<id>.jsonl`; JS string
compare does the reverse. With first-write-wins on conversation rows, 13 conversations
got a subagent's first message as their title instead of the real `ai-title`.

**3. First-write-wins was the wrong merge strategy.** Fixed by upserting with
`COALESCE(conversation.col, excluded.col)`, which makes conversation metadata independent
of file order entirely.

After all three fixes, the two implementations agree exactly: 0 message mismatches,
0 conversation mismatches, across 150,895 rows. Bun was added as a third implementation
and also agrees.

## Portability finding

**Bun does not support `node:sqlite`;** it ships an incompatible `bun:sqlite`. Porting
took a one-line import change plus a type-only import change, and nothing else — but a TS
core is not runtime-portable for free, and the choice of Node vs Bun leaks into the data
layer.

## Reading

- BM25 relevance on prose was good enough on the first try to find the exact conversation
  where this project was first discussed, from a four-word query.
- Neither language is disqualified. Rust is 4–5× faster to invoke and 30× smaller to ship;
  TypeScript is ~20% less code and much better at format archaeology.
- The seam matters more than the language: at 1–3 ms query time, a daemon makes every
  variant equivalent. The subprocess seam is what makes startup visible at all.
