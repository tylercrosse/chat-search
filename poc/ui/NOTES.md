# Design log — interface prototype

Work in progress, kept because the *decisions and measurements* are the durable asset and
the code is not. The prototype is throwaway; this file should survive it.

Companion to [`README.md`](./README.md), which says what the thing currently does. This
says why, what was measured to get there, and what was got wrong on the way.

Status: **still iterating.** Nothing here is a product decision. `docs/DECISIONS.md` and
`docs/TUI-DESIGN.md` remain authoritative for the shipped surfaces; where this disagrees
with them the divergence is filed as `chat-search-4ar.8`.

---

## 1. Measurements

The most reusable thing produced. All against the live index (2026-08-03, 3,059
conversations / 184,099 messages) unless marked as sample. Re-derivable with
`poc/ui/export.py` and the queries in §7.

### Corpus shape

| | |
| --- | --- |
| conversations / messages | 3,059 · 184,099 |
| by source | chatgpt 2,011 · codex 655 · claude-code 381 · claude.ai 188 · gemini-cli 12 |
| avg messages | chatgpt **7.2** · claude-code **132.0** · codex **182.1** |
| prose fraction | chatgpt **0.88** · claude-code 0.30 · codex 0.23 |
| user turns | chatgpt 2.8 · claude-code 8.0 · codex 6.5 |
| msgs per steer | claude-code 20.4 · codex 37.0 |
| largest conversation | 2,553 messages |

**Archetype is ~87% predicted by source.** Of conversations ≥10 messages: 564/613 codex
and 274/327 claude-code are under 35% prose; 265/411 chatgpt are over 60%. So a "shape"
encoding that only separates agentic from conversational duplicates the agent badge and
has to carry finer structure to earn space.

### Titles

`title_origin`: codex **99.4% first_user**, claude-code 11.0% first_user but **75.9%
generated**, chatgpt **99.9% custom**. So TUI-DESIGN §3's "opening prose is roughly the
title" holds for ~22% of the corpus, not generally — it is a Codex property.

### Where matches land

Message-level, by query: `borrow checker` 78% in tool traffic, `fts5` 66%, `timezone`
85%, `schema migration` 74%. Conversation-level, tool-only matches: 20–73% depending on
the query.

Consequence: §8's "tool_* that matched fts_tools → expanded" is **the common path, not an
exception**.

### Free structural signals

| signal | coverage |
| --- | --- |
| `ended_at` | 100% — the only signal covering the whole corpus |
| `cwd` | **1,036 / 3,059 (34%)** — codex and claude-code 100%, chatgpt and gemini-cli 0% |
| `git_branch` | 932 conversations, 93 branches — claude-code 381/381, codex 551/655 |
| file paths in `tool_call` args | **30,373 of 63,585 tool calls** |
| gaps > 4h inside a conversation | 1,088 (≈1 per agent conversation) |
| assistant prose ending in `?` | 4.6% of 30,666 (≈1.4 per agent conversation) |
| `is_error` on tool_result | 1.3% of 62,611 |
| `is_sidechain` | 4.2% of all messages |
| multi-file conversations | 38 (~4%) |
| conversations spanning >1 day | 127 (~9%) |

### Projects

After normalisation (worktrees folded into their parent, Codex scratch and temp dirs
dropped): **67 projects, 991 conversations.**

| | |
| --- | --- |
| share of the corpus | **34% of conversations, 92% of all messages** (169,589 / 184,099) |
| size distribution | 11 projects hold **719 (73%)**; 12 hold 10–20; **38 hold ≤3** |
| worktrees folded | chat-search 10 directories → 1 project; calorie_tracker 2; mars-attacks 3 |
| dropped as not-a-project | 30 Codex scratch dirs (43 convs), 2 `/private/var` temp dirs |
| concurrency | median **4** active projects per week, peak 16 |
| run boundary inside a project | **12h.** 3 days (the `lineages()` rule) left chat-search, meety-local and dev/career each as one undivided run; 12h gives 1–15 runs and tracks the day count (chat-search 6/6, personal-site 4/4, ga 7/7) |

**Projects cannot be recovered for the other 66%.** Of cwd-less conversations,
`mars-attacks` appears in 2, `calorie tracker` in 3, `chat-search` in **0**. Only `goal
drift` hit (79) — and that is a subject, not a repo. Repo names do not appear in chat
prose; subject names do. So projects are the natural axis for the agentic corpus and
topics for the conversational one; they are not competing taxonomies over one corpus.

**The corpus is far richer in structure than in usable subject signal.** Every
subject-based grouping attempt was mediocre; every structural signal was strong and free.
That is the opposite of the assumption the collections work started from.

### Topics

Broad seeds (the organiser's nine) were too wide to explore: AI/ML reached **899 of
3,059** (29%), Programming 872. After splitting into 27 narrower seeds: **median topic
5.3%**, largest 14%, max pairwise Jaccard **0.34**.

On the full corpus with the shipped taxonomy: **max Jaccard 0.27, nothing contained in
anything** — the topics genuinely divide the space. **37% match no topic at all.**

`Prose and editing` × `Technical writing and docs` = **0.13 Jaccard**. They read as
duplicates and barely share conversations, because one is a *mode* and one is a *subject*.

### Cross-validation

`~/dev/sandbox/chat-history/organizer` parsed the raw ChatGPT export independently:
**2,011 vs 2,009** conversations, mean **7.28 vs 7.2** messages. Two parsers agreeing is
real validation of `cs-import`. One discrepancy unresolved: it reports max 116
active-path messages where the index reports 101 for chatgpt — possibly a counting
difference (`active_message_nodes` vs `msg_count`), possibly an importer gap. **Worth
checking.**

---

## 2. Decisions, with rationale

### Row

- **Three lines with a query, two without.** Line 1 metadata + ribbon, line 2 title full
  width, line 3 the best match. The third is spent only when there is something to show.
- **Line 3 is the *true* best match, framed with provenance** (`⚙ Bash(cargo test) › "…"`).
  Not "prefer prose": if the ribbon's strongest tick is 80% along and line 3 shows prose
  from 12% along, the row contradicts itself. The frame is what stops a tool hit reading
  as log spew.
- **The ribbon is one graphic with two channels.** Body = shape, query-independent,
  run-length encoded over consecutive same-kind messages. Overlay = match ticks,
  query-dependent, vanish with no query. Notches = pauses > 4h. Dots = agent asked you.
  Red = tool failed. Sharing an axis is the point: *was my hit in a steer, or buried in a
  40-message run?* is the discriminating read, and 66–85% of hits are in tool traffic.
- **Hidden kinds are dimmed on the maps, not dropped from the axis.** Reversed mid-session
  — see §3.

### Preview

- **~602px (78ch)**, paid for by the title moving off the column grid. First time it has
  had a real reading measure.
- **Gutter spine carries role and kind; text contrast is identical for user and agent.**
  `me9.1.1` records "do not privilege user turns; an assistant answer is often the thing
  being looked for", and the earlier mock violated it by dimming assistant prose.
- **Per-kind fidelity is the model** (user / agent / reasoning / tools × hidden /
  collapsed / expanded). The three preset names are presets over those four knobs. Direct
  three-state controls, not a cycle — cycling forced you through `hidden` to get from
  expanded back to collapsed.
- **Segment is the coarse fold unit** — a steer plus the run it caused, summarised as
  `→ 34 calls · 2 failed · asked you 1×`. Per-message is 211 toggles on a 211-message
  conversation.
- **The default follows archetype.** Real ChatGPT threads rendered `→ 1 messages` after
  every turn: the segment fold is for agentic runs and actively harms conversational ones.
  Derived from prose fraction.

### Topics and collections

- **Seeded keyword taxonomy, not clustering.** k-means assigns every point, so there is
  always a leftovers bucket — here 68 conversations at 0.22 cohesion labelled "practice,
  interview, amazon, seems". A proposal machine has to be allowed **no opinion**.
- **Topics explain themselves by the seeds that fired**, ranked by members reached
  (`security 26, safety 8, policy 8`). Mining "distinctive" terms failed twice: first
  returning `tool, exec, output, codex` (frequent everywhere), then `helpful, final,
  touch, ask` (lift-scored). Term mining is for *discovered* clusters; a seeded topic has
  a stated reason.
- **Grouped and split by facet — subject vs mode.** This is what resolves near-duplicate
  feel without merging sets that barely overlap.
- **Overlap is fine; seed-word collisions are not.** A GPU conversation genuinely is
  systems and deep learning. But `agent` in both "Agents and tooling" and "Reinforcement
  learning" manufactures overlap that is not real. Conceptual overlap ≠ bug.
- **Collections stay a rule** — `matches(query) ∪ pinned − excluded` — with the arithmetic
  shown on the card, because storing the rule rather than the membership is what survives
  a reindex.

### Information architecture

- **Grouping is a dimension, not a destination.** Search, Projects and Sittings drew the
  same rows, from the same filters, into the same drawer — two of the three were `GROUP
  BY` wearing the costume of a place, and nothing was in one and not the others. One list
  with `group: none | project | run | topic | source` replaces them. Two axes came free:
  **topic**, which was filterable but never browsable, and **source**, which matters
  because archetype is ~87% predicted by it.
- **A run and a sitting were one primitive at two parameterisations.** Both cluster
  `ended_at`. One was a divider inside a project, the other a third of the top-level
  navigation. One renderer, one word: **run**.
- **The cross-tool requirement on sittings was a display filter posing as a definition.**
  It threw away most runs to keep the ones that made a point. Runs are all shown now;
  cross-tool ones are tagged.
- **One filter state, and all of it drawn.** `state.query = {free, agents, date, dirs,
  topics, range}`. Topics lived in a `Set` and the range in a tuple, and neither appeared
  in the query line — so the list could be narrowed with nothing on screen saying why.
  That is the defect TUI-DESIGN §5 exists to prevent and `me9.16` closed in the shipped
  TUI; the prototype had drifted back into it.
- **A narrowing with no grammar is drawn dashed, not hidden and not faked.**
  `cs_core::query::Facet` is `{Agent, Dir, Date}`, so `topic:` and `when:` cannot be
  typed, copied out or replayed. Inventing syntax for them in a mock would be lying about
  the core; leaving them off the line would be lying about the list. Filed as `me9.18`.
- **The rail is ordered by coverage** — When (100%), Sources (100%), Projects (34%),
  Topics (76%) — not by how interesting the facet is. Topics led and took two thirds of
  the height, pushing both 100% axes below the fold.
- **Every facet is reachable directly.** `dir:` had no rail section and could only be
  acquired by grouping and clicking through, which made it the only filter you could
  apply but not find.
- **Library is the only view that is not derived from the index.** Everything else
  survives `rm index.db && cs index`; collections, pins, merges and dismissals do not.
  This is why Collections kept failing to find a tab — it was the only authored thing,
  competing with derived views for space. Mostly empty, and it says so; these are the
  first empty states in the prototype.
- **Merges are suggested, never applied.** Two projects sharing a last path segment are
  probably one project that moved (`/dev/career` 89 + `/dev/projects/career` 65 = 154),
  but that is an authored fact (ADR 3). Guessing it into the index would make a derived
  column depend on a judgement call.
- **A view that facets do not narrow does not draw the facets.** Sittings drew the rail
  and ignored it — including topic counts that moved when you filtered a list it was not
  showing. Library hides them instead.

### Views

- **Projects merges cwd + lineages + files.** They were three names for one idea, and
  files only mean anything *scoped to a directory*: across the whole corpus the top files
  were `SKILL.md`, `README.md`, `CLAUDE.md` — agent scaffolding — and a bare `README.md`
  matched a pitch deck, a hackathon and an offer letter.
- **Projects is an accordion, not master/detail.** Expanding a row reveals the *same* row
  component the search list draws — same ribbon, same grid, same third line — so the
  preview drawer and every filter come along unchanged. Master/detail spent half the
  width on a directory list and forced a click before anything was legible.
- **A cwd is not a project.** Grouping the raw column gave 111 groups that misrepresented
  the corpus four ways: worktrees split a project across rows (chat-search was 11), 30
  directories were Codex scratch (`~/Documents/Codex/<date>/<slugified-first-message>` —
  the path *is* the opening line), 2 were `/private/var` temp dirs, and one project moved
  on disk. The first three are string rules over the archive, so they stay pure (ADR 1).
  The fourth — `/dev/career` (89) and `/dev/projects/career` (65) are one project — is
  **not derivable and is deliberately left alone**: merging paths is an authored fact and
  belongs in `library.db` (ADR 3). After the rules: **67 projects, 991 conversations.**
- **Counts on a project row are corpus-true, not sample-true.** `114 · 30 here`. The
  topic rail still reads per-sample and is thinner than the corpus because of it.
- **Run dividers are labels, not controls.** Making them collapsible would imply the rows
  beneath are optional; they are the only thing on the screen that is not.
- **Topics go on the project header, never on the conversation rows.** Measured over all
  message text a chat-search conversation matches **7.3 topics (median 7)**; the top topic
  is `Rust and cargo` on **10 of the 12 largest**; density normalisation gives an
  identical 69/114 distinct top-3 signatures; lift-within-project returns `Money and
  finance`, `Travel`, `Health and fitness`. At project scale the fingerprints are sharp
  and differ — chat-search = Rust/SQLite/TUI, personal-site = Web/Transformers/DL,
  career = Interviews/Resume/Money.
- **Project fingerprints rank subjects before modes.** By count alone three of the six
  largest opened with `Git and version control` — true, and useless: a mode fires on every
  agentic project, so it distinguishes none of them.
- **The overflow hands off rather than nesting a scroller.** 20 rows, then `open in
  search →` sets `dir:` — one list implementation, and the handoff is a real query.
- **Sittings share the drawer too.** A sitting answers "what else was I doing that
  afternoon", which you ask *because* you want to read one of the answers.
- **Timeline is a bottom drawer on Search**, filtering the same set. Hits above the
  baseline, sources below. Time is the only signal covering 100% of the corpus.
- **Source marks are CSS masks tinted with the palette**, not full-colour logos: two of
  five ship white-on-transparent (invisible on light), one on an opaque white square, and
  optical weight ranged 17%–70% ink. Masking gives shape + hue + label as three redundant
  channels, per §7's "nothing encoded in colour alone".
- **Inlined as data URIs** — Chrome refuses `file://` subresources, same constraint that
  made the export a script rather than JSON.

---

## 3. Things I got wrong

Recorded because the corrections cost real time and two of them are still live in
published material.

1. **"`Blocks::load` returns `ratatui::Line`."** Wrong, and repeated in both published
   artifacts and in `chat-search-4ar.8`. `Preview::load` returns a `Preview` of plain
   data; `Block` is portable; `lines()` is a separate render step. The only ratatui
   coupling in `Block` is one private `mark_style`. **The seam is far better than I
   described, and `cs show` is mostly a move, not a build.** ⚠ still uncorrected in the
   two artifacts.
2. **"39,000 lines of tested Rust."** Actual: **20,335** across the five crates. Corrected
   in the artifact.
3. **Pipe latency quoted as 1–6 ms.** That was round-trip *including* the query; the
   boundary itself is ~0.2 ms. The corrected chart argues the point harder.
4. **Ribbon drawn per message.** 142 near-identical 1.6px greys. Needed run-length
   encoding — the approved sketch was chunky runs all along.
5. **Mock data sampled each message kind independently**, so tool calls and prose
   interleaved and no agentic runs existed — the exact structure the ribbon exists to
   show. Regenerated segment-wise.
6. **Hidden kinds dropped from the ribbon axis.** Sounded principled; in practice the
   default preset hides tools and reasoning, so every ribbon collapsed to the same
   alternation and the list stopped being triageable. Now dimmed.
7. **Counted "conversations matching 3+ topics" as a defect.** It is the point of tags.
   The real failure modes are redundant pairs (none: max Jaccard 0.27) and topics too
   broad to narrow (that one was real).
8. **First export sampled by recency alone** — 144 claude-code, 25 codex, zero ChatGPT,
   when ChatGPT is two thirds of the corpus. Every cross-tool claim was untestable.
9. **Sittings chained transitively.** A ≤2h gap rule merged 18 conversations across a
   whole day. Needs a total-span bound too.
10. **Reveal-on-scroll hid all content** when the observer did not fire. Never hide
    content behind an animation hook.
11. **The row cap was applied after the run dividers were drawn**, so a divider announced
    "26 conversations" above a run the cap had truncated to 20 — the list contradicting
    its own label. Cut first, then divide.
12. **Ranked a project's topics by term frequency.** Every fingerprint opened `Rust and
    cargo 1392, Prose and editing 1113` — the length bias of a 2,553-message conversation,
    not a fingerprint. Count conversations, not hits.
13. **`screen` and `offer` as seeds for "Interviews and hiring"** put it on chat-search at
    50 conversations. A TUI project talks about screens constantly. A seed shared with the
    general vocabulary manufactures membership the same way a seed shared with another
    topic manufactures overlap — §2 had the rule and the seed list still broke it.
14. **Sparkline over a fixed 26-week window.** chat-search is five days of work, so it
    drew as a dotted rule with one spike — a picture of the axis, not the project. Bucket
    over the project's own span; suppress it below three conversations.
15. **Wired the drawer's controls on `view === 'search'`.** The moment the drawer appeared
    in a second view its fidelity buttons and minimap were dead. Wire by presence.
16. **Sampled only the ten largest projects**, so the small-project fold had nothing to
    fold and looked broken — the same selection-effect mistake as #8, one level down.
17. **Let a second filter state back in.** TUI-DESIGN §5 and `me9.16` both settle that
    query text is the only filter state, and this prototype quietly kept topics in a
    `Set` and the timeline range in a tuple, neither drawn in the query line. I wrote the
    projects view against that state for two rounds without noticing. A principle that
    only holds in the crate it was written for is not holding.
18. **Built Projects and Sittings as views.** They were `GROUP BY`, and treating them as
    destinations is what forced three copies of the list, left the rail inert in one of
    them, and made a run a divider in one place and a tab in another.
19. **Shipped Library drawing the rail beside it** — repeating, in the same pass, the
    exact inert-control defect the pass existed to fix. Caught in the screenshot.

---

## 4. Rejected or dropped

- **Inbox / triage view** — did not earn its place on review.
- **Sittings as a view** — see §2. The rule survives as `group: run`.
- ~~**Standalone Collections view**~~ — reinstated as **Library**. It was dropped because
  it could not justify a tab against three derived views; once those collapsed into one
  grouped list, the authored surface was the only thing left that genuinely needed one.
- **Files as a global view** — only meaningful scoped to a directory (see §2).
- **k-means / unsupervised clustering** — always produces a garbage bucket.
- **Automated sub-topic discovery** — vocabulary is not separable at that granularity.
- **Auto-derived topic groups from member overlap** — max Jaccard 0.27 makes any grouping
  arbitrary. Hand-grouping is the honest tool.
- **Semantic topic segmentation** — deferred; needs `6eb.41` then `6eb.40`. The free
  structural boundaries (steers, >4h pauses, resumes) ship first and are the floor a model
  has to beat.
- **`git_branch` as an axis in Projects** — present and unused (932 conversations, 93
  branches; personal-site 27, mars-attacks 16, chat-search 3 + 10 worktree branches).
  Branch names are human-authored labels and the best subject signal in the corpus, but
  deprioritised by hand: the accordion answers the question first. Left in §5.
- **Nesting projects under a parent** (`spar-gd` is 54 conversations across 7 directories,
  `rl` 51 across 6) — a second disclosure level on every row to fix 3 cases. The
  small-project fold handles the symptom.
- **Per-conversation topic chips inside a project** — see §2; they print the same seven
  tags on every row.

---

## 5. Open questions

- ~~**Sittings are rare**~~ — resolved by making them a grouping. A run is now every
  cluster of `ended_at`, not only the cross-tool ones, so there are 109 in the sample
  rather than 8.
- ~~**37% untagged needs an answer**~~ — it is a group of its own under `group: topic`,
  with the reason on the band. Still unanswered in the rail, where the topic chips are
  the only thing shown and the residue is invisible.
- **Two views are gone; their keyboard model went with them.** `⌘\` panes, `→` expand and
  arrow navigation are drawn in the footer and wired nowhere.
- **Grouping and ranking interact and nobody has decided how.** With `group: none` the
  list is recency-ordered; grouped, the groups are ordered by recency and the rows inside
  by recency. A real query would want BM25 in both places, and "the best group" is not
  obviously "the group with the most recent row".
- **A project that moved on disk is two rows.** `/dev/career` + `/dev/projects/career` =
  154 conversations. Not derivable; wants `library.db` and an authored merge.
- **`git_branch` is unused** — 932 conversations, 93 branches. See §4.
- Left rail is now long enough that Sources and When fall below the fold.
- Reasoning violet is hard to spot at 2px in the ribbon.
- Topic counts in the rail are per-sample; project counts are now corpus-true, so two
  numbers on one screen mean different things.
- No empty, error or loading states anywhere in the prototype.
- 61 DOM nodes per row × 655 results ≈ 40,000 nodes — needs virtualisation.
- ~~File extraction keeps basenames~~ — fixed for the project rollup (`PATH_RE` keeps the
  path, `SCAFFOLD` drops `SKILL.md`/`CLAUDE.md`/`AGENTS.md`). Per-conversation `files`
  still uses basenames.
- Two published artifacts still carry the `Blocks::load` error.

---

## 6. What to build next, and why

**`cs show` + the core move.** It is largely the same work as `me9.1.1` (P1, in progress),
which is the fold model, tool collapsing and outline mode — exactly what would move to
`cs-core`. It unblocks the preview pane here, `me9.8`'s reader, and the VS Code client
(`me9.9`). Three surfaces, one method, and it improves the TUI you use daily.

Cheaper first step, no Rust: `Group` already carries `match_seqs`, `user_turns`,
`msg_count`, `prose_count`, `cwd`, `ended_date`, `title` and `destinations` — so the row
is feedable from `cs search --json` today. Only the preview pane actually needs `cs show`.

---

## 7. Reproducing the measurements

```bash
python3 poc/ui/export.py            # sample + topics + projects + lineages -> real-data.js
python3 poc/ui/export.py --limit 400 --per-project 30
python3 poc/ui/icons.py --rebuild   # re-inline source marks
```

The sample is stratified three ways — by source, by size, and by project. The third
stratum exists because source-stratified sampling alone gave 9 projects, six of them with
four conversations or fewer: enough to draw a list of projects, not enough to exercise
what goes inside one. Current output: **354 conversations, 8.8 MB, 284 in 33 projects.**

Ad-hoc corpus queries go through the `sqlite3` CLI against
`~/.chat-archive/index.db` (read-only). Python's `sqlite3` module could not open that
path in this environment; the CLI can. Workaround when the module is needed — including
for `export.py` itself, which is Python:

```bash
cp -f ~/.chat-archive/index.db /tmp/idx.db && python3 poc/ui/export.py --db /tmp/idx.db
```

`real-data.js` is gitignored — it is conversation text, and `.gitignore`'s existing
transcript rule covers it. It contains genuinely personal material.
