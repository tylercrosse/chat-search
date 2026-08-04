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

### Compaction

11 conversations, 12 boundaries, **all claude-code, all mid-stream** (seq 51–924, never
at the head) — so a compaction splits a conversation rather than starting one, and
`forked_from` is null on every one of them. One 476-message conversation compacted twice.

Rare overall at 0.4% of the corpus, but that is the wrong denominator:

| claude-code length | conversations | with a compaction |
| --- | --- | --- |
| 600+ | 13 | **3 (23%)** |
| 300–599 | 33 | 2 (6%) |
| 100–299 | 92 | 5 (5%) |
| under 100 | 243 | **0** |

It appears exactly where a reader is most lost and nowhere else. Detection is a string
match on a harness sentence, so it is a heuristic: it will drift when the wording changes
and finds nothing for Codex, which also compacts. The durable version records the
boundary in the importer.

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
- **The four kind colours sit on a luminance ramp, not four hues.** Measured, the old
  palette separated them by hue and not by luminance — `agent`/`reasoning` were 2.00
  apart in dark and **1.12 in light**, i.e. the same colour — and hue is the channel that
  degrades fastest at the 2px these bands are drawn at. §7 says nothing is encoded in
  colour alone, and kind *is*, so the colour has to carry it on the strong channel. Now
  2.2 / 4.0 / 7.2 / 13.0 against the track — an even ~1.8× per step, in both themes.
  Order is agent brightest (the prose you are usually after) then user, reasoning, tool
  (66–85% of the volume, so it must be quietest or the ribbon is a wall).
- **The ribbon stays message-count weighted, and that is correct.** By bytes the corpus
  is prose 42.3% / tool_result 34.7% / tool_call 20.7% / reasoning 2.4%, against message
  shares of 23.6 / 34.0 / 34.5 / 7.8 — so a byte-weighted ribbon would look very
  different. It would also stop being a *map*: positions have to agree with the match
  ticks and the fold model, both of which index messages. Length is already encoded
  where it belongs — `mapBands` varies bar height by `log10(len)` on a count axis.
- **Tool calls carry an act, and the act is drawn.** `look / change / run / steer`, from
  the tool name, which the harnesses spell differently for the same capability (codex
  `exec_command`, claude-code `Bash`). Corpus: run 47,479 · change 9,460 · look 4,502 ·
  steer 1,164. Ribbon runs break on act as well as kind, so a patch landing inside forty
  exec calls is its own band. A `tool_result` inherits its call's act — one event.
- **Act sub-shades are deliberately narrow** — 1.29× and 1.42× apart, all below
  reasoning's 4.0 — because the primary read has to stay on the main ramp. They say
  "something changed here", they are not independently decodable at 2px. The glyph in the
  preview gutter carries the distinction where there is room for it.
- **The ribbon's drawn width is the length cue.** It was a fixed 236px whether the
  conversation held 10 messages or 2,553, so the one mark that could carry scale refused
  to. Log-scaled with a 14% floor: linear would render everything under ~200 messages as
  a stub. The *cell* keeps its full width so the columns either side stay on the grid.
- **Hidden kinds are dimmed on the maps, not dropped from the axis.** Reversed mid-session
  — see §3.

### Preview

- **The drawer says what the conversation *did*, not only what it was made of.** One line
  of acts (`353 calls · run 47% · change 27% · look 24%`), one of files touched. Both are
  free — the act is the tool name, the paths were already mined for the project rollup
  and thrown away at conversation scale.
- **Subagents share the acts line rather than owning one.** 57 conversations in the whole
  corpus carry any, so a label blank on 98% of rows is not worth a row — but where it
  appears it averages 52% of the conversation, so it earns a badge.
- **Model sits in the meta line.** Whether it changes inside a conversation is
  **unanswerable from the index** — see §3 #23. It is shown as a conversation-level fact
  because that is the only shape the index has.
- **The asked-you mark is the union of two signals.** `request_user_input` is exact and
  rare (237 calls in 92 conversations); assistant prose ending in `?` is broad and noisy
  (1,402 in 578). Neither alone is the fact.
- **A compaction boundary is marked, and is not a pause.** A pause says you went away; a
  compaction says the earlier half stopped being verbatim, so the agent past it knows
  different things. Distinct mark on the ribbon and the minimap (doubled, full height,
  against the notch's single partial stroke) and a rule in the transcript, because at
  seq 924 of 1,323 you would never find it by scrolling.
- **~602px (78ch)**, paid for by the title moving off the column grid. First time it has
  had a real reading measure.
- **Gutter spine carries role and kind; text contrast is identical for user and agent.**
  `me9.1.1` records "do not privilege user turns; an assistant answer is often the thing
  being looked for", and the earlier mock violated it by dimming assistant prose.
- **Per-kind fidelity is the model** (user / agent / reasoning / tools × hidden /
  collapsed / expanded). The preset names are presets over those four knobs.
- **Visibility and detail are two axes, not three states on one.** Hiding a kind is "is
  it on screen"; brief/full is "how much of it". Conflating them is why every arrangement
  of this control felt wrong: with a single 3-cycle, one of the six transitions always
  costs two clicks. One chip per kind carries both — the **body cycles** off → brief →
  full → off (shift-click reverses), the **dot** toggles visibility and restores the
  level that kind last had.
- **The cycle order follows the path you actually walk**: peek at the tools, read them,
  put them away. See §3 #22 — the earlier decision here optimised the rare transition.
- **Label, state and dot live in one box.** The 2×2 grid that preceded it put `you`'s
  control nearer to `agent`'s *label* than `agent`'s own control was, so proximity
  pointed at the wrong thing on every read. That was most of the fiddliness, not the
  cycling.
- **The state is a word, not a glyph.** `○ ◐ ●` is a legend you have to learn, on an 18px
  target; `off / brief / full` is neither.
- **Four presets, no all-buttons.** There were three presets plus `expand all` and
  `collapse all`, of which two were the same command — `outline` set every kind to
  collapsed and so did `collapse all` — while `full` was *not* full (reasoning and tools
  stayed collapsed) and `expand all` was. Now `segments · outline · read · everything`,
  each distinct and named for what it does. 17 controls to 8.
- **The drawer's topic chips fold to three.** Sixteen of them took four rows and pushed
  the controls halfway down the drawer; the header went 242px → 172px.
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
- **The source badge lost its box.** It was the heaviest mark in a row — bordered, mono,
  saturated — while carrying the least discriminating information: inside a project all
  twenty rows show the identical badge. Colour now lives on the icon alone; shape, hue
  and word remain three redundant channels, only the frame is gone.
- **Five type sizes, not ten.** 13 / 12.5 / 11.5 / 10 / 9. There had been ten, with 313
  of ~380 text nodes crammed into 10 / 9.5 / 9 — three steps inside one pixel, doing
  different jobs, which reads as drift rather than hierarchy.
- **`--ink-3` is 4.6:1 in both themes.** It was 3.64 in dark and **2.90 in light**, at
  9–11px, on a tier carrying date spans, group counts, section labels, the stat line and
  the footer. That is real information below the AA floor for text that size.
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
20. **Dimmed hidden kinds to 0.22 opacity.** On the new ramp that took a dimmed tool
    band to **1.15:1** against the track — invisible, which is "dropped from the axis"
    by another name, and #6 above is the entry where I already learned that. 0.55 now.
23. **Reported a measurement that could not have said anything.** I wrote "model never
    changes inside a conversation — 0 of 3,057". `model` is a single column on
    `conversation` and there is no per-message model in the index, so the query joined
    one value onto every message and counted distinct: 1 by construction. The right
    answer is that the index cannot say. Chasing it found a real defect — the two
    importers collapse the archive's per-message model in opposite directions,
    `claude_desktop` keeping the last and `chatgpt_export` the first, both silently and
    neither tested. Filed as `chat-search-n58.25`.
22. **Rejected a cycling control for the wrong reason.** The note said cycling forced
    you through `hidden` to get from expanded back to collapsed — true, but that is the
    *rare* transition. The common path is hidden → collapsed → expanded → hidden: peek
    at the tools, read them, put them away. I optimised the transition nobody makes and
    shipped twelve 18px targets to do it, then had to be told the result felt fiddly.
    The deeper error was treating visibility and detail as one axis.
21. **Claimed the selection fill ran under the ribbon and hurt it.** It does not:
    `.rb-track` is opaque `--map-bg` and spans the cell, so the computed background is
    identical on selected and unselected rows. Measured before fixing; nothing to fix.
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
- **Time as the ribbon axis** — measured and rejected. On conversations over 80 messages
  the single longest gap is **45% of the wall-clock span on average**, up to 99.9%, so a
  time-weighted ribbon is one blank stripe with the work crushed at the edges. The notch
  is the right way to show a pause.
- **A retry/error-storm graphic** — measured and rejected. 822 runs of a single failure,
  8 of two, nothing longer. A failure is a one-off, so the single red tick is right.
- **Byte-weighting the ribbon** — see §2. It answers a composition question the ribbon is
  not asking, and would break its correspondence with the ticks and the fold.
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
