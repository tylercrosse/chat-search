# Decision log

Thin ADRs. Each entry records _why_, so that when circumstances change you can tell whether the decision still applies — that is the part worth keeping, not the choice itself.

**Status** is `accepted` (agreed, build on it), `proposed` (my recommendation, not yet agreed), or `open` (genuinely undecided).

Terms follow [GLOSSARY.md](./GLOSSARY.md). Measurements come from [../poc/RESULTS.md](../poc/RESULTS.md), taken 2026-07-28 on an Apple M3 against a frozen 973-file / 1.8 GB corpus.

---

## 1. Raw archive is the source of truth; the index is disposable

`accepted` · 2026-07-28

**Context.** ~3.2 GB of transcripts across five tools, in formats that change without notice. Every schema, tokenizer, chunk-size and ranking choice downstream is a guess.

**Decision.** Capture raw transcript bytes append-only and never rewrite them. Treat `index.db` as a pure function of _(raw archive, importer version)_, safe to delete and rebuild at any time.

**Why.** It converts most downstream decisions from irreversible to reversible — you `rm index.db` instead of writing a migration. A full rebuild is 7s, so this costs nothing in practice. The temptation to skip it is that raw is mostly noise (91% tool traffic); disk is cheaper than the decisions it buys back.

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

**Why.** If annotations live in the index, "rebuild the index" means "destroy user data," so you stop rebuilding — which breaks decision 1. The event-log shape also makes multi-machine merge concatenate-and-fold, with no conflict resolution. Costs ~30 lines now; genuinely unpleasant to retrofit once annotations exist.

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

**Decision.** One SQLite file, FTS5 contentless indexes, BM25 with a time-decay multiplier. `sqlite-vec` in the same file when embeddings arrive. Every UI surface is a thin reader.

**Why.** BM25 is built in, there is no server, and Rust/TS/Swift/Python can all read it — which is what makes "open it anywhere" tractable. Measured: query is 1–3 ms regardless of runtime, so the storage layer is not the bottleneck for any surface.

**Revisit when.** Corpus exceeds ~50 GB, or hybrid ranking needs something FTS5 can't do.

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

**Open sub-question.** If a conversation is renamed both in-app and upstream, authored currently wins. Last-write-wins by timestamp is the alternative.

---

## 9. Deletion keeps data; forgetting is explicit

`proposed` · 2026-07-28

**Decision.** Three cases. Upstream-deleted and retention-pruned both _keep_ the content and set `deleted_upstream_at` (flagging that `resume_cmd` will fail). A deliberate `forget` removes from raw and index and writes a tombstone so the next scan cannot resurrect it from a source file that still exists.

**Why.** Outliving upstream deletion is the point of an archive — Claude Code's 30-day retention already destroyed everything before 2026-06-19. But a searchable index of everything is also a concentration of risk, so deliberate removal has to be possible and has to stick.

**Revisit when.** Deciding redaction policy, which is the adjacent unsolved problem.

---

## 10. No incremental indexing yet

`proposed` · 2026-07-28

**Decision.** Build change _detection_ — `(path, size, mtime, hash of first 64 KB, importer_version)` — but respond to change with a full rebuild. Tail-append only files touched in the last few minutes, for the live session.

**Why.** Rebuild is 7s and ingestion is I/O-bound, so incremental indexing is pure complexity right now; patching an FTS index through deletes is where the bugs live. The tail path then only ever handles pure append, the easy case.

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

**Decision.** Define one JSON request/response contract. Ship it over argv today; the same contract can run over stdin/stdout or a unix socket later without changing client code.

**Why.** This is the un-boxing move — it lets the daemon question stay open and makes a core rewrite contained. Measured: the query is 1–3 ms in every runtime and the entire spread between runtimes is process startup, so the seam choice matters more than the language.

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

**Decisive evidence against A.** Within a _single conversation_ the files disagree about their surface: in session `019f760a` the main thread carries `source: "vscode"` while its guardian subagent carries `source: {"subagent": {...}}`. Surface is a **thread** attribute, not a conversation attribute, so it cannot serve as conversation identity — A is not merely awkward, it is incoherent.

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
rather than _space_ — those are different problems, and cloning explicitly solves neither the
second nor the first.

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
