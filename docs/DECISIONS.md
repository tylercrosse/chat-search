# Decision log

Thin ADRs. Each entry records _why_, so that when circumstances change you can tell whether the decision still applies — that is the part worth keeping, not the choice itself.

**Status** is `accepted` (agreed, build on it), `proposed` (my recommendation, not yet agreed), or `open` (genuinely undecided).

Terms follow [GLOSSARY.md](./GLOSSARY.md). Measurements come from [../poc/RESULTS.md](../poc/RESULTS.md), taken 2026-07-28 on an Apple M3 against a frozen 973-file / 1.8 GB corpus.

---

## 1. Raw archive is the source of truth; the index is disposable

`accepted` · 2026-07-28

**Context.** ~3.2 GB of transcripts across five tools, in formats that change without notice. Every schema, tokenizer, chunk-size and ranking choice downstream is a guess.

**Decision.** Capture raw transcript bytes append-only and never rewrite them. Treat `index.db` as a pure function of _(raw archive, importer version)_, safe to delete and rebuild at any time.

**Why.** It converts most downstream decisions from irreversible to reversible — you `rm index.db` instead of writing a migration. A full rebuild is 7s, so this costs nothing in practice. The temptation to skip it is that raw is mostly noise (91% tool traffic), but disk is cheaper than the decisions it buys back.

**The invariant that makes this work.** Every column in `index.db` must be a pure function of _(archive, importer version)_. Hold that and any schema change is `rm index.db`, change the importer, rebuild — no migrations, ever.

**Corollary: needing a migration for `index.db` is a bug**, not a chore. It means the index holds something the archive cannot reproduce, which quietly makes the disposable database non-disposable and takes the flexibility with it.

**What this leaves free, and what eventually freezes.** Tables, columns, indexes, tokenizer, ranking and even the id formats are soft while nothing references them. Three things freeze on specific triggers:

| what | freezes when | mitigation |
| --- | --- | --- |
| id formats | the first annotation is written to `library.db` | already designed stable (ADR 2); do not change the format after that point |
| JSON field names | a second client exists | keep changes additive, version the contract |
| cheap rebuild | embeddings land | cache embeddings keyed on `(message_id, model_id)` outside the rebuild path |

The last one matters most. Rebuild is ~7s; embedding 30k messages is minutes. If embeddings sit inside the rebuild path, every later schema change costs minutes and gets avoided — which is how a disposable index stops being disposable.

**Revisit when.** Rebuild exceeds ~60s, or archive storage becomes a real cost.

---

## 2. Stable, source-native composite IDs — never rowids

`accepted` · 2026-07-28

**Decision.** `conversation.id = <source>:<native_session_id>`, `message.id = <conversation.id>:<native_message_id>`. Message ids are scoped by transcript file where the native id is only file-unique. Never expose an autoincrement rowid or a content hash in anything user-visible or referenced.

**Why.** Annotations, stars, notes and read-state all reference these ids. Unstable ids mean a rebuild silently scrambles user data. Measured: unscoped Codex ordinals collided on 7,637 messages (5%) because subagent threads share the parent's session id, and which copy survived depended on directory iteration order.

**Revisit when.** A source appears whose native ids are not stable across exports.

---

## 3. Authored data lives in a separate file from derived data

`accepted` · 2026-07-28

**Decision.** `index.db` (derived, disposable) and `library.db` (authored, precious, backed up). Authored data is an append-only event log: `{ts, machine, target_id, op, value}`, folded last-write-wins per key and projected into queryable tables at build time.

**Why.** If annotations live in the index, "rebuild the index" means "destroy user data," so you stop rebuilding — which breaks decision 1. The event-log shape also makes multi-machine merge concatenate-and-fold, with no conflict resolution. Costs ~30 lines now, and is genuinely unpleasant to retrofit once annotations exist.

**Revisit when.** Never, ideally. This one is load-bearing.

---

## 4. A conversation is a DAG, not a list

`accepted` · 2026-07-28

**Decision.** Messages carry `parent_id`. `conversation.head_id` points at the current leaf and is the only mutable field. The displayed conversation is the walk from head to root; off-path messages remain searchable.

**Why.** Every branching mechanism in these sources is an append _if_ the model can hold two children of one parent, and a destructive mutation if it cannot. Editing a ChatGPT message creates a sibling rather than overwriting — 287 branch points across 234 of 2,011 conversations, leaving 869 message nodes off the current head path. Retrofitting flat→DAG would mean rewriting every importer, dedup, result grouping and the reader UI.

**Consequence.** Search can return messages from abandoned branches. For an archive that is correct — but results need to say so, which is what `head_id` is for.

**Threads are derived, not materialised — for now.** A conversation can hold several threads (one main plus one per subagent), but only **2.1%** of Codex and **4.9%** of Claude Code conversations have more than one, so a `thread` table would be 1:1 noise for the other 95%. Messages instead carry `thread_key` and `is_sidechain`, and the set of threads is `SELECT DISTINCT thread_key`.

Carrying `thread_key` as an explicit column — rather than parsing it back out of the message id, which already encodes it — is what keeps this reversible.

Materialising later is cheap for the same reason the rest of the index is: `rm index.db`, change the importer, rebuild in ~7s (ADR 1). Nothing user-facing references thread ids; annotations target conversations and messages.

**Revisit when.** The reader needs to show thread structure — nesting is real in this corpus (subagents observed at depths 1, 2 and 3, so a subagent has spawned a subagent, which a boolean cannot express), and thread-level metadata (`agent_nickname`, `agent_role`) has nowhere to live without a table. Also revisit if search results need collapsing by thread.

---

## 5. Prose is indexed separately from tool traffic

`accepted` · 2026-07-28

**Decision.** Two FTS5 tables. `fts_prose` is searched by default; `fts_tools` is opt-in. `reasoning` is stored but not indexed for now.

**Why.** Measured on the real corpus: prose is 20.1 MB of 240 MB — **8.4% of the text**. A combined index would be 91% tool sludge competing for BM25 rank against the content you actually want.

**Tool text is clipped to 1 KiB at index time.** Tool traffic was 241 MB of the 285 MB of stored text while prose was 41 MB, and none of it is searched by default. All of it is reproducible from the archive, so by ADR 1 it need not be in the index at all — but the *head* earns its keep, because that is where a tool's name and the start of its arguments live, which is what makes "which conversation ran `cargo build`" answerable. Measured: 549 MB → 292 MB, and the rebuild halved to 10.6s.

Truncation is marked in the stored text with the dropped byte count. A silently shortened tool result reads as a tool that returned little, which is more misleading than an explicit pointer back to the archive. `--tool-text-limit 0` drops tool text entirely (85%).

**Revisit when.** Tool search proves useful enough to want unified ranking with field boosts instead of separate tables — or the 1 KiB clip proves too short to find things by.

---

## 6. SQLite + FTS5/BM25 as the whole storage and search layer

`accepted` · 2026-07-28

**Decision.** One SQLite file, FTS5 external-content indexes (`content='message'`), BM25 with a time-decay multiplier. `sqlite-vec` in the same file when embeddings arrive. Every UI surface is a thin reader.

**Why.** BM25 is built in, there is no server, and Rust/TS/Swift/Python can all read it — which is what makes "open it anywhere" tractable. Measured: query is 1–3 ms regardless of runtime, so the storage layer is not the bottleneck for any surface.

The indexes were `content=''` until 2026-08-01. External content stores no more, since the postings are the same and the text is read back from `message`, which every ranking query already joins by rowid. What it buys is that fts5's auxiliary functions become usable against the index, and `highlight()` is how a snippet marks the word that actually matched (chat-search-6eb.30). Two consequences to know: an unconstrained scan of an fts table now returns every row of `message` rather than only the indexed ones, so anything counting postings must go through `MATCH` or the `_docsize` shadow table; and `message.text` is now *the* content, so the indexer's postings and that column must never drift.

**Rejected — a prefix index (`prefix='2 3 4'`).** Priced 2026-08-01 on the 181k-message / 317 MB index, release, warm, at TUI settings (chat-search-6eb.38). It does exactly what fts5 documents and it does not buy what it was wanted for.

The premise was that walking the vocabulary for a prefix term is most of what a broad typeahead keystroke costs. It is not. Ranking `the`* takes 59.9 ms; ranking `the` — the same query with no prefix in it at all — takes 48.9 ms. The expansion is that 11 ms gap, and a prefix index recovers exactly it (59.9 → 50.4 ms). The other 49 ms is BM25 scoring: `ORDER BY score` has to score every one of the 37,911 matching rows before `LIMIT` can discard them. Asked directly, the doclist merge for `"the"*` is 7 ms against 1 ms with the index, while the same query with the scoring left in is 148 ms against 131 ms. So the two queries this was meant to rescue stay over the 30 ms budget of chat-search-me9.1 after it — `the` 71.6 → 62.1 ms end to end, `pro` 38.3 → 35.5 ms — and cold first touch in a fresh process does not move at all (`pro` 637 ms → 747 ms, `the` 487 → 538, both slightly worse for the larger file).

The one thing it genuinely fixes is already fixed. Marking a row through the corpus index costs 5.2 ms/row for a prefix term and 37 µs/row with a prefix index — a real 140x, and the measurement this bead was filed on. But `highlight::spans_for` already routes prefix terms to a 150-row scratch table where the expansion is free, and that route costs the same either way, so the wall clock is unchanged (`the`* marking: 10.4 ms before, 10.3 ms after).

Nor does it collapse `spans_for`'s two-branch rule, which was the second reason to want it. It moves the branch rather than removing it: prefixes longer than the longest configured length still fall back to the walk, so at `prefix='2 3 4'` the index route wins for `pro`* (4.7 ms vs 8.1) and loses for `commit`* (11.0 vs 2.4) and `learning`* (17.8 vs 15.1). The condition would become "is this prefix longer than the schema's longest configured prefix length", which is a worse rule than the one it replaces — it couples `highlight.rs` to a declaration in `schema.rs` that it currently does not have to know about.

Against that: the index grows 317 → 400 MB (+26%), postings 41 → 123 MB (3x, one full posting list per configured length), and a full rebuild goes from ~10.4 s to ~15.4 s (+48%, medians of three interleaved rounds on a loaded machine). `prefix='2 3'` is the same trade at two thirds the size and buys less, since it leaves 4-character prefixes like `deep`* on the slow path.

Do not re-litigate this without first repricing the BM25 scoring, because that is where the time is. The harness both sets of numbers came from is in commit 015e661 of this branch, deleted after use.

**Revisit when.** Corpus exceeds ~50 GB, or hybrid ranking needs something FTS5 can't do. The open question on ranking cost is no longer the prefix expansion but the candidate ceiling: the ranking pass (`search::rank`) pulls `limit * 50` rows, and `ORDER BY bm25(...)` scores the whole match set to fill it (chat-search-6eb.29).

---

## 7. Determinism rules for importers

`accepted` · 2026-07-28

**Decision.** (a) Sort the discovered file list by the _string_ form of the path. (b) Merge conversation metadata field-by-field with `COALESCE(conversation.col, excluded.col)`, never first-write-wins.

**Why.** Both were found by differential-testing two implementations. Directory enumeration order is not guaranteed and differs between runtimes — Rust's `Path: Ord` compares components, so `<id>/subagents/x.jsonl` sorts before `<id>.jsonl` while a string compare does the reverse. With first-write-wins, 13 conversations took a subagent's opening line as their title instead of the real name. These are silent-corruption bugs: the index looks fine and contains the wrong thing.

**Revisit when.** Never. Add cases as new sources appear.

---

## 8. Titles are a fold, not a stored value

`accepted` · 2026-07-28

**Decision.** Resolve as `authored override > custom-title > ai-title > first user message`. An in-app rename is authored and wins until cleared.

**Why.** Claude Code already models renames as append-only `custom-title` events (re-emitted on every save, observed up to 44× in one session), so rename history is fully recoverable and rebuild-safe. Ignoring `custom-title` loses the most useful titles — 35 sessions carry one, and manual renaming is a primary organising habit here.

**Sub-question, settled 2026-08-01.** If a conversation is renamed both in-app and upstream, **authored wins**, unconditionally — the fold order above is the whole rule, with no timestamp comparison anywhere in it.

Last-write-wins was the alternative and is rejected. It depends on two independent sources' clocks being comparable, which nothing here establishes: an in-app rename is stamped by this tool and an upstream one by the vendor, and a fold that silently prefers whichever clock ran fast is worse than one that is merely opinionated. The opinion is also the right one — an in-app rename is a deliberate act by the user, while an upstream rename is usually the vendor regenerating a title from the conversation's own content. Preferring the vendor's regeneration over a name the user chose is the failure mode worth designing against, and it is the one last-write-wins would produce every time the vendor re-titled after a rename.

Consequence for the fold: `authored override` is unconditional, so `custom-title` and below are only ever consulted when no override exists. `chat-search-6eb.15` implements this and carries the test.

---

## 9. Deletion keeps data; forgetting is explicit

`proposed` · 2026-07-28

**Decision.** Three cases. Upstream-deleted and retention-pruned both _keep_ the content and set `deleted_upstream_at` (flagging that reopening will fail). A deliberate `forget` removes from raw and index and writes a tombstone so the next scan cannot resurrect it from a source file that still exists.

**Why.** Outliving upstream deletion is the point of an archive — Claude Code's 30-day retention already destroyed everything before 2026-06-19. But a searchable index of everything is also a concentration of risk, so deliberate removal has to be possible and has to stick.

**Revisit when.** Deciding redaction policy, which is the adjacent unsolved problem.

---

## 10. No incremental indexing yet

`proposed` · 2026-07-28

**Decision.** Build change _detection_ — `(path, size, mtime, hash of first 64 KB, importer_version)` — but respond to change with a full rebuild. Tail-append only files touched in the last few minutes, for the live session.

**Why.** Rebuild is 7s and ingestion is I/O-bound, so incremental indexing is pure complexity right now. Patching an FTS index through deletes is where the bugs live. The tail path then only ever handles pure append, the easy case.

**Revisit when.** Rebuild exceeds ~60s, or live-session latency becomes a felt problem.

---

## 11. Multi-machine: sync raw, never the database

`proposed` · 2026-07-28

**Decision.** Sync `raw/<machine>/…`; each machine builds its own index. Never put a local path in an identity. Namespace archives by machine from day one.

**Why.** SQLite over file-sync is a known corruption path. Raw is append-only immutable files, the easiest possible thing to sync. Claude Code encodes the working directory into its project directory name, so path-derived identity breaks on a second machine — and `cwd` changes mid-session in 26% of sessions anyway, making it an attribute rather than an identity.

**Revisit when.** Real-time cross-machine sync becomes a requirement rather than a nice-to-have.

---

## 12. Client seam is a JSON protocol over a swappable transport

`proposed` · 2026-07-28

**Decision.** Define one JSON request/response contract. Ship it over argv today. The same contract can run over stdin/stdout or a unix socket later without changing client code.

**Why.** This is the un-boxing move — it lets the daemon question stay open and makes a core rewrite contained. Measured: the query is 1–3 ms in every runtime and the entire spread between runtimes is process startup, so the seam choice matters more than the language.

**Where it is written down.** [JSON-CONTRACT.md](./JSON-CONTRACT.md), field by field, for `cs search --json`. It was not, for the first week of the seam being real, and "one JSON contract" turned out not to mean the same thing to the code and to a client: the first surface to decode it without reading the Rust structs typed a nullable field as non-optional and threw at row 54 of a 60-row page (chat-search-me9.27). A contract nobody wrote down is a contract each client reconstructs by observation, and observation only ever reaches the states that happen to be common.

**Revisit when.** A transport other than argv is actually needed. Decision 14 settled daemon vs subprocess in favour of subprocess (2026-07-29), so argv is the only transport in play; `--stdio` is the next one up, and a socket only after that.

---

## 13. Core language

`accepted` · 2026-07-28 — **Rust**

**Context.** Measured, same program written twice plus a third runtime:

| variant                       | total p50  | total p95  | binary         |
| ----------------------------- | ---------- | ---------- | -------------- |
| node + `.ts` (type stripping) | 73.9 ms    | 174.6 ms   | 111 MB runtime |
| node + bundled `.mjs`         | 33.0 ms    | 36.8 ms    | 111 MB runtime |
| bun (bundled or compiled)     | 17.9 ms    | 20.5 ms    | 61 MB          |
| rust `--release`              | **3.8 ms** | **4.9 ms** | **2.1 MB**     |

Index build was 7.1–7.8s across all three — I/O-bound, no meaningful difference. Code volume: 512 lines TS vs 646 Rust.

**Decision.** Rust, for the invocation headroom, the 2.1 MB single-binary artifact, and deliberately as a chance to build real fluency in the language.

**Why.** Neither language was disqualified — with bundling, every runtime clears the type-ahead budget through a subprocess — so this was a free choice on performance grounds and the learning goal is a legitimate tiebreaker. Rust cost only 26% more code (646 vs 512 lines) and compiled clean first try, because the shape of this work (read bytes → parse → own strings → write SQLite) is the easy part of the language.

**Consequences.** The PoC exercised none of the harder parts: concurrency for parallel imports, async if the core is daemonised, FFI if a Swift surface appears. Format archaeology is genuinely worse in Rust — `serde_json::Value` navigation is verbose, and `p.get("a").or_else(|| p.get("b"))` is subtly wrong where JS `??` is right, because `.get()` returns `Some(Value::Null)` for an explicit null. Decision 12's normalized-JSONL seam keeps a TypeScript importer available if a source turns out to be too gnarly to be worth fighting in Rust.

**Note.** Running `.ts` directly costs ~40 ms per invocation. Any future benchmark must bundle first, or it will reach the wrong conclusion — the first pass here did.

**Revisit when.** A Swift-native surface makes FFI overhead a real cost, or an importer proves genuinely impractical in Rust.

---

## 14. Client seam: no daemon — link the core in Rust, spawn the CLI elsewhere

`accepted` · 2026-07-29 — **no daemon**

**Context.** The measurements this was decided against, recorded 2026-07-29 for the 293 MB index (post-clip, decision 5):

| step | cost |
| --- | ---: |
| in-process query (`cs-core`, index already open) | 1.4–6.4 ms |
| spawn `cs` + open the 293 MB index | ~3 ms |
| **`cs search --json`, end to end** | **~9 ms** |

That is the whole argument. The daemon's prize is the ~3 ms of spawn-and-open, on a path that already fits a type-ahead budget without it. Decision 12 deliberately left this reversible so it could be decided against numbers rather than taste, and the numbers are now in.

**Options.**

_A. Resident daemon, clients talk to it over a unix socket_

- Pro: every client in every language gets the 1.4–6.4 ms floor, with no per-query spawn. The saving is per _keystroke_, not per session, so it compounds with typing speed.
- Pro: it is the only place cross-query state can live — a warm page cache, prepared statements, an embedding model held in memory once `sqlite-vec` lands (decision 6), a tail-follower for the live session (decision 10).
- Pro: a single resident process is the natural owner of the write path if incremental indexing ever lands, which sidesteps writer contention between the archiver and N readers.
- Con: a socket protocol is not just decision 12's JSON body — it is framing, discovery, and a version handshake between a client and a daemon that may be older or newer than it.
- Con: lifecycle. Start on demand or under launchd, reap stale sockets, and restart when the binary is upgraded underneath a running instance. This machine already runs one launchd job for the archiver; this would be a second.
- Con: **cache staleness, which is the real cost.** A rebuild deletes and recreates `index.db` (decision 1); a daemon holding an open handle then reads a deleted inode and serves stale results silently. Every long-lived cache in front of a disposable database is that bug waiting to happen.

_B. Rust clients link `cs-core`; everything else spawns `cs search --json`_ **← recommended**

- Pro: Rust surfaces (the CLI, a TUI) cross no process boundary at all and get the 1.4–6.4 ms floor without a daemon existing. The daemon's headline benefit is already available to the clients most likely to need it.
- Pro: ~9 ms end to end for everyone else, which leaves the bulk of a type-ahead budget for the client's own rendering.
- Pro: staleness is structurally impossible — each query opens the index fresh, so a rebuild underneath is picked up on the next keystroke with no invalidation logic.
- Pro: crash isolation, trivial parallelism, and nothing to debug when a client hangs. There is no "is it running?" state.
- Con: no cross-query state. Re-establishing it costs 3 ms today; it would cost far more if a model has to be loaded per query.
- Con: non-Rust clients marshal JSON through argv and stdout instead of calling a function, so the contract has to be genuinely stable (decision 12).

_C. Ship `cs-core` as a C-ABI shared library for non-Rust clients_

- Pro: the in-process floor without a daemon or a process boundary, for Swift or Node surfaces too.
- Pro: no lifecycle at all — the host process owns it.
- Con: a C ABI to design and keep stable, plus a per-platform build matrix, in exchange for 3 ms.
- Con: a panic in the core takes the host down. Decision 13 already flags FFI as the part of Rust this project has not exercised.

**Decision.** B — _accepted 2026-07-29; A rejected on measurement._ No daemon. Rust clients depend on `cs-core` as a library and pay only the query. Every other surface spawns `cs search --json` and pays ~9 ms. C stays in reserve for a Swift-native surface, where the process boundary is more awkward than the ABI.

**Why.** A daemon is a permanent structural cost — a protocol, a lifecycle, and a stale-cache failure mode — bought with a one-off 3 ms saving that the budget does not need. It is also the wrong shape for this system specifically: decision 1 makes the index disposable and rebuilt from scratch, and a resident process caching a database designed to be deleted is a contradiction that shows up as wrong results rather than as an error.

**The cheap middle step, if it comes to it.** Before a daemon there is `cs serve --stdio`: one long-lived process per client, requests on stdin, responses on stdout, the same JSON contract. It removes the spawn cost while leaving socket discovery, daemon lifecycle and cross-client cache coherence out of it, because the client owns the process's lifetime and its death. Try that before anything resident.

**Consequence — the JSON contract is now load-bearing for real.** Argv in, JSON on stdout, and a nonzero exit is part of the interface. Clients must treat "no index yet" and "index being rebuilt" as first-class states, not as transport errors, because they will hit both.

_2026-08-04, `chat-search-me9.28`:_ the contract now carries the distinction, because a client could not draw it — a rebuild in place made queries return silently partial result sets for ~5s, with exit 0 and nothing in the body saying so. A rebuild is assembled in a sibling file and renamed over the target, so a reader sees the whole old index or the whole new one, and the four states travel as names a client can branch on: `no_index` and `building` as an `error.code` beside the nonzero exit, `ready` and `rebuilding` as `index_state` on a body that is complete either way. See `cs_core::build`.

**Revisit when.** Spawn-plus-open p95 passes ~20 ms — far enough above today's ~3 ms that the per-keystroke path stops leaving room for the client to render, most plausibly reached as the index grows well past 293 MB or as `cs` accumulates startup work. Or when a client needs state that only a resident process can hold: a warm embedding model, a live tail of the running session, or an incremental writer that must not be re-established per keystroke. Reaching either means starting with `--stdio`, not with a socket.

---

## 15. Redaction and secret handling

`open` · 2026-07-28

**Context.** Agent transcripts contain env vars, tokens and file contents; the index concentrates all of it into one file. At minimum this must stay out of cloud-synced directories.

**Not yet decided.** Whether to redact at import (destructive, breaks decision 1 unless raw is kept intact) or at display (safer, but the index still holds secrets).

**Decide by.** Before any surface is shared or any archive leaves this machine.

---

# Identity schema

Decisions 16–19 are the ones the archiver depends on. Unlike the index, the raw archive is never rebuilt (decision 1), so these are the most expensive decisions in the project to get wrong — changing any of them later means rewriting the archive, which breaks the append-only property everything else relies on.

The index schema — DAG, `head_id`, kinds, FTS config, ranking — is deliberately _not_ in this group. It is disposable and can stay fluid.

---

## 16. Source identifier is the watched location, not the surface

`accepted` · 2026-07-28

**Context.** `conversation.id = <source>:<native_session_id>`, and every annotation in `library.db` targets those ids. The source string is therefore permanent. It is also not obvious: seven distinct surfaces write into `~/.codex/sessions` — `codex_vscode` (559), `Codex Desktop` (60), `codex_work_desktop` (30), `codex-tui` (13), `codex_cli_rs` (3), `codex_exec` (1), `codex-chrome-extension-sidepanel` (1). Claude Code likewise: `claude-vscode` (25,135), `cli` (1,711), `sdk-py` (225).

**Options.**

_A. Source = semantic surface_ (`codex-vscode`, `codex-desktop`, …)

- Pro: search can filter by surface without a join; ids are self-describing.
- Con: the surface is only knowable by _reading file contents_, so the archiver would have to parse — exactly what we want it not to do.
- Con: vendors add and rename surfaces (`codex-chrome-extension-sidepanel` appeared once); every addition is a new permanent id namespace.

_B. Source = watched location_ (`codex`, `claude-code`, `opencode`, `gemini-cli`, `chatgpt-export`) **← recommended**

- Pro: determinable from the path alone; the archiver stays dumb.
- Pro: stable — the set changes only when you start watching a new directory.
- Pro: surface becomes a derived column on `conversation`, free to change and recompute.
- Con: cannot filter by surface without the index having parsed it (acceptable — that is what the index is for).

_C. Source = location, split per account_ (`codex`, `codex-work`, …)

- Pro: keeps a work/personal boundary explicit and permanent.
- Con: account is content-derived, same problem as A; and one directory may hold both.

**Decision.** B — _accepted 2026-07-28._ Source is the location, frozen at the point you start watching it. Surface (`originator`/`entrypoint`) and account are derived columns.

A "watched location" is one entry in the archiver config; the `id` is the permanent source name:

```toml
[[source]]
id      = "codex"              # permanent — part of every conversation id
path    = "~/.codex/sessions"
layout  = "mirror"
include = "**/rollout-*.jsonl"
```

**Decisive evidence against A.** Within a _single conversation_ the files disagree about their surface: in session `019f760a` the main thread carries `source: "vscode"` while its guardian subagent carries `source: {"subagent": {...}}`. Surface is a **thread** attribute, not a conversation attribute, so it cannot serve as conversation identity. That is what makes A incoherent rather than merely awkward.

**Open — work/personal separation.** `codex_work_desktop` (30 rollouts) suggests a separate work account sharing `~/.codex/sessions`. Options: (i) ignore, treat as one source and filter by derived account; (ii) make it a distinct source so it can be excluded from sync or archived to a different root; (iii) exclude at capture entirely. This is the one part of this ADR that _cannot_ be retrofitted, since it changes conversation ids. Worth an explicit call rather than a default.

**Revisit when.** A new directory is watched, or a confidentiality boundary needs to become a storage boundary.

---

## 17. Machine identity is a generated UUID with a human alias

`accepted` · 2026-07-28

**Context.** Decision 11 namespaces archives by machine so several machines can merge. Available identifiers on this Mac: `IOPlatformUUID` (hardware), hostname (`Tylers-MacBook-Air.local`), ComputerName (`Tyler's MacBook Air`).

**Options.**

_A. Hostname_

- Pro: zero setup, human-readable paths.
- Con: changes when the machine is renamed, and macOS mangles it (`…(2).local`) on network collisions. A rename silently forks the archive namespace.

_B. `IOPlatformUUID`_

- Pro: stable across OS reinstalls, needs no state.
- Con: changes on logic-board replacement.
- Con: it is a hardware identifier; embedding it in paths that may be synced or shared is needless exposure.

_C. Generated UUID stored at the archive root_

- Pro: fully under our control, survives renames and hardware service.
- Con: opaque paths (`raw/8f3c…/codex/`) are unpleasant to navigate by hand.

_D. User-chosen slug in the path, generated UUID as canonical id_ **← recommended**

- Pro: readable paths (`raw/mba/codex/…`) plus a stable identity that survives renaming the slug — the directory can be renamed and `.machine.json` proves continuity.
- Pro: no hardware identifier leaves the machine.
- Con: two identifiers to keep in sync; needs one interactive prompt on first run.

**Decision.** D — _accepted 2026-07-28._ `raw/<slug>/.machine.json` holds `{id, alias, created_at, hostname_at_creation}`. The UUID is canonical for provenance and dedup; the slug is cosmetic and renameable.

**Correction to the original draft:** the slug should be **auto-derived, not user-selected**. Slugify `scutil --get ComputerName` at first run — "Tyler's MacBook Air" → `tylers-macbook-air` — with a `--machine-slug` override. Because it is captured once rather than read live, renaming the Mac later cannot fork the namespace. Zero prompts in the common case.

Example slugs: `tylers-macbook-air` (auto), or overridden to `mba`, `studio`, `work-mbp`.

**Why a slug at all,** rather than using the UUID as the path component: the archive is the one artifact that must remain comprehensible under failure. `raw/mba/codex/2026/07/28/rollout-….jsonl` can be understood at a glance; `raw/8f3c1d2e-4a91-…/…` cannot.

**Open — clones and restores.** Restoring a backup onto a new machine, or cloning a disk, yields two archives claiming one UUID. Options: (i) do nothing and accept occasional double-capture, deduped later by conversation id; (ii) detect on startup when `hostname_at_creation` no longer matches and prompt to re-key; (iii) re-key silently and record a `machine_forked_from` link. (ii) is cheap and I lean toward it, but it is a real call.

**Revisit when.** A second machine is actually set up.

---

## 18. Archive layout: mirror whole-file sources, bundle fragmented ones

`accepted` · 2026-07-28

**Context.** Sources fall into two shapes, and the split is extreme:

| source      |   files | bytes | shape                           |
| ----------- | ------: | ----: | ------------------------------- |
| opencode    | 170,037 |  1.2G | many fragments per conversation |
| codex       |     686 |  1.5G | one append-only file per thread |
| claude-code |     454 |  314M | one append-only file per thread |
| gemini      |      37 |  179M | one file per project            |

OpenCode is 99.3% of the file count but 30% of the bytes — average `part` file is 3,793 bytes. Its paths are `session/<project>/<ses-id>.json`, `message/<ses-id>/<msg-id>.json`, `part/<msg-id>/<prt-id>.json`, so fragments _can_ be grouped into conversations by path structure alone, without parsing content.

**Options.**

_A. Mirror every source 1:1_

- Pro: trivially simple; archive is browsable; restore is a copy.
- Pro: for the three whole-file sources this is already the natural unit.
- Con: 170k files per scan for OpenCode; syncing that many small files over Syncthing/git is genuinely painful.
- Con: no dedup across snapshots or machines.

_B. Content-addressed store for everything_ (`objects/<sha256>` + manifest)

- Pro: dedup and immutability for free; versioning falls out — each version is a new object.
- Pro: identical content on two machines converges automatically.
- Con: unreadable without the manifest; loses the "just look at the files" property that makes an archive trustworthy.
- Con: still 170k objects — solves dedup, not file count.
- Con: needs orphan GC, which is a destructive operation inside an append-only store.

_C. Bundle every conversation into one file_

- Pro: uniform handling; file count collapses everywhere.
- Con: for Codex/Claude Code it is a pointless repack of what is already one file per thread.

_D. Hybrid — mirror whole-file sources, append-only bundle for fragmented ones_ **← recommended**

- Pro: OpenCode collapses from 170k files to ~800; the other three stay byte-identical mirrors that you can read with `less`.
- Pro: bundling is path-only (two-pass: `message/` paths give message→session, then group `part/` by message), so the archiver still never parses content.
- Pro: fragments are immutable once written, so the bundle is genuinely append-only — new fragments are appended, nothing is rewritten.
- Con: two code paths, and a bundle format to define.

**Decision.** D — _accepted 2026-07-28; B measured and rejected._ Per-source `layout: mirror | bundle` policy. Bundle format is JSONL, one line per fragment: `{"path": "<relative path>", "content": <verbatim json>}` — lossless, reversible, greppable, and diff-friendly.

**Why B (content-addressed store) was rejected — measured, not assumed.** CAS stores a new object every time a file changes, and every CLI source here produces files that _grow throughout a conversation_. Modelling capture of the 676 real Codex rollouts (1.51 GB on disk, median session span 19 min, long tail to 6 h):

| scan interval | CAS would store | vs mirror |
| ------------- | --------------: | --------: |
| 5 min         |        291.9 GB |  **193×** |
| 15 min        |         98.0 GB |   **65×** |
| 60 min        |         25.4 GB |       17× |
| 4 h           |          7.3 GB |      4.9× |

CAS is excellent for _immutable_ blobs (git's case: a commit never changes) and pathological for _append-only growing_ files. It also fails to deliver the benefit hoped for — OpenCode remains 170k objects, merely with unreadable names — and it forfeits graceful degradation, since a mirrored archive is still readable with `less` if the manifest is lost while a CAS archive is anonymous blobs. For the one component whose failure is unrecoverable, that matters.

The genuinely useful part of CAS — "do not re-store what has not changed" — is already provided by `prefix_hash` in option D. Content hashing as a _check_ is right; content hashing as a _storage layout_ is what breaks.

**Supporting measurement for bundling.** OpenCode's 170,035 files are 0.64 GB apparent but **1.16 GB allocated — 1.82× block waste**, since the average 4,025-byte file still consumes a 4 KB block. Bundling recovers ~0.5 GB before any compression.

**Consequence — keep layout out of importers.** A per-source layout policy would otherwise couple parsing to storage: the OpenCode importer would need to know it is reading bundles while the Codex importer reads mirrored files. Interpose an **archive reader** that yields `(logical_path, bytes)` regardless of physical layout, so every importer sees the source's original shape and layout stays free to change.

**Sub-decision — changed files.** Source transcripts grow. Options: (i) copy the whole file each time, overwriting; (ii) append only the delta; (iii) keep every version. Recommendation: **overwrite when the new file is a strict superset** (prefix hash matches and size grew) — monotonic, so nothing is lost, and far simpler than delta-appending. When the prefix hash _changes_, the file was rewritten or compacted: preserve the old copy under `_superseded/<name>.<first-seen>.jsonl` and start fresh.

**Open — sync transport.** Even at ~1,900 files the archive is 3.2 GB and growing. Options: (i) sync the raw tree directly and accept it; (ii) generate periodic pack files for transport while keeping the tree canonical (git's loose-objects/packfile split); (iii) do not sync raw at all, and treat each machine's archive as independent with only `library.db` merged. (iii) is the cheapest and may be right — worth deciding only when a second machine actually exists.

**Revisit when.** A new source has a shape neither policy fits, or sync becomes real.

---

## 19. Manifest is an append-only event log inside the archive

`accepted` · 2026-07-28

**Context.** Change detection needs per-file state: `(path, size, mtime, prefix_hash, first_seen, last_seen, deleted_upstream_at)`. Two of those — `first_seen` and `deleted_upstream_at` — are _observations over time_. They cannot be recomputed from a snapshot of the archive, which means they are not derived data and cannot live in a disposable index.

**Options.**

_A. Manifest lives in `index.db`_

- Pro: no mutable state inside the archive; one database to manage.
- Con: **breaks decision 1** — `rm index.db` would destroy `first_seen` and `deleted_upstream_at` permanently, since neither is recoverable from raw bytes.

_B. Manifest as a SQLite file in the archive_

- Pro: fast queries, easy to update in place.
- Con: mutable binary state inside an append-only store.
- Con: SQLite over file-sync is the corruption path decision 11 explicitly avoids.

_C. Append-only JSONL event log in the archive_ **← recommended**

- Pro: same shape as decision 3's annotation log — append-only, merges by concatenation, syncs safely, and current state is a fold.
- Pro: keeps the archive fully self-describing with no mutable files in it.
- Pro: gives free capture history — you can see when a file first appeared and when it vanished.
- Con: needs projection into a queryable table for fast lookup (rebuilt into `index.db`, which is fine because the log is the source of truth).

**Decision.** C — _accepted 2026-07-28._ `raw/<slug>/manifest/<yyyy-mm>.jsonl`, one event per _change_:

```jsonc
{
  "ts": 1785257068000,
  "op": "seen|appended|rewritten|vanished",
  "source": "codex",
  "path": "2026/07/28/rollout-….jsonl",
  "size": 184320,
  "mtime": 1785257000000,
  "prefix_hash": "sha256:…",
}
```

Nothing is appended when a scan finds no change, so the log grows with actual change rather than with scan frequency. `deleted_upstream_at` on a conversation is a fold over `vanished` events for its files.

**Open — hash strength and coverage.** Options: (i) hash only the first 64 KB (fast, catches rewrites, misses mid-file edits); (ii) full-file hash (certain, but ~3.2 GB of hashing per scan); (iii) first 64 KB plus size plus mtime as a composite (fast, and a mid-file edit without a size change is implausible for append-only logs). I lean (iii), with an opt-in `--verify` full-hash pass for periodic paranoia.

**Revisit when.** A source turns out to mutate files in place without changing size.

---

## 20. Archive compression and storage location

`accepted` · 2026-07-28

**Context — the archive will not fit.** Measured growth, bytes written per month:

| month       |   codex | claude-code | opencode | gemini |    total |
| ----------- | ------: | ----------: | -------: | -----: | -------: |
| 2026-05     |   70 MB |           0 |        0 |      0 |    70 MB |
| 2026-06     |  288 MB |       19 MB |        0 |      0 |   306 MB |
| **2026-07** | 749 MB  |    283 MB   |        0 |      0 | **1,032 MB** |

Three-month mean is 469 MB/month, but July is 1,032 MB with three days still to run, and Claude Code has only been accumulating since 2026-06-19 because of the retention pruning — now that `cleanupPeriodDays` is raised, its ~300 MB/month is permanent rather than rolling off. Call it **~1 GB/month and rising**.

| horizon  | uncompressed | at 3× compression |
| -------- | -----------: | ----------------: |
| today    |       3.2 GB |                 — |
| +1 year  |       ~15 GB |             ~5 GB |
| +3 years |       ~40 GB |            ~13 GB |
| +5 years |       ~65 GB |            ~22 GB |

**This machine has 17 GB free (96% full).** An uncompressed archive consumes the remainder in roughly 14 months — and on day one it roughly _doubles_ the on-disk transcript footprint, since the sources stay where they are.

**Measurements** (2026-07-28, this corpus):

| lever                              |                             result |
| ---------------------------------- | ---------------------------------: |
| APFS clone of 1.5 GB codex         | **3.2 MB consumed** — free         |
| clone after source append          |    only the delta allocated        |
| zstd -3 / -9 / -19 on a rollout    |         2.98× / 3.10× / 3.13×      |
| opencode per-file zstd             |                              4.7×  |
| opencode per-file zstd + dictionary|                              7.3×  |
| **opencode bundled, then zstd**    |                         **10.3×**  |
| cap tool_result at 32 KB / 8 KB    |            3% / 27% of text saved  |

Three findings reshape the options. **Compression level is irrelevant** — use `-3`, since `-19`
buys 5% for far more CPU. **Bundling more than doubles the ratio** for fragmented sources
(4.7× → 10.3×), an argument for ADR 18's option D independent of file count. And **capping tool
output does not pay**: it is a fat middle, not a few giants (p50 718 B, p99 40 KB), so real
savings would require dropping it wholesale — irreversible, and a breach of ADR 1.

**Options.**

_A. APFS clone on the internal disk_ **← recommended for v1**

- Pro: measured at ~0 cost; growth costs only diverged blocks.
- Pro: zero loss, zero format change, and it can be layered with compression later.
- Pro: the cost curve is exactly right — shared blocks are only charged once the source is
  deleted upstream, i.e. you pay only for data that would otherwise have been lost.
- Con: same volume only, so it is **not** disaster protection — same failure domain.
- Con: mutually exclusive with compressing the same copy, since a clone shares the source's
  uncompressed blocks.

_B. Compressed (zstd -3) on the internal disk_

- Pro: ~3× on rollouts, ~10× on bundled fragments; a single self-contained tree.
- Con: not directly greppable; `prefix_hash` must be computed on _uncompressed_ content.
- Con: forfeits cloning.

_C. On an external / secondary volume_

- Pro: removes the space constraint entirely.
- Con: unavailable when unmounted — bad for an always-on safety net; also forfeits cloning.

_D. Encrypted object storage (Glacier Deep Archive or similar)_

- Pro: solves disaster recovery, which A/B/C do not; ~$1/TB/month is negligible.
- Pro: privacy is addressable with client-side encryption (`age`, `rclone crypt`, restic) —
  the provider only ever sees ciphertext.
- Con: the real risk is **key management**; an archive you cannot decrypt is worse than none.
- Con: retrieval measured in hours, so cold tier only.

_E. Content-defined chunking backup tool (borg / restic / kopia)_

- Pro: dedups growing files natively, plus compression and encryption in one package.
- Con: opaque format — forfeits the "readable with `less` when everything is on fire"
  property that was decisive against CAS in ADR 18.

**Decision.** **A** — _accepted 2026-07-28_ — with the archive root configurable and bundling on for fragmented
sources. Cloning defers the space problem by years at zero loss, which buys time to add B when
divergence actually starts costing. D is worth adding when the concern becomes _disaster_
rather than _space_. Those are different problems, and cloning addresses only the second.

**Rejected — stripping tool output.** Measured above; it would be the only lossy lever and it
is not needed, because clone + bundle + compress solve the problem losslessly.

**Open sub-question — compress what, and when.** Per-file (independently readable) or
per-bundle (better ratios). A "clone-first, compress-on-divergence" policy — compress an
archived file only once its source disappears and it starts occupying real blocks — is the
best space/effort curve but adds a background compaction job. Probably too clever for v1.

**Revisit when.** Free space drops below ~10 GB, or measured growth exceeds 1.5 GB/month.

## 21. Server-side sources are fetched into the archive, never into the index

`accepted` · 2026-07-30

**Context — three surfaces are dark and they are dark for different reasons.** The index holds
2,935 conversations from `chatgpt-export`, `codex` and `claude-code`. Gemini used through the
web, Claude used through claude.ai, and ChatGPT used through the web contribute nothing, and the
newest ChatGPT export here ends 2026-07-10. ADR 16 defines a source as a *watched location*,
which is exactly the definition a server-side source does not satisfy — it has no path.

A survey of this machine on 2026-07-30 found the constraint is not uniform, so a single policy
would be wrong. Details in [FORMAT-NOTES](./FORMAT-NOTES.md); the load-bearing facts:

| surface | what is actually reachable |
| --- | --- |
| Claude desktop (Cowork / Chat tabs) | **30 MB of plaintext `audit.jsonl` already on disk** |
| claude.ai | server-side; but `sessionKey` + `lastActiveOrg` sit in the app's own cookie jar, and the endpoint it calls is known |
| ChatGPT | server-side; local store is AES-256-GCM behind a Keychain **access group**, so the key is not derivable by anyone but OpenAI |
| Gemini web | server-side; no API exists, but Takeout schedules **every 2 months → Google Drive** |
| Gemini / ChatGPT desktop apps | verified to store no conversation content at all |

**The architectural question is where network code is allowed to live**, because ADR 1 makes
importers pure functions over `(logical_path, bytes)` with no filesystem access, and the whole
reparse-the-archive-against-a-better-parser property depends on it.

**Options.**

_A. Manual export only_

- Pro: zero new machinery; `chatgpt-export` already proves the shape works.
- Pro: no credentials, no ToS surface, no breakage when a vendor changes an endpoint.
- Con: every day without an export is an unrecoverable gap, and the gap is unbounded because
  nothing forces the export to happen.

_B. A fetcher that writes export-shaped files into a watched directory_ **← recommended**

- Pro: **preserves ADR 1 and ADR 16 exactly.** The fetcher is not an archiver and not an
  importer; it materialises bytes into a directory the archiver already scans. `cs-archive` and
  `cs-import` stay offline and pure, and the archive stays reparseable.
- Pro: reuses the credentials already on the machine rather than asking for new ones.
- Pro: `?tree=True` returns branches the claude.ai UI does not show, so this is strictly richer
  than the official export, not merely fresher.
- Con: unofficial endpoint. `__cf_bm` rotates every ~30 min, `sessionKey` expires monthly, and
  prior art needs Chrome TLS impersonation to get past Cloudflare's fingerprinting.
- Con: reading the cookie jar means Keychain access and a decrypt step — new dependencies
  (`cookie-scoop`-shaped) for something orthogonal to search.

_C. An MV3 browser extension sidecar_

- Pro: the only route that survives Cloudflare indefinitely, because it *is* the browser.
- Con: a second codebase in a second language, and an extension cannot read
  `~/.claude/projects` — so it can never be this tool's primary shape, only an appendage.
- Con: contradicts the framing in [README](../README.md) that ~80% of the corpus is reachable
  without a browser.

_D. Scheduled Takeout → Drive_

- Pro: the only **officially sanctioned** automation of any of the three surfaces.
- Con: 2 months is the shortest cadence offered, so staleness is bounded but bounded loosely.
- Con: Gemini only.

**Decision.** **Per surface, because the constraint is per surface** — _accepted 2026-07-30_.
Claude desktop becomes an ordinary local source, no fetching involved. claude.ai takes **B**.
ChatGPT stays on **A**. Gemini web takes **D**, with its downloaded archives landing in a
watched directory exactly as B's output does.

**The invariant that makes B legal, stated so it is not eroded later:** a fetcher's only output
is files in a watched directory. It never writes to `index.db`, never calls an importer, and
lives outside `cs-archive` and `cs-import`. If the fetcher is deleted, the archive still
reparses and the index still rebuilds. Anything that cannot be expressed as "write bytes to a
path" does not belong in it.

**Constraint on how snapshots are named — this one is a data-loss trap.** `_superseded/` is
written by `capture.rs` and **read by nothing**; `ArchiveReader::files()` walks
`<machine_dir>/<source_id>` only, and `_superseded` is a sibling. A fetcher writing to a stable
path would classify every run as `Rewritten` and silently retire the previous snapshot out of
the index. Each run must therefore land under a **unique path**, the way each ChatGPT export
unpacks into its own `Conversations__<hash>-chatgpt-NNNN/`. Index-side dedup then folds the
overlap on the conversation id, which is already tested.

**Rejected — C, the browser extension.** Polylogue reached for it and recorded the reason in its
own architecture notes ("Cloudflare friction on Claude.ai"), so the route is real. It is rejected
here because an extension cannot reach the 80% of this corpus that is local, which makes it a
second codebase serving a minority of the data. If B proves unmaintainable, C is the fallback —
not A, because A is what B is trying to escape.

**Rejected — fetching directly into the index.** It is less code and it is wrong: it would make
`index.db` hold something that is not a pure function of (archive, importer version), which is
the one invariant this project has been most careful about.

**Accepted risk.** B may break without warning; it has no compatibility promise. The mitigation
is already specified — `chat-search-a7k.10`'s staleness nag treats "never fetched" and "fetched
today" as different states, so a silently dead fetcher surfaces as a stale source rather than as
an empty one. Claude.ai degrades to A when B breaks, which is a worse day, not a lost archive.

**Revisit when.** An official conversation-history API appears for any of the three surfaces —
that collapses B and D into a supported route and this ADR should be rewritten, not amended. Or
B breaks twice in one quarter, at which point C stops being theoretical.

---

## 22. A search the log cannot vouch for is excluded by hand, not by a rule

`accepted` · 2026-08-04

**Context.** `queries.jsonl` is the only record of real information needs this project has, and
`chat-search-6eb.21` plans to harvest an eval set straight out of it: each folded query becomes
a `[[query]]`, each picked conversation a grade-3. Measured on 2026-08-04, the log held 2,720
events and 136 distinct query strings — and almost none of that was a need.

Three ways the log lies about itself, all of which would have survived into the generated set:

| what is in the log | what it looks like to a harvester | what it actually is |
| --- | --- | --- |
| `l`, `la`, `lau` … `launchd`, ~100 ms apart | seven distinct queries | one query, typed |
| `borrow checker` ×96, no pick | a query the ranking failed | a latency benchmark |
| a pick with `q = ""` | a grade-3 | the recent list, browsed |

The first and third are decidable from the events. The second is not, and that is the whole
difficulty: a query typed to measure the ranking is ordinary text and goes unpicked, which
is exactly what an abandoned search — the signal 6eb.21 most wants to keep — also looks like.

**Decision.** Two rules over the events, and one thing authored.

_Over the events._ A search collapses into whatever followed it when the follower repeated or
extended its text under the same filter within `UNREAD_MS`, because nobody read the first one.
A pick never collapses, so the judgement it carries cannot be folded away. A pick with no query
is set aside as browsing rather than becoming a need.

_Authored._ `Event::Driven { from, until, why }` declares a half-open span of the log as
machine-driven. `cs needs --driven FROM..UNTIL --why TEXT` appends one. It is a line in the log
rather than a constant in the harvester, which is the same shape ADR 3 gives every other piece
of authored data: appended, never rewritten, deletable if it was wrong, and it travels with the
log when the log is synced.

**Why not detect it instead.** Every automatic rule proposed here separated benchmarks from real
searches by proxy — volume, repetition rate, absence of a pick — and every one of them also
catches a real search somebody gave up on. There is no evidence in a search event that says
which it was, because the only difference is the intent of the person typing. Asking them once
costs a line; guessing costs the abandonment signal, silently, forever.

**`UNREAD_MS` = 2 s, measured.** Of the 2,072 prefix-adjacent pairs in the log on 2026-08-04,
the slowest inside a typed ladder was 1,328 ms apart and the next one up was 10.9 s. The
boundary therefore sits in the middle of a gap eight times wider than the constant, and moving
it anywhere inside that range changes nothing. What it *means* is that two seconds is not long
enough to read a result list and decide it was wrong.

**`CS_LOG_QUERIES=0` is a convenience, not the mechanism.** It keeps a driven run out of the log
in the first place, and wraps a whole measurement session including subprocesses — which
`--config` pointing at a scratch file does not. It is not the answer on its own because it is
retroactively useless, and because forgetting it has to stay recoverable. That is what the span
is for.

**What it measured, 2026-08-04.** 2,720 events → 93 needs on the fold alone; six declared spans
covering 2,641 events took that to **35 needs, 19 judgements across 17 answered queries**. No
judgement was excluded by any span. The raw log had read as 37 picks across 124 distinct
queries, which is most of the way to 6eb.21's "50-100 picks across 20+ distinct queries"
trigger; the honest figure is about a third of the way, and the trigger has not been met.

**Revisit when.** A client appears that logs genuinely per keystroke — the TUI does not, it
writes one event per session — since then the ladders arrive inside one process and a session
id would be cheaper and more exact than a timing rule.

---

## 23. The reply to a search is a core-owned type, and clients adapt rather than assemble

`accepted` · 2026-08-04

**Context.** ADR 12 put the client seam at a JSON protocol and ADR 14 kept it a spawned CLI
rather than a daemon. Neither said who *builds* the reply, and the answer turned out to be
everybody. `cs/src/commands.rs` assembled the envelope inline at four sites; the flat and
grouped key lists agreed only because `json_contract.rs` asserted it; `(ms*100).round()/100`
appeared three times, one of them feeding the query log, so a logged need and a printed answer
could round differently. Meanwhile `cs-tui` reached `search_grouped_counted` for a `Total` that
no JSON client could ask for, so `cs search --json` could not report how many conversations
matched — only how many it returned.

Five public entry points (`search`, `search_grouped`, `search_grouped_counted`,
`count_matching`, `recent`) exposed routing that was never a caller's decision. The reply had
no author, so every client became one.

**Decision.** `cs_core::answer` owns the reply. One entry point returns an `Answer`; a private
receipt of the ask lets `settle` establish the total the first pass declined to pay for.
Serializing it *is* the wire, the way `blocks::Transcript` already works for `cs show --json`.
Clients adapt: the CLI serializes, the TUI renders, a Swift app decodes.

Three wire rules follow, and all three are things a client can no longer get wrong:

- **One shape per question.** `results` is always conversations; the polymorphic `results` and
  its sometimes-present `grouped` flag are gone, and `--flat` has its own small envelope. A
  decoder never branches to learn what type a key holds.
- **Identity is stated once.** A match carries message fields only. Repeating `conv_id`,
  `title`, `source` and the whole `destinations` array inside every nested hit was bytes and a
  second place for two copies to disagree.
- **`v`, and a policy for it.** Additions are silent; `v` bumps only when a field changes
  meaning.

**Two promises made early because they are free now and breaking later.** `score` is opaque
ordering — clients never re-sort and never parse it — and `snippet_spans` may be empty. Both
exist for ranking this project has already said it intends to grow into: the Indexer's
definition in GLOSSARY.md says "and later vectors", and a semantic match need contain no
lexical term to highlight. Documented today they cost one table row each; documented after a
client ships they cost a `v` bump.

**Why not a sectioned reply.** The alternative — sections requested à la carte, keyed by
`conv_id`, with a total policy per ask — is genuinely better for three futures: retiring
`kind_runs` by economics rather than by schema, adding facets, and settling a count over the
wire without re-running the search. It was rejected because the cost lands daily and the
benefit only if those futures arrive: every consumer would unwrap `Option`s it knows are
`Some`, a Swift client would join matches to conversations by key instead of reading nesting,
and "asked implies present" is a runtime law no type system checks. It also designs for the
stdio transport ADR 14 deferred and `chat-search-me9.22` measured as unnecessary — one adapter
is a hypothetical seam.

**Revisit when.** A second thing in the reply becomes expensive and optional the way
`kind_runs` already is. One knob is a knob; two are a sections model arriving one field at a
time, and at that point the rejected design is the right one.

---

## 24. The model is a property of the message; the conversation label is a summary of it

`accepted` · 2026-08-04

**Context.** `conversation.model` was a single column, there was no per-message model anywhere,
and each of the five importers that could read one collapsed it privately — in two different
directions. `claude_code` and `chatgpt_export` kept the **first** model they saw; `claude_desktop`,
`codex` and `gemini` kept the **last**. None of them said so where the other could see it, none
had a test that fed two models, and `chatgpt_export` additionally let `default_model_slug`
outrank every message in the file.

The defect was found while asking whether a model switch was worth drawing in the interface
prototype. The measurement taken then — "0 of 3,057 conversations use more than one model" —
could not have said anything: it joined a single conversation column onto each message and
counted distinct, which is 1 by construction (`poc/ui/NOTES.md` §3.23). The index could not
answer the question at all.

**What the archive actually says.** Measured 2026-08-04 by reading the raw archive rather than
the index, because the index was the thing that could not answer:

| source | conversations | naming >1 model | first ≠ last |
| --- | --- | --- | --- |
| chatgpt-export | 2,011 | 110 | 56 |
| codex | 684 | 33 | 31 |
| claude-code | 428 | 23 | 22 |
| gemini-cli | 11 | 0 | 0 |

**166 of the 3,134**, about 5%, and the pairs are not cosmetic: `claude-opus-4-8` → `claude-haiku-4-5`,
`gpt-5.4` → `gpt-5.4-mini`, `o3-mini-high` → `gpt-4o`. A downgrade part-way through an agentic
session is an epistemic boundary in the transcript, and the single label denied it existed.

Two further things the same scan turned up. `default_model_slug` names something no message ever
names in 154 conversations, **134 of them the literal `auto`** — the router setting that picks a
model, recorded as though it were the model. And `<synthetic>`, which `claude_code` rejected as
"not a model that ever ran", reaches `claude_desktop` too through the same claude-shaped audit
log, where nothing rejected it.

**Decision.** Three parts.

(a) **`message.model`** holds the model that produced that message, as the source declares it,
`NULL` on messages no model produced — user turns and tool results. Never inferred: a user turn
is not backfilled with whatever was running, because that is a reading of the transcript rather
than something it states. Codex is the one source whose declaration is not on the message —
`turn_context` heads a turn and names what will run for it — so it is carried forward onto the
assistant messages of the turns it governs.

(b) **No importer collapses.** `Conversation::model` is renamed `declared_model` and holds only
what the *conversation* says about itself: ChatGPT's `default_model_slug`, the Claude Desktop
state file, a codex rollout whose `turn_context` records precede every message.

(c) **The label is resolved once**, in the rollup at the bottom of `index::write_conversations_with`,
as the model of the last message that named one, ordered by `ts`. `declared_model` is the
`COALESCE` fallback beneath it.

**Why last, and why in the rollup.** Last because it is what the conversation *ended on*, so the
label predicts what resuming it would run — the reason `codex.rs` already gave for its own rule,
now the only rule. In the rollup because the alternative was five importers agreeing by
convention, and this codebase has already paid for that once: the local-date bug came from three
clients each deriving the day. Ordering by `ts` rather than by insertion is also strictly more
correct than any importer could be. A conversation is assembled from several files (ADR 7) and
file order is path order, so for the 6 claude-code sessions whose files disagree, "first file
wins" froze an arbitrary one; and ChatGPT's mapping is walked in id order, so "first seen" there
was never even "first chronologically".

**What it changed, measured over the full corpus.** 4,377 conversations in both the old and new
index: **4,182 labels unchanged, 195 changed.** 134 stopped saying `auto` (128 now say `gpt-4o`,
5 now say nothing, which is the honest answer when no message named a model); 45 stopped saying
the router name and now say the variant the messages name, `gpt-5` → `gpt-5-thinking` and
similar; 16 claude-code and codex conversations flipped end for end. Zero conversations are
labelled `auto`. The index now answers the question it could not: **171 conversations name more
than one model**, queryable as `count(distinct model) > 1` over `message`.

**Cost.** One `TEXT` column over 196,450 messages, ~1% of a 345 MB index, and an `IMPORTER_VERSION`
bump to 5 — the column is a pure function of (archive, importer version), so ADR 1 makes
`rm index.db && cs index` the whole migration.

**Not done.** Nothing reads `message.model` yet: it is not in the `cs search --json` envelope and
the preview does not mark a switch. That is deliberate — this ADR is about the index being able
to answer, and what to draw is a design question for whoever draws it (`chat-search-n58.26` is
the adjacent case, compaction boundaries, which are also structurally invisible).

**Revisit when.** A source appears whose model varies *within* a message, or one that reports a
model for a user turn in a way that is a statement rather than an inference. Also when the first
consumer lands, since drawing a switch may want the run boundaries precomputed rather than
derived per query.

---

## 25. A theme the project ships is fenced; a theme a person loads is measured and drawn anyway

`accepted` · 2026-08-05

**Context.** The token seam (`chat-search-me9.8.8`) carries two measurements that a theme has to
hold: the kind ramp at 2.2 / 4.0 / 7.2 / 13.0 against the ribbon track with even ~1.8× steps, and
the 4.5:1 AA floor on every text tier, taken on the harder of the two grounds it lands on. Neither
was assumed — `chat-search-4ar.11` raised `--ink-3` from 2.90 to 4.60 for the second, and
`poc/ui/NOTES.md` §2 argues the first from the ~2px the ribbon draws a kind at, where hue is the
channel that degrades first.

Two follow-ups then arrived at the same question from opposite ends: several directions in one
binary (`chat-search-me9.8.9`) and a token set loaded off disk (`chat-search-me9.8.10`). Both need
to know whether a theme is allowed to miss. Tyler wants Solarized and Gruvbox, and the published
values of both miss — on their own dark tracks:

| | tool | reasoning | user | agent | steps |
| --- | --- | --- | --- | --- | --- |
| solarized on `base03` | 2.79 | 3.43 | 4.75 | 5.61 | 1.23× 1.39× 1.18× |
| gruvbox on `bg0_h` | 2.52 | 5.98 | 7.79 | 11.95 | 2.37× 1.30× 1.53× |

Their quiet tiers miss too, and Solarized's is the designated one: `base01`, which Solarized itself
labels comments and secondary content, reads 2.79:1 on `base03` and 2.42:1 on `base02` — under half
the floor. Low contrast is what these palettes *are*.

**None of that is a porting mistake, and the search says so.** Every assignment of each palette's
own published colours to (track, four kinds) was measured — 16 colours for Solarized, 19 for
Gruvbox, both themes, every colour allowed to be the track:

| | the best a published palette can do | steps | what it means |
| --- | --- | --- | --- |
| solarized | `base01 blue base1 base2` on `base03` — 2.79 4.08 5.61 12.25 | 1.46× 1.38× 2.18× | no assignment is even. Nothing sits between `base1` at 5.61 and `base2` at 12.25, and that gap is the palette |
| gruvbox dark | `bg3 gray green fg0` on `bg0_h` — 2.52 4.47 7.94 14.45 | 1.77× 1.78× 1.82× | evenly spaced, the whole ramp ~1.11× above the targets |
| gruvbox light | `bg4 green red fg0` on `bg0` — 2.45 4.29 7.60 12.99 | 1.75× 1.77× 1.71× | one step 0.09 out |

**And the re-solve costs what the palette's own range costs.** Feeding each palette's hues through
`poc/ui/palette.py`'s `solve` at the fenced targets, on the dark side:

| | tool | reasoning | user | agent |
| --- | --- | --- | --- | --- |
| solarized | `#586e75` → `#4b5e63` | `#6c71c4` → `#797ec9` | `#2aa198` → `#34c8bc` | `#93a1a1` → `#edefef` |
| | L 40% → 34% | 60% → 63% | 40% → 49% | **60% → 93%** |
| gruvbox | `#665c54` → `#5d544c` | `#d3869b` → `#c45c78` | `#8ec07c` → `#83ba70` | `#ebdbb2` → `#f0e5c7` |
| | L 36% → 33% | 68% → 56% | 62% → 58% | 81% → 86% |

Gruvbox survives a nudge — every band lands within a few percent of where its author put it.
Solarized's brightest kind has to leave the grey ramp it belongs to entirely, past `base2`. That is
the difference between a direction that can carry a palette's name with *derived* after it and one
that would be wearing the name.

Worth saying once: fidelity was never on the table anyway. A theme here is 30 colour tokens per
side. Solarized publishes 16 colours and Gruvbox 19, so a port invents at least half the set —
panels, rules, selection grounds, match grounds, five source hues — before it reaches any fence.
The question was never whether to invent, only which parts and whether to say so.

**Decision.** Two classes of theme, and the class is **provenance, not content**.

A **direction** is compiled into the binary. It is fenced: `--verify-theme` measures every
direction present and a direction that misses does not ship. Everything the picker offers is one.

A **user theme** is read at launch off a file the person wrote. It is measured by exactly the same
code, and then drawn whatever the readings say.

**What the app does about a user theme that misses**, so that `chat-search-me9.8.10` decides none
of it:

1. **It is always measured**, by `ThemeCheck` and not by a second copy of these rules. One rule,
   one place — the local-date bug is what happens otherwise.
2. **It is drawn as authored, entire.** Never partially merged with a direction to patch the
   failing tokens: half a palette from each is a palette nobody designed and nobody can debug.
3. **The misses are said out loud** — `Report.failures` on stderr at launch, and the class beside
   the name wherever the app names a theme. No modal and no banner: the only person who can be
   nagged here is the one who wrote the file, and they already know.
4. **Unreadable is not unfenced.** A file that is malformed, or missing a token, is not a theme —
   fall back to the shipped direction and say why. (`Palette`'s precondition treats an incomplete
   set as a programmer error, which is right for a generated file and fatal for a typed one, so
   the loader validates before it constructs.)

**Sub-decision — the file is watched, and a bad read holds rather than falls back.**
`chat-search-me9.8.39` made the file reload on save, which is what turns rule 4 from one rule into
two cases. Rule 4 was written when the only read was at launch, and *at launch there is nothing
else to draw*: the file is the first thing the app knows about a token set, so a file that is not a
theme leaves the shipped direction as the only answer. Mid-session there is a third thing — the
theme already on screen, which loaded whole and was measured — and **that is what stays**:

> The last theme that loaded whole stays on screen until another one does. Unreadable,
> half-written, or deleted, what is drawn does not change, and the reasons are said.

Three reasons, in the order they decided it. **A save is not atomic in most editors**, so a watch
reads files that are empty or half a palette — truncate-then-write leaves one for about a
millisecond — and falling back would flick the whole window to a palette nobody is working on and
back again on every save, which is worse than the relaunch it replaced. **The two cases cannot be
told apart** at the moment they are read: a file caught mid-write and a file with a typo in it are
both a file that is not a theme, so a rule that treats them differently is a rule about a
distinction that does not exist at the read. And **a timing guarantee is not available** — events
are coalesced after a quiet period, but no interval is long enough for every editor and short
enough to feel live, so a rule that leaned on the timer would be a rule that fails on somebody
else's editor rather than one that holds.

What it costs is that deletion is no longer read live. "The file is the memory — it is there or it
is not" still holds across launches, and mid-session the way back to a direction is
`--no-theme-file` and a relaunch. That is the same act as looking at a direction beside what you
are dialling, so it is a route that already existed rather than one this created.

The announcement follows the same shape: the full launch line is said once per file, and a reload
says only what it did not say last time — a save that clears the fence says nothing, one that
breaks something says what broke, and one that breaks it again says nothing. Nagging is what rule 3
already refuses on the same grounds: the only person who can be nagged here is the one who wrote
the file.

**Why the class cannot be a field on the theme.** The alternative was letting a theme declare
itself unfenced. That fails the first time it is used: the themes that fail the fence are exactly
the themes that would declare the exemption, and a fence that only measures what already passes is
decoration. Provenance cannot be asserted by the thing being measured, which is the whole reason to
use it.

**Why relaxing the fence was rejected, having been measured.** The obvious middle — keep "even
steps and a visible foot", drop the absolute 2.2 / 4.0 / 7.2 / 13.0 — buys Gruvbox's dark side and
nothing else. Its light side is still 0.09 out and Solarized is 0.42 out, so no *complete* palette
is rescued by it, and `NOTES.md` §2 holds the ratios constant across directions on purpose: it is
what makes directions that look nothing alike read identically at 2px. A relaxation that
admits no new theme is a weaker check bought for nothing.

**Why a person is allowed to break AA on their own screen.** The fence is a promise about what this
project ships, not a restraint on what somebody may look at. Refusing would also be worse for the
loop `chat-search-me9.8.10` exists to shorten: a refusal means the edit silently does nothing, and
dialling a palette in means passing through dozens of failing intermediate states on the way to a
good one. What it costs is bounded, because nothing in this client is encoded in colour alone
(docs/TUI-DESIGN.md §7) — the reader draws a band as a 3pt spine *and* a sigil *and* a change of
face. The ribbon is the exception and the honest cost: when `kind_runs` lands it is 2px of colour
with no second channel available, so an unfenced theme makes that strip unreadable, and the reader
still says everything the strip was summarising.

**Where Solarized and Gruvbox land.** Three routes, none of which pretends to be another:

- **as a user theme, exactly as published** — which is usually what "I want Solarized" means;
- **as `solarized-derived` / `gruvbox-derived`**, hues through `palette.py`'s solve, shipped as
  directions, with the table above as the record of how far each one had to move;
- **as itself, if a palette is found that holds as authored.** Nothing here forbids that. Neither
  of these two is it.

**Revisit when.** A theme can be authored *inside* the app rather than in a file — a picker that
edits is the app authoring a theme, and the argument above turns on the app not being the author.
Also when the ribbon lands, because that is the first surface where the ramp is load-bearing on its
own, and the answer may want to be "the ribbon draws in the shipped direction's ramp when the
loaded one is unfenced" rather than "the ribbon is unreadable".

---

## 26. `kind_runs` travels at full resolution; the strip is downsampled by whoever draws it

`accepted` · 2026-08-06

**Context.** `kind_runs` (`chat-search-me9.19`) was designed on a prediction that did not survive
contact with the corpus. Tool traffic is 66–85% of it and arrives in long stretches, so the runs
were expected to compress hard. They do not: agent prose alternates with nearly every tool call,
so the encoding is `agent,1` / `tool,N` repeated a few hundred times, and drawn messages divided
by runs is 1.75–2.7x. Re-measured on the live 3,059-conversation index, 2026-08-06, `--nested 3`:

| query | rows | runs/row | `kind_runs`, compact | on the wire |
| --- | --- | --- | --- | --- |
| `borrow checker` `--limit 60` | 58 | median 5, max 688 | 35.9 KB of 110.0 (33%) | 85.9 KB of 273.1 (31%) |
| `commits` `--limit 100` | 100 | median 101, max 688 | 169.9 KB of 390.0 (44%) | 403.1 KB of 1086.8 (37%) |
| `the` `--limit 100` | 100 | median 159, max 688 | 226.3 KB of 486.0 (47%) | 538.2 KB of 1445.0 (37%) |
| _(no query)_ `--limit 100` | 100 | median 5, max 196 | 26.6 KB of 83.1 (32%) | 63.6 KB of 200.7 (32%) |

`chat-search-me9.31` filed that and then declined to answer it, on the grounds that `kind_runs`
counts what `blocks::drawn` draws, so settling the payload before the fold model would settle it
twice. The fold model settled on 2026-08-06 (`chat-search-me9.41`), and it settled in the
direction that decides this rather than merely permitting a decision: `Folds` is keyed on band,
and **which bands a reader is showing right now is client session state, not a property of the
conversation**. `drawn` itself has not changed since the day it was written.

**Decision.** The field stays at full resolution — neither downsampled to the strip's width nor
capped per conversation. Recorded here rather than left as an absence, because "nobody changed it"
and "this was decided" are the same diff and different facts.

**Why the server *cannot* do the downsample, rather than need not.** It would have to know three
things, and it knows none of them.

1. **The width.** `poc/ui` draws this data at three scales from one renderer — row strip, sitting
   card, big minimap — under its own note that this is "the TUI spec's rule that the density strip
   and outline mode are the same data at two resolutions, extended rather than duplicated". The
   ~200px row strip is the smallest of the three. A server that quantises to it starves the other
   two, and the reader's minimap is the one that most needs the resolution.
2. **Which bands are drawn.** `chat-search-me9.8.36` ports four per-band knobs, and tools hidden
   is the setting that makes a 900-message agent session legible. Hiding tools removes 66–85% of
   the axis, so the strip a client then draws is over the *non-tool* messages — which it can
   derive from full runs and cannot derive from bucketed ones.
3. **That bucketing is a rendering.** `cs_core::blocks` opens by saying it holds the rules and
   never the rendering, and names which messages are drawn as a rule. How many pixels they are
   drawn in is not one. A downsample on the wire is that line crossed in the one direction the
   module was written to prevent.

**Measured, so the loss is not a guess.** Rebuilding the tools-hidden axis from a 200-column
downsample, against rebuilding it from the runs as sent, under both bucket rules anyone has
proposed — dominant band (`chat-search-me9.8.19`'s design) and prose-beats-tool priority (what
`poc/ui`'s `mapBands` actually implements):

| query | rule | median error | p10 / p90 | rows wrong by >1.5x |
| --- | --- | --- | --- | --- |
| `borrow checker` 60 | dominant | 1.00x | 0.87 / 1.00 | 4 of 58, too short |
| | priority | 1.00x | 1.00 / 2.37 | 10 of 58, too long |
| `the` 100 | dominant | 1.00x | 0.49 / 1.03 | 21 of 100, too short |
| | priority | **1.85x** | 1.00 / 2.75 | **66 of 100**, too long |

The median row needs no downsample at all — 53 of those 58 rows and 65 of those 100 are already
under 200 runs — so the change would do nothing for two thirds of a page and silently corrupt the
rest. Nothing in the payload would let a client tell which third it was holding.

**What the bytes actually are.** `kind_runs` is a third of the compact response and 31–37% of what
goes on the wire, and the gap between those two numbers is the point: `--json` is still
pretty-printed, at 2.4–3.0x on these queries. On the `--limit 60` response, indentation is 163 KB
of 273 KB where the whole of `kind_runs` is 86 KB. Downsampling it to 200 columns saves 18–23% of
the response; emitting compact bytes saves 60%, needs no interface change, and is already filed
(`chat-search-me9.29`). A bytes argument that skips the larger, cheaper, already-agreed win to make
a lossy change to a published field is not a bytes argument.

**Why not cap the runs and say so.** A cap is the downsample with an honesty flag bolted on, and
the flag does not rescue it: a client told "this shape is truncated" still cannot draw the strip,
the minimap or the tools-hidden axis, so it has bytes it must not use. It also adds a second thing
on the wire that is sometimes not what it says it is, beside `kind_runs` already being empty when
unasked-for.

**What this costs, stated plainly.** 226 KB and ~4 ms of client-side parse on the broadest query at
`--limit 100`; 36 KB and ~0.7 ms on a realistic one at 60. Server-side it is the read, not the
bytes — `chat-search-me9.26` measured filling the field at +2.0 to +11.1 ms in-process, which is
why it stays behind `SearchOptions::shape` and why the TUI does not pay it. Downsampling would not
have avoided that read; it only shrinks what is serialised afterwards.

**Revisit when.** Any of the three unknowns above stops being unknown. A transport that keeps the
connection open (`chat-search-me9.21`) would let a client state its width and its visible bands in
the ask, and at that point the server can fold — because it would be told, not guessing. Also when
ADR 23's trigger fires: a second expensive optional field makes the sectioned reply the right
design, and "retiring `kind_runs` by economics rather than by schema" is one of the futures that
design was rejected against.

---

## 27. The macOS 26 design system is inherited; the titlebar is taken back

`accepted` · 2026-08-06

**Context.** `swift --version` reports `Target: arm64-apple-macosx26.0`, and `Package.swift` said
that declaring `platforms: [.macOS(.v15)]` "keeps a hand-fenced palette out of Liquid Glass". That
was wrong, and it was wrong in the direction that hides itself: adoption keys off the SDK a binary
is **linked against**, never the platform it is built for. What the shipped binary records:

```
$ vtool -show-build apps/macos/.build/release/chat-search
    platform MACOS   minos 15.0   sdk 26.2
```

`chat-search-me9.8.28` settled it by photograph rather than by argument, because the app's own
camera cannot see its own chrome — `Measure.capture` is `cacheDisplay` on `window.contentView`, so
there is no titlebar in the rectangle and no window server in the path. Build once, rewrite **only**
the SDK field with `vtool -set-build-version macos 15.0 15.0 -replace`, re-sign, photograph both
with `screencapture -l` against the real window server. Rewriting the field back to its own value
produced a byte-identical capture, so the rewrite changes nothing and the value changes this:

| the real app, 1200×800 content | as built, `sdk 26.2` | same bytes, `sdk 15.0` |
| --- | --- | --- |
| window height | 832 pt | 828 pt |
| titlebar band | rgb(31,33,37) | rgb(39,42,46) |
| corner radius | ~21 pt | ~15 pt |
| title | leading | centred |
| separator under the titlebar | none | hairline |

A probe with a toolbar shows the reach the app's plain window hides: titlebar+toolbar 66 pt against
52, capsule buttons against rounded rects, `List` transparent against backed, no focus ring against
one.

**What is not happening, which decides most of this.** No glass *material* is on screen. The
titlebar is opaque at alpha 255 and samples nothing — the probe put a striped window directly
behind it and no stripe came through. The app has no toolbar content, no sidebar, no popover and no
sheet, which is where macOS 26 actually puts glass. What is inherited is metrics, shapes and chrome
colours. **The complaint that opened the bead is a colour disagreement, not a translucency
problem**: rgb(31,33,37) chosen by the system beside `bg` at rgb(10,65,78) chosen by `CsTheme`.

**Decision.** Neither opt out nor opt in. Stay on the macOS 26 SDK with no compatibility key and no
change to the macOS 15 floor, and take back the one surface that was wrong: the window gets
`.fullSizeContentView` with `titlebarAppearsTransparent`, so the content draws to the top edge in
its own tokens. Implemented by `chat-search-me9.8.30`, which wanted the content to own the titlebar
anyway; recorded here because "nobody opted out" and "opting out was rejected" are the same diff
and different facts.

Measured, on the same SDK, same probe, one flag apart:

| top band | |
| --- | --- |
| plain window | rgb(30,34,38) — the system's grey, matching the real app's rgb(31,33,37) |
| `.fullSizeContentView` | **rgb(10,65,79) — `CsTheme`'s own `bg`**, alpha 255, nothing composited over it |

**What implementing it added, 2026-08-07.** Two flags are the decision and three lines are the
change: `chat-search-me9.8.30` found that `.fullSizeContentView` alone buys the *background* and
nothing else. `NSHostingView` hands SwiftUI a top safe area exactly as tall as the titlebar it
replaced and lays every view out below it, so what the probe photographed as a win is, in a real
SwiftUI window, `bg` painted into an empty strip of the same 32 points. `safeAreaRegions = []` is
what actually moves content into the band. The probe could not have caught this — it was an
`NSHostingView` too, but its content was a `ZStack` over a `Color`, which fills whatever it is
given including the inset. Everything else in the table above held on the real app: the band is the
theme's ground in all six directions and on both sides of the appearance axis, band-against-page
off `screencapture -l`.

**Why not opt out.** `UIDesignRequiresCompatibility` is cheaper than it looked — the bead assumed a
bundle-less SwiftPM executable had nowhere to put it, and that is false. It works two ways, both
measured on a copy of this package: linked into `__TEXT,__info_plist` with `-sectcreate` from
`linkerSettings`, and as a plain `Info.plist` file beside the binary, which `Bundle.main` reads for
an unbundled executable. `poc/swift` still builds either way, because the flags sit on the
`ChatSearch` executable and the path dependency reaches `CsKit`.

It is rejected on aim, not on cost. **It moves the band from rgb(31,33,37) to rgb(39,42,46)** — a
different grey chosen by the same authority — so it does not fix the thing that was wrong. It buys
"stop the design system moving under me", which nobody asked for, at the price of `unsafeFlags` and
a key Apple describes as a transition affordance that a later SDK stops honouring. (That expiry is
reported, not measured; what is measured is that it works on 26.5.2 today.) Paying an expiring key
to avoid a material that is not being drawn is paying for a picture that does not exist.

**Why not opt in.** `NSGlassEffectView` is `API_AVAILABLE(macos(26.0))`, so adopting it costs the
floor `chat-search-me9.8.27` deliberately bought — there is no 16 through 25, so 15 is one release
of headroom and 26 is none. It also costs the fence: ADR 25 rests on a contrast ratio between two
tokens, and a glass surface's effective background is whatever window is behind this one, which is
not a token and cannot be made into one. Either the fence stops applying to the chrome or it grows
a notion of "a backdrop I cannot see", which is a fence that passes everything. And `--shot` cannot
photograph a window-server blur at all, so every glass surface adopted is a surface no pull request
screenshot shows. Mostly, though: there is nothing on screen for it to land on. It is a decision
waiting for a surface.

**What this costs, stated plainly.**

1. **It is not a rejection of the design system.** Control shapes, metrics and chrome colours stay
   the system's and will keep moving at every SDK bump. Only the titlebar comes back.
2. **The window's shape stays the system's** — corner radius measured ~21 pt with the flags and
   without them.
3. **The traffic lights stay system-drawn and now sit on a themed ground.** That is a contrast pair
   `ThemeCheck` knows nothing about, and a light direction is where it would first go wrong. Filed
   rather than waved at.
4. **The app owns more chrome.** The title, its inset, and the ~78 pt the lights occupy become a
   per-direction problem.

**Revisit when.** The app grows a toolbar, sidebar, popover or sheet — that is the first time an
opt-in has a subject, and it should be decided against a photograph of the real surface rather than
in the abstract. Also when an SDK bump moves something the fence cannot absorb, which is the case
this decision deliberately leaves un-defended.

Answering either needs a window-server capture, and the repository does not have one: `--shot` is
`cacheDisplay`, so **no pull request screenshot in this project has ever shown window chrome, and
none can**. The instruments this decision was measured with are in `.shots/me9.8.28-probe/`,
untracked, next to the captures — a working, focus-stealing-free `screencapture -l` run against the
real app among them. Folding that into `scripts/shot.sh` is filed separately; until it lands, the
probes are a directory somebody has to be told about, which is the weakest part of this entry.
