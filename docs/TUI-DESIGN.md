# TUI design note

Working spec for `chat-search-me9.1` (native TUI). Records the crate boundary, layout, the row
model, titles, filter surface, destinations, colour discipline and the preview pane, plus what was
lifted from and rejected in [fast-resume](https://github.com/angristan/fast-resume) after reading
its source on 2026-07-29 (v2.5.0, ratatui 0.30, ~4,100 lines of TUI across
`tui.rs` + `tui/{layout,render,state,input,preview,text,images}.rs`).

Already decided elsewhere, not restated here:

- No worker thread, generation counter or debounce — the search is synchronous at 1.4–6.4 ms
  (`me9.1` design field, with its revisit trigger at ~30 ms).
- No daemon; a Rust TUI links `cs-core` directly (decision 14).
- `cs search --json` is the client contract (decision 12).
- Titles are a fold, not a stored value (decision 8).

**What here is about terminals, and what is about the product** (added 2026-08-04, `4ar.8`).
§1, §2 and §3 belong to the terminal client: a crate boundary, a column plan in character cells,
and a row model budgeted in screen lines. A second client is not bound by them, and is expected
to say where it differs rather than differ quietly — §3 now names the one divergence that
exists. §4 through §8 are not terminal-specific. The title fallback chain, one filter state, the
destination model, the colour rules and the fold model are claims about the data and about what
a reader needs, so a GUI that breaks one breaks it for the same reason a TUI would; `poc/ui`
cites §5 and §7 as binding on itself and that reading is correct.

---

## 1. Crate boundary

```
cs-tui   depends on cs-core only.
         Entry: run(db_path: PathBuf, log: &mut dyn FnMut(querylog::Event), opts) -> Result<Exit>
cs       depends on cs-archive already.  Resolves cfg.default_db() and the queries.jsonl
         path, passes both in, and exposes the TUI as a `cs tui` subcommand.
```

**The TUI takes a log sink, not a log path.** `querylog::append` writes to
`<archive_root>/queries.jsonl` (`6eb.22`), which the TUI does not know and should not learn —
the same argument that keeps `default_db()` in `cs-archive`. A sink keeps all filesystem policy
in `cs`, lets tests capture events without touching disk, and puts `append`'s own rule — *a
search that cannot write its log line should still return results* — in one place instead of at
every call site.

**The TUI never resolves the index path.** `default_db()` stays on `Config` in `cs-archive`
(`config.rs:99`), where a doc comment already argues for it: path resolution is a question about
the config, and pointing `cs-core` at a config would run the dependency backwards and drag `toml`
and the globs into the search crate. A client that resolved its own path would need `cs-archive`
— the scanner, machine identity and the manifest — to compute one `PathBuf::join`. Taking the
path as a parameter costs nothing and keeps the edge clean.

This resolves the contradiction in `me9.12`'s acceptance criteria, which permits `default_db()`
living in `cs-archive` in one clause and asks that a second Rust client get it from `cs-core` in
another. It gets it from its caller.

**Ships as `cs tui`, not a separate binary.** `build_info` records `IMPORTER_VERSION` so a stale
index is detectable rather than silently wrong (`schema.rs:66-68`). A separately-installed TUI
binary could drift from the `cs` that built the index, which means duplicating that check in every
client. One binary, one version. `ratatui` is not initialised unless the subcommand runs, so the
CLI's startup path is unaffected.

The TUI's *code* still lives in its own crate rather than in the binary — `cs` is a thin shell
that resolves a path and calls `run`. That is what keeps the dependency edge honest.

---

## 2. Layout

Five bands, one flexible. fast-resume's skeleton (`tui/layout.rs:39-58`) is the right shape and
we adopt it unchanged:

```
Length(1)   header      result-set scale, corpus scale, latency
Length(3)   search      bordered, always focused
Length(1)   filters     source facets + coverage state
Min(3)      main        results, optionally split with preview
Length(1)   footer      key hints, or status when there is one
```

**The header's middle number is the result set's real size**, not the drawn one:
`50 of 655 matched / 3019 indexed`. `limit` caps what is drawn at a screenful and a half, so
without it a list that is the whole answer and a list that is the first page of a long one are
the same screen — and "keep typing" and "you have seen everything" are opposite conclusions
drawn from the same rows. `50 of 50 matched` states the complete case rather than going quiet,
because a form that appears only sometimes makes the reader infer meaning from an absence.

**A keystroke never pays for it.** `cs_core::answer` reads the total off the ranking pass,
which already visits every matching message unless it stops at its own `limit * 50` scan
ceiling — so every query narrow enough to finish typing is answered for free. The rest come
back `settled: false`, and the header draws `50 of … matched` rather than the floor the answer
carries: the floor is an artifact of where the scan stopped, about half the truth on this
corpus, and a number that lands, is read, and then doubles is worse than one that never
claimed to be ready.

**`Answer::settle` settles it 250 ms after the last keystroke**, from the event loop, gated on
there being anything to settle (`App::unsettled`). That timing is the whole design rather
than a detail. Settling costs 5–36 ms against this corpus and only ever on a broad prefix —
which is exactly a query on its way somewhere, whose total nobody is reading yet. Charging it
per keystroke spent the milliseconds at the one moment they bought nothing; charging it to the
pause spends them when the number is looked at. Measured at `limit 50`:

```
   search    +count   settle    total   query
     0.2       0.2      0.0       37   borrow checker
     9.9      10.0      5.5    …1583   ind
    60.0      60.2     36.6    …3008   the
```

This is not the generation counting `me9.4` closed and ADR 14 rules out. There is no
concurrency: the count runs on the event loop like everything else, and a keystroke arriving
first replaces the whole state before it is reached. The wait is one `event::poll` timeout
that exists only while a total is outstanding, so an idle TUI is still a process asleep in
`read()` rather than one waking to redraw an unchanged screen. `search::count_cost` guards
both halves — the keystroke at zero, the settle bounded.

The corpus total sheds first when the line will not fit. It is the only one of the three that
does not move as you type, and it is also the sum the facet bar is already showing.

**Two independent levels of responsiveness.** This is the part worth copying deliberately,
because the naive approach — hide the preview when it gets tight — is worse than both levels.

*Pane level.* At `width >= 116`, horizontal split 62/38. Below that, **reflow to vertical**:
results `Min(8)`, preview `Length(12)`. The preview is never dropped for width; it is dropped
only on the explicit toggle. A user who narrows a window has not asked to lose the preview.

*Column level.* Progressive shedding with a floor on Title. fast-resume's plan
(`render.rs:507-534`) sheds Directory first at `<72`; we shed **Msgs** first and the density
strip second, because for this corpus the directory is a stronger retrieval cue than either —
`6eb.26` measured date and project as the strongest recognition cues, "stronger than anything in
the text."

| inner width | agent | dir | density | msgs | age | title |
| ---: | ---: | ---: | ---: | ---: | ---: | --- |
| ≥116 | 13 | 30 | 10 | 6 | 9 | remainder, floor 16 |
| ≥100 | 13 | 28 | 10 | 0 | 9 | remainder, floor 16 |
| ≥72 | 12 | 22 | 0 | 0 | 8 | remainder, floor 16 |
| <72 | 12 | 0 | 0 | 0 | 7 | remainder, floor 16 |

One blank column separates each *visible* pair; a hidden column consumes no gap. Implemented in
`cs-tui/src/layout.rs`, whose tests assert over a width sweep that no two visible columns overlap.

**These widths are the results pane's inner width, not the terminal's**, and the two are far
apart while the preview is up: 62% of a 116-column terminal is 71, so the widest column plan does
not appear until roughly 190 terminal columns — or at 118 with the preview toggled off. That is
the intended trade. Msgs is the least load-bearing column, so needing room to spare before it
appears is correct, and the `116` here is numerically equal to `SPLIT_MIN_WIDTH` by coincidence
rather than by derivation. The two thresholds are free to move apart and the code keeps them
separate deliberately.

**Path truncation must not be tail-elided.** fast-resume's `truncate` (`tui/text.rs:27-44`)
appends `...` after a head slice, so `~/dev/projects/personal-site` at width 20 becomes
`~/dev/projects/pe...` — it discards exactly the discriminating token. Paths get
middle-elision with the leaf preserved: `~/dev/…/personal-site`. Titles keep tail-elision;
the front of a title is the discriminating part.

---

## 3. The row model

The open question was one line per conversation versus two or three. `me9.1`'s acceptance
criteria already commits to something more capable than either — *"a single flat cursor that
moves through conversation headers and their message hits in visual order"* — so the real
question is how many hit rows and whether they are selectable.

**Revised 2026-07-30, after `6eb.23` landed.** `Group` now carries `match_seqs` — every match's
0-based position — and `cs_core::match_density(seqs, msg_count)` renders a fixed ten-cell strip
(`· ▁ ▄ █`). That bead chose the strip explicitly *over* showing three snippets, because "it gets
noisy when two of the three are unrelated." That critique lands, so hit rows stop being automatic:

**Decision: header row carries the density strip; hit rows expand on demand.**

| mode | rows per conversation |
| --- | --- |
| blank query (recent fallback, `6eb.5`) | header only |
| non-blank query | header, with the ten-cell strip |
| non-blank query, expanded | header + up to 3 message hits |

The strip is the always-on signal and costs 10 columns with no vertical cost — it answers *where
in the conversation the matches sit*, which is the "is this the subject or an aside" question.
Hit rows answer *what they say*, which is worth vertical space only when you have asked. Expansion
is per-conversation, on the selected row.

The strip and outline mode (§8) are the same data at two resolutions: ten buckets versus one cell
per message. Neither should be computed twice — `match_density` lives in cs-core for the same
reason `ended_date` does.

Reasoning:

- **A snippet on the *selected* row is redundant** — the preview already shows it, highlighted.
  What is additive is match evidence on the rows the cursor is *not* on, because that is what
  lets you triage the list without moving. So the cheap "expand the selected row" pattern buys
  nothing here, and the expensive one is the useful one.
- **Under a blank query there is no match evidence to show.** A second line would carry the
  conversation's opening prose, which is roughly what the title already is. Density is the only
  thing worth having in browse mode.
- **Selectable hit rows earn their keep by anchoring the preview scroll.** They do not earn it
  through `Enter`: no terminal resume can open at a message, so for the CLI destination a hit row
  and its header resolve to the same action. The payoff is that moving the cursor down through
  hits scrolls the preview to each match, which is the cheapest possible "show me the other
  places this matched." It also pre-builds the selection model a conversation reader (`me9.8`)
  would need.
- **Cap at 3.** Uncapped hits let one verbose conversation fill the viewport. If a conversation
  has more, the header shows `+N more` and the preview is where you go.

**Revised 2026-08-04, against the live index (`4ar.8`).** 3,059 conversations / 184,099 messages,
measured while building `poc/ui`; the full log is `poc/ui/NOTES.md` §1. The decision above
survives. One of its four reasons does not, and a second gains a number that cuts the other way.

**"Opening prose is roughly the title" is a Codex property, not a corpus property.**
`title_origin` by source: codex **99.4% `first_user`** of 655, claude-code 11.0% `first_user` and
**75.9% generated** of 381, chatgpt **99.9% custom** of 2,011. So the second bullet holds for
about 22% of the corpus and is false for the rest — a ChatGPT thread's custom title and its
opening turn are different sentences, and a claude-code title was generated by a model that had
read the whole conversation.

The decision does not move, because the reason that actually carries it is **vertical budget,
not redundancy**: a second line halves rows-per-screen, and `6eb.26` measured date and project —
both already on line 1 — as the strongest recognition cues, "stronger than anything in the
text." That is a claim about what a terminal row can afford. It was never a claim that a second
line would have nothing to put on it, and it should not be cited as one.

**The GUI prototype diverges, and the divergence is narrower than it reads.** `poc/ui` draws
metadata and the ribbon, then the title on its own full-width line, then topics, then — under a
query only — one best-match line: three lines browsing and four with a query, against this
section's one and one-plus-expansion. (`4ar.8` was filed against an earlier two-and-three
prototype, which is itself the argument for recording the divergence here rather than there.)
It is not spending a line on opening prose either. Its line 2 is the *title*, which this section
puts in the remainder of line 1; it moved because a nine-cell metadata grid in a 706px list
column leaves the title no usable measure, where §2's column plan gives it the remainder with a
floor of 16. Two answers to a width question asked at two widths. What is genuinely in dispute
is smaller:

- **Topics as a line of their own.** The index has no topic column, so the TUI could not draw
  this today and the question does not arise until one lands.
- **One always-on hit line, against up to three on demand.** A real disagreement with the table
  above, and the measurements are on the prototype's side of it.

**Most hits are in tool traffic, which argues for showing one rather than hiding all.**
Message-level, by query: `timezone` 85% of hits in tool traffic, `borrow checker` 78%,
`schema migration` 74%, `fts5` 66%. At conversation level, 20–73% of matching conversations
match *only* there. That sharpens `6eb.23`'s objection to three snippets — "it gets noisy when
two of the three are unrelated" — rather than softening it: three fragments of file contents is
the ordinary case, not the bad one. But it is an objection to *three unlabelled* snippets, and
the first bullet above already concedes that the additive evidence is the evidence on rows the
cursor is not on. One hit line, framed with what matched (`⚙ Bash(cargo test) › "…"`), costs one
screen line and answers both, because the frame is what stops a tool hit reading as log spew.
`hit_row` today renders the snippet with `[subagent]` and `[edited away]` tags and says nothing
about kind or tool — `me9.24`.

**Ten cells is coarser than the structure underneath it, deliberately.** A conversation has 2–8
natural segments — user turns, at 20.4 messages per steer on claude-code and 37.0 on codex — so
the strip sits between the segmentation and the message axis. That is fine enough to separate
"the subject" from "an aside", which is the question §3 gives it, and too coarse to say which
run a hit landed in on a 600-message session, which is the question the outline (§8) and the
preview anchor answer instead.

**Implementation consequence.** Materialise a flat `Vec<Row>` once per search, where
`Row = Header { conv } | Hit { conv, msg }`. Do not compute offsets from the tree during render.
This is what keeps scrolling, `PageDown` and mouse hit-testing uniform — you scroll over rows,
not over conversations, so fast-resume's one-line-per-row arithmetic (`render.rs:476-490`) stays
valid despite the variable rows-per-conversation. The tree-to-cursor flattening that `me9.1`'s
design field calls "the real work" is exactly this vector.

**Aggregates on the header row.** Once grouped, the header's numbers are aggregates:

- `Msgs` — **sum** across the group. A conversation resumed four times is one long
  conversation, and the total is what says whether it was substantial. Rename from `Turns`;
  per-thread counts are meaningless after grouping, and a range spends width to say less.
- `Age` — **last-touched, single value.** It is what you navigate by, and it stays one token
  wide in a column competing with Directory. The full span goes in the preview header next to
  the absolute timestamp.
- Fork count only when > 1, as a marker beside the title (`⑂3`), not a numeric column.

**Ranking must decay on the same timestamp the column shows.** If the decay uses one
`ended_at` and `Age` renders another, the order looks broken for precisely the resumed
conversations that grouping exists to fix.

---

## 4. Titles and the fallback chain

fast-resume renders `session.title` raw, and it shows: of 49 visible rows in a real screenshot
of this machine's corpus, **12 were harness machinery** — `<environment_context>` five times
identically, `<ide_opened_file>The user opened the file...` three times,
`Base directory for this skill: /Users/...` twice, plus `<ide_selection>` and
`The following is the Codex agent history whose request action...`. Five mutually
indistinguishable rows cannot be chosen between at all.

That is `n58.3` and `chat-search-202` rendered at full size, and it sets the client-side
requirement: **the title cell is never empty and never shows raw markup.** Resolution order,
as a fold (decision 8):

1. Cleaned title — markup stripped per the importer's rules.
2. First authored user prose, walking *forward* past messages that are markup-only.
3. `leaf-dir · date` as the floor.

**Step 2 is almost entirely a Codex fix** (added 2026-08-04, `4ar.8`). `title_origin` is
`first_user` for 99.4% of codex conversations, against 11.0% of claude-code (75.9% generated)
and 0.1% of chatgpt (99.9% custom) — and a title taken from the first user message is exactly
the one that can be `<environment_context>`. Where the title was authored by the user or
generated by something that had read the conversation, step 1 already holds a real sentence and
the walk never runs. Both the fold's cost and its payoff are concentrated on one source, which
is a reason to build it rather than a reason to scope it down.

**Where this bites hardest is browse mode, not search mode.** Under a query the hit rows carry
the row and a bad title is survivable. Blank query has no hit rows by the decision above — so
the mode with no fallback is the one that just shipped as the default empty state. That is the
argument for pulling `6eb.15` (title resolution fold) up from P3.

---

## 5. Filters: one source of truth

**Sequencing note, superseded 2026-07-31.** This section previously recorded `6eb.11` as
blocked by `6eb.26` and therefore unavailable at TUI v1. That dependency was removed on
2026-07-30 — `dir:` selects on `conversation.cwd`, which is already populated (100% on both
agent sources) and needs no project derivation — and `6eb.11` landed on 2026-07-31. The DSL
*is* available at v1. What `6eb.26` still buys is the facet **name** for the chip row, which
belongs with `me9.14`. `6eb.26`'s empirical finding stands and is unaffected: date and project
are the strongest recognition cues, "stronger than anything in the text."

**As shipped.** `cs_core::query` parses the whole DSL and `cs_core::search` applies it, so the
CLI and the TUI filter through one parser and one set of SQL clauses. `agent:` and `dir:` take
comma lists; both negation spellings work and mean the same thing; repeated tokens of one facet
union, and repeated `date:` tokens intersect so two bounds make a range. `date:` takes
`today`, `yesterday`, `week`, `month`, `<Nu` and `>Nu` with units `m|h|d|w|mo|y`, and its
day/week/month/year arithmetic is civil rather than fixed-width — a day across a DST boundary
is 23 or 25 hours, pinned in `cs_core::time`'s tests at 82,800 s and 90,000 s. A value nothing
can select on (`date:nope`, a half-typed `agent:`) neither errors nor filters: it is reported
by `Query::rejected` and shown by both clients, which is the affordance the struck-through
rendering below builds on.

fast-resume already ships the DSL `6eb.11` specifies (`query.rs`): `agent:claude,!codex`,
`-dir:test`, `dir:"my project"`, `date:today|yesterday|week|month`, `date:<2d`, `date:>1w` with
units `m|h|d|w|mo|y`. One regex extracts keywords; the remainder is free text. Worth lifting
wholesale, with four details:

- **`agent:` matches exact, `dir:` matches case-insensitive substring** (`query.rs:39-56`).
  Enums want equality, paths want substring.
- **Both negation forms** — prefix `-agent:x` and inline `agent:!x` — so include and exclude
  mix in one term.
- **Live syntax highlighting in the input, with invalid values struck through in red**
  (`tui/text.rs:68-141`). `date:nope` renders crossed-out as you type. This is what makes an
  undocumented DSL self-teaching, and it is ~70 lines.
- **Ghost-text completion** of `agent:`/`date:` values on `Tab`, falling back to cycling the
  facet when there is no suggestion (`tui/input.rs:34-38`).

**The facet bar is a projection of query state, not a parallel filter.** fast-resume gets this
right in mechanism — `cycle_agent` rewrites the query *text* (`tui/state.rs:317-344, 445-455`) —
then undermines it by keeping a second source of truth, a `agent_filter` field fed from the CLI
flag. The cost is six reconciliation methods (`active_agent_filter`, `active_agent_filters`,
`effective_agent_filter`, `count_agent_filter`, `all_agent_filter_active`,
`clear_explicit_filter_if_query_has_agent`). **Desugar CLI flags into the query string at
startup and keep exactly one state.**

**Done, 2026-08-01 (`me9.16`).** `App` has no source field: a chip click calls
`Query::toggling`, which returns the query *text* with the `agent:` token added or taken back
out, and the bar draws itself from `Query::selection`. So a source chosen from the bar is
visible in the input box, editable there, and survives being copied out of it — none of which
was true while the selection lived beside the query. `--source` is desugared once by
`Query::with_source`, which rewrites the text rather than pushing a filter in beside it, so
what the flag produces is indistinguishable from what a click or a keystroke produces and
there is nothing left to reconcile. Clicking a second chip widens the selection
(`agent:codex,claude-code`) because repeated values union; the All chip clears the facet,
exclusions included. Rewriting rules live in `cs_core::query` with the grammar, not in the
bar — a client assembling `agent:` tokens itself would be a second, partial parser.

**Graceful degradation is a formal ladder, and it is where the tests go.** fast-resume tries
four stages of `(counts, icons, labels)` in order, then falls back to a window anchored on the
*active* facet so your current filter never scrolls off (`render.rs:243-311`). Five unit tests
cover it. Adopt the pattern; that anchoring rule is the non-obvious half.

**The facet bar is also the only place coverage state is visible.**
`agent_filters_with_sessions` (`tui/state.rs:278-286`) keeps only sources where `count > 0`:

```rust
let count = self.engine.count_for_agent(Some(agent));
(count > 0).then_some((*agent, count))
```

So a source with zero indexed conversations is **invisible by construction**. A configured
source whose importer threw, or whose archive run never happened, produces a chip row that
looks complete. You search, get nothing, and conclude you used a different tool — when the
truth is a broken importer. That is the failure the Capture epic explicitly forbids: *a source
that is skipped says so; it never just fails to appear.*

Build the chip row from **config ∪ index**, in three states:

| state | render |
| --- | --- |
| configured, has rows | normal chip, count |
| configured, zero rows | dim chip, `!`, count 0 |
| detected on machine, not configured | distinct dim style, no count (`a7k.12`) |

---

## 6. Destinations

`me9.3` established that `resume_cmd` cannot be a stored string, and **landed**: the column is
gone and `cs_core::destinations(source, native_id) -> Vec<Destination>` resolves it at action
time. fast-resume independently reached the same conclusion — the command is computed from a
per-source trait, `adapter_for(&session.agent).resume_command(&session, yolo)`
(`tui/input.rs:130-134`).

Two deltas from the sketch below, both settled by the implementation. It takes the id pair
rather than a `&Conversation`, because the callers holding a search result do not have a parsed
conversation and ADR 2 makes the pair the stable part anyway. And `Destination` is an enum of
`Terminal { argv }` / `Web { url }` rather than a string, so nothing downstream re-sniffs
`startswith("http")` or splits a command line on whitespace.

Its permissions modal generalises directly into the destination picker. Three things to lift
from `begin_action` (`tui/input.rs:63-85`):

1. **Capability probe per source.** `supports_yolo()` becomes
   `destinations(&conversation) -> Vec<Destination>`. Codex has no web surface; Claude.ai has
   no terminal resume. The modal offers what is reachable rather than four options where two
   error.
2. **Skip the modal when there is one answer.** Their `!supports_yolo` short-circuit becomes
   `if destinations.len() == 1`. Nobody should confirm a choice that is not one.
3. **A sticky default.** Their global `state.yolo` flag bypasses the prompt permanently; ours is
   a default destination, so `Enter` goes straight there and a modifier opens the picker.

### Logging the pick

`Enter` is also where ground truth is recorded, so the open action and the log event are one
moment (`6eb.22`).

**Never emit `Search` per keystroke.** That bead excludes typeahead deliberately: `--prefix` fires
once per keystroke, so logging it buries the handful of real queries under every prefix of each.
The TUI is pure typeahead, so it is exactly the client that rule was written for.

| moment | event |
| --- | --- |
| keystroke | **nothing** |
| `Enter` on a row | one `Pick { q, conv_id, rank, shown, n }` — finished query, 1-based rank, `shown` truncated to `MAX_SHOWN` = 20 |
| quit with a non-blank query and no pick | one `Search { q, shown, n, ms }` |

That last row is a judgement call worth stating: a `Search` with no `Pick` is the abandonment
signal — *the ranking showed me nothing worth opening* — and it is information `6eb.21` cannot get
any other way. Without it the log only ever records successes.

On a hit row, the `Pick` carries its parent conversation's `conv_id`; `rank` is the conversation's
rank, not the hit's.

**This matters more than it looks.** The TUI has replaced the fzf script it was written against,
so it is now the only interactive source of harvested queries — a TUI that does not log makes
`6eb.21` go blind at exactly the moment the tool became good enough to use.

**The modal is destination × action, not one modal per verb.** `PendingAction` is already an
enum over `{Resume, Copy}` and the modal carries it through, so the same dialog serves both.
Copy emits `cd <dir> && <cmd>` (`tui/input.rs:141-150`); a VS Code destination emits a URI, a web
destination a URL. Same modal, different formatter. Sizing: `centered_rect(48, 8)`, driven by
`←/→`, `Tab`, `Enter`, `Esc`, plus a letter accelerator per destination.

---

## 7. Colour

fast-resume hardcodes 24-bit RGB throughout — `ACCENT rgb(224,150,70)`,
`SELECTED_BG rgb(68,52,34)`, `PANEL_BORDER rgb(70,80,95)`, `WARNING rgb(240,180,80)` — plus a
brand colour per agent in `config.rs`. Both choices are worth rejecting, for different reasons.

### Three layers, and only one of them is ours to pick

**Layer 1 — structure, from the terminal's indexed palette.** Selection, borders, dim text,
warning, accent. Use indexed colours (0–15), not RGB. The user has already themed their
terminal; inheriting that palette is what makes a TUI feel native instead of like it brought
its own skin, and it degrades sanely on 8-colour terminals. Hardcoded slate
`rgb(70,80,95)` borders are near-invisible on a light background, and `Color::DarkGray` for the
directory cell is unreadable in several common light themes. **Never set a background on the
full screen** — let the terminal's own background show through.

This is also why `me9.10` (theming) can stay P4: with layer 1 done properly, the user's existing
terminal theme *is* the theme, and a config-file colour system is a nicety rather than a fix.

**Layer 2 — source identity, as a discriminable categorical encoding, not brand colours.** Brand
colours are chosen for logos on white, and they collide in a terminal list. In fast-resume,
`cursor` and `grok` are both `rgb(255,255,255)`, `opencode` is `rgb(207,206,205)`, and `copilot`
is `rgb(156,163,175)` — four sources that are near-indistinguishable in the one column whose
entire job is telling sources apart. The text badge is doing all the work, which means the colour
is decorative while looking informative.

Assign from a small palette chosen for discriminability, stable per source id (sorted, so it is
deterministic — decision 7's instinct applied to rendering). Reserve red for errors:

| source | indexed |
| --- | --- |
| claude-code | yellow (3) |
| codex | cyan (6) |
| gemini-cli | blue (4) |
| chatgpt | green (2) |
| claude.ai | magenta (5) |
| copilot | bright yellow (11) |
| opencode | bright cyan (14) |
| cursor / antigravity | bright magenta (13) |

Eight stays inside the ~8-hue ceiling for categorical discriminability. Past that, colour stops
being an encoding and the badge is the only identifier — which is fine, because of the rule below.

**Layer 3 — match highlighting, deliberately the loudest thing on screen.** This is the answer
to "why is this row here," so it is the one place to spend a hardcoded high-contrast pair rather
than inherit a theme colour that might land invisible. Reverse video or an explicit bg/fg pair.

### Hard rules

- **Nothing is encoded in colour alone.** Age has a text label, source has a badge, selection
  has the `▸` pointer as well as the fill (fast-resume gets this right, `render.rs:614`).
  Colourblind users and 8-colour terminals lose scanning speed, never information.
- Respect `NO_COLOR`; do not emit truecolor without `COLORTERM`.
- **Test on a light background before shipping.** This is the failure mode every
  hardcoded-RGB TUI shares.

### Age colour: drop the ramp

`age_style` (`tui/text.rs:177-194`) is a continuous exponential heat map,
`t = 1 - exp(-0.0149 · hours)`. That reaches 90% saturation at **6.4 days** and flattens to
uniform grey beyond, so for a corpus spanning 2023-01 → 2026-07 the colour encodes only
"this week / not this week" while looking like a gradient. Either use three discrete bands
(today / this week / older) or leave the column uncoloured and spend the discriminability
budget on layers 2 and 3, which are the encodings that carry information.

---

## 8. Preview pane

fast-resume's preview is a parser for its own serialization, and that is the single largest
divergence available to us. `Session.content` is one flat `String` (`model.rs:15`); the adapter
flattens structured messages into it with sigil prefixes, and the preview re-derives structure by
parsing prose — split on `\n\n` for message boundaries (`preview.rs:15`), `starts_with("» ")` for
user, `starts_with("  ")` for assistant (`preview.rs:131-140`), then peel the sigil back off
(`:142-148`). The consequences are unrecoverable, because the structure was destroyed at flatten
time:

- A multi-paragraph user message becomes N messages. Paragraph 1 keeps its `» ` and renders as
  User; paragraphs 2..N fall through to `Other` and lose the role.
- Prose legitimately beginning with `» ` reads as a user turn.
- Indentation flips a message to Assistant.

We have `message(role, kind, seq, parent_id, thread_key, on_head_path, text)`. **Render from rows.**

```sql
SELECT id, role, kind, seq, text FROM message
WHERE conv_id = ? AND on_head_path = 1 ORDER BY seq
```

Covered by `idx_message_conv`. Read on selection change, not eagerly — fast-resume carries the
conversation body on every row struct and still clips at 6,000 chars (`preview.rs:29`); one
indexed query per selection is well inside budget and shows the whole conversation.

### The fold model

Every message renders to a block with two forms, and a fold state decides which is used. This
subsumes tool collapsing, reasoning collapsing and the outline mode below into one mechanism
rather than three special cases:

```rust
struct Block { msg_id, role, kind, on_path, collapsed: Vec<Line>, expanded: Vec<Line> }
enum Fold { Auto, Collapsed, Expanded }   // per-message override
enum Density { Full, Outline }            // pane-wide default
```

`Fold::Auto` resolves against `(density, kind)`; an explicit per-message override always wins.

| kind | Full | Outline |
| --- | --- | --- |
| `prose` | expanded | collapsed |
| `reasoning` | collapsed | collapsed |
| `tool_call` / `tool_result` | collapsed | collapsed |
| `tool_*` that matched `fts_tools` | **expanded** | collapsed |

Tool traffic is 91% of the corpus (decision 5), so collapsing it is what makes the pane readable
at all — a preview that renders everything verbatim is mostly file contents. Collapsed, not
hidden: *that a tool ran here* is part of recognising a conversation. The `fts_tools` exception is
how a tool-field hit becomes visible instead of mysterious.

**Revised 2026-08-04: that last row is the common path, not an exception** (`4ar.8`).
Message-level, 66–85% of hits land in tool traffic depending on the query, and 20–73% of matching
conversations match *only* there (§3). So under a query the row fires on most of the list rather
than occasionally, and two things follow. It is not the part to leave for later — a preview that
ships the first three rows and not the fourth renders blank at the match on the majority of
searches. And its cost is not an exception's: where forty tool calls matched, "expanded" is most
of the pane. Whether a matched tool expands wholly or only to its matching line, as `prose`
already does, is open (§11).

**Collapsed forms:**

| kind | collapsed rendering |
| --- | --- |
| `prose` | role sigil + the **matching** line, else the first non-empty line |
| `reasoning` | `⋯ reasoning · 34 lines` |
| `tool_call` | `⚙ Read(schema.rs)` — name plus primary argument |
| `tool_result` | **omitted** — except a failure, which keeps its error text |

**Revised 2026-07-30, from first use.** `tool_result` was specced as a collapsed one-liner
alongside `tool_call`; omit it instead. The result is a blob whose existence the call already
implies, so a line saying `↳ 1.2 KB` spends a row to repeat what `⚙ Read(schema.rs)` just said.
The failure case survives the change: "the tool broke here" is recognition information, and it
stays legible in the error colour.

### Outline mode

`Density::Outline` collapses every turn, including prose, to one line — two only for the focused
message. At a 36-line preview inner height that shows 36 messages, which is the entire
conversation for most of this corpus (the sampled screenshot ranged 1–88 turns per row).

**Revised 2026-08-04 (`4ar.8`).** "The entire conversation for most of this corpus" is true by
count and misleading in use. ChatGPT averages 7.2 messages and is two thirds of the corpus, but
claude-code averages 132.0, codex 182.1, and the largest conversation is 2,553 — so a 36-line
outline shows a fifth of a typical Codex session, which is the archetype the mode exists for.
Outline is a scrollable map of a long conversation, not a one-screen table of contents; specced
as the latter it would be sized against the conversations that need it least.

Two things make it worth more than a density knob:

- **Because collapsed prose prefers the matching line, outline mode is a match map.** You see
  every place the query hit, across the whole conversation, in conversation order. That is
  complementary to the hit rows in §3 rather than redundant with them: hit rows are the top 3
  by rank, in the results list; the outline is all of them, in sequence, in the preview.
- **It changes the navigation unit.** In `Full`, scrolling moves lines. In `Outline`, it moves
  messages. The focused message is what expands to two lines and what the fold override applies
  to.

### Keymap constraint

**The search box consumes every unmodified keystroke**, so each command costs a modifier or a
special key. fast-resume's whole keymap obeys this — `Ctrl+Y`, `Ctrl+P`, `Tab`, `Esc`, arrows,
`PageUp`/`PageDown`, `Alt+±` (`tui/input.rs:16-58`). The budget is small; do not spend keys
casually. Recommended: `Ctrl+O` cycles density, and the focused-message fold override gets a
second modified key. Everything else stays on the existing five.

### Highlighting and anchoring

Both are currently wrong in our own code — see `6eb.20`. Two rules for the preview:

- **Highlight from the same matcher that ranked.** Contentless fts5 rules out the SQL `snippet()`
  and `highlight()` functions, so hand-rolling is forced; it must still tokenize and Porter-stem
  the way the index does, or a stemmed hit highlights nothing. `6eb.10` fuzzy-on-failure would
  make a substring highlighter useless outright.
- **Anchor on the best hit, not the earliest.** The ranking already scores per message before
  grouping, so the best-hit message id is known at hydrate time. Scroll there on selection.

### Off-path branches

`on_head_path` and `parent_id` let the preview show that a message was **edited away** — the
abandoned branch dimmed or struck through beside the surviving one. `Query.include_off_path`
already exists, and nothing in `PRIOR-ART.md` can do this, because nothing else models the DAG
(decision 4). For ChatGPT edit-branches and Codex forks it is the difference between "I cannot
find what I asked" and "here is the version you replaced."

Off by default, behind a key. It is specced now because it constrains the block model: blocks
carry provenance — which message, on-path or not — not just text.

### Build once, not per frame

`draw_preview` calls `render_preview_lines(session, &state.query)` on every render
(`render.rs:769`), so the snippet search, line construction and syntax highlighting re-run on
every scroll tick and keystroke, then get `.take(220)`'d. Cache the blocks in state, keyed on
`(conv_id, query, density, fold overrides)`. Same instinct as `6eb.19`, one layer down.

### No hand-rolled syntax highlighting

fast-resume spends ~150 lines of `preview.rs` on it — `code_word_style`, `is_code_keyword`,
`hash_comments`, `string_end`, `code_spans` — and it is barely language-aware: `code_fence_language`
captures the language but only `hash_comments` consumes it, so the keyword list is a flat union
across languages and `def` highlights inside JavaScript. For a search preview the value is near
zero; you are confirming identity, not reading code. Render fenced blocks dim, **keep the match
highlight applied inside them**, and stop. If it ever matters, use a real grammar rather than
growing a keyword list.

---

## 9. Lifted, rejected, and already ours

| fast-resume behaviour | verdict |
| --- | --- |
| Five-band layout, pane reflow at 116 | **lift** |
| Column shedding with a Title floor | **lift**, shed Msgs before Directory |
| Filter-bar degradation ladder + active-facet anchoring | **lift**, with its tests |
| Query DSL, incl. dual negation and dir-substring | **lift** (`6eb.11`) |
| Invalid-filter strikethrough in the input | **lift**, cheap and self-teaching |
| Ghost-text completion on `Tab` | **lift** |
| Command computed at action time via per-source trait | **lift** — independent confirmation of `me9.3` |
| Capability probe + skip-modal-when-single | **lift**, generalised to destinations |
| Selection preserved by `(source, id)` across re-search | **defer** — needed when `6eb.16` lands, not before |
| Error keeps prior results on screen, sets status | **already ours** (`me9.5`) |
| Generation-numbered searches | **already ours, deliberately not built** (`me9.4` closed; sync at 6 ms) |
| Relative age in list, absolute in preview | **lift** |
| Filter tokens stripped before deriving highlight terms | **lift** (`preview.rs:462-467`) — we need it for `6eb.20` |
| Raw titles | **reject** — 24% of visible rows were markup |
| Preview parses a flattened content blob | **reject** — render from `message` rows (§8) |
| Substring match anchoring against a stemmed index | **reject** — ours has the same bug, `6eb.20` |
| Earliest match as the snippet anchor | **reject** — anchor on best hit (§8) |
| Preview rebuilt every frame | **reject** — cache per `(conv, query, fold)` |
| Hand-rolled syntax highlighting | **reject** — ~150 lines, barely language-aware |
| Tail-elided paths | **reject** — middle-elide, preserve leaf |
| Hardcoded RGB, brand-colour agents | **reject** — see §7 |
| Continuous age heat map | **reject** — saturates at 6.4 days |
| Terminal-graphics icons via auto-detected protocol | **reject** — see below |
| Zero-count sources hidden from the facet bar | **reject** — see §5 |

**On the icons.** The agent glyphs are PNG logos rendered through terminal graphics protocols
(`tui/images.rs`) at 2×1 cells in rows and 8×4 in the preview, via `ratatui-image` with
`Picker::from_query_stdio()` auto-detection. On this machine detection claimed Kitty support the
terminal did not honour, and it degraded to visible garbage rather than to the text badge — the
fallback exists (`agent_badge`, `label_x = 2`) but never fires, because detection lied. **Make
graphics opt-in via config, never auto-detected.** A wrong "yes" is unrecoverable on screen; a
wrong "no" costs an icon.

---

## 10. Scope check

`me9.1` estimates ~300–400 lines. fast-resume's TUI is ~4,100, of which roughly 550 is image
support and hand-rolled preview syntax highlighting we are not obliged to build. The acceptance
criteria as written — responsive column plan, filter degradation ladder, destination modal,
highlighted preview, flattened tree cursor — is closer to **1,200–1,800 lines**. The 300–400
figure describes a list plus a search box, which is a legitimate first milestone but is not the
stated AC. Worth splitting rather than discovering mid-build.

## 11. Open

- Which key carries the per-message fold override. `Ctrl+O` is spoken for by density; the
  unmodified keyspace belongs to the search box (§8).
- Whether a `tool_*` that matched `fts_tools` expands wholly or only to its matching line. §8
  says wholly, specced when that row still looked like an exception; at 66–85% of hits it is the
  common path and the whole-call form is most of the pane.
- Whether hit rows are reachable by `Tab`/`j`-`k` only, or also collapsible per conversation.
- Whether outline mode renders forks as indented branches, or stays linear over the head path
  and leaves branch display to the off-path toggle.
- Whether the results list needs a scroll offset independent of the selection. Today the
  window is derived from the cursor, so a wheel over the results moves the selection rather
  than scrolling past it. That is the more useful behaviour when the selection drives the
  preview, but it means you cannot look ahead without committing.
