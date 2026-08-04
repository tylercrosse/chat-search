# Interface prototype

> **Design log:** [`NOTES.md`](./NOTES.md) — decisions and their rationale, every
> measurement taken, what was got wrong and reversed, and the open questions. That file
> is the durable asset; this code is not.

A clickable mockup of a native surface for `chat-search`. The app fills the window; nothing
scrolls except the panes that would scroll in the real thing.

```bash
open poc/ui/index.html          # no server, no build, no dependencies
open poc/ui/gallery.html        # every component, every state, both themes
open poc/ui/directions.html     # four visual directions, measured against each other
open "poc/ui/index.html?dir=paper"   # the whole prototype in one of them
```

Sits beside `poc/rust` and `poc/ts` for the same reason they do: an instrument for answering a
design question, not part of the product. `poc/` is excluded from the cargo workspace.

## Three views

**Search** — filter rail, results, preview. Click a source or a date to watch it rewrite the query
text; there is no second filter state anywhere in the code, which is the rule `TUI-DESIGN.md` §5
caught fast-resume breaking (it needed six reconciliation methods to hold two sources of truth
together).

Rows are multi-line: metadata plus a **fused ribbon**, then the title full width, then — only when
there is a query — the true best match framed with its provenance (`⚙ Bash(cargo test) › "…"`).
Browsing stays two lines, so the third is spent only when it buys something.

The ribbon is one graphic carrying two channels over one axis. The body is **shape** and is
query-independent: consecutive same-kind messages are run-length encoded into blocks, so a
30-message tool run reads as one block and a back-and-forth reads as rapid alternation. Over it
sit **match ticks**, which are query-dependent and simply vanish when there is no query. Notches
mark pauses over four hours; dots mark where the agent asked you something; red marks a failed
tool. Because 66–85% of real hits land inside tool traffic, the useful read is the compound one:
*was my hit in a steer, or buried in a 40-message agentic run?*

The preview is ~602px (78ch, a real reading measure) and has **three zoom stops** over the same
data — segments, outline, full. Segments collapses a steer plus everything it caused into one
summary line (`→ 34 calls · 2 failed · asked you 1×`), which turns a 211-message conversation into
about six lines. Message types are told apart by a **gutter spine** using the same vocabulary as
the ribbon, never by text contrast: `me9.1.1` records that an assistant answer is often the thing
being looked for, so neither role may be visually demoted.

**Collections** — the categorising surface. A collection is stored as a rule,
`matches(query) ∪ pinned − excluded`, and every card shows that arithmetic rather than just a
total, because you will want to know why something is in there. Storing the rule instead of the
membership is what lets a collection survive a reindex. Below them sit cluster **proposals**,
visibly provisional: accept one and it freezes into a collection you own, dismiss it and it costs
you nothing. A cluster is a proposal; a collection is a decision.

**Sittings** — days grouped into `ended_at` windows of about two hours. No embeddings, no model,
no `inference.db` — a self-join on a column that already exists. It reaches the 69% of the corpus
that is ChatGPT and carries no `cwd`, which is exactly where project-shaped filters cannot go.

## Notes

Press **N**, or click *notes*, to drop annotations onto the live interface. Eight on Search, four
on Collections, two on Sittings. They are set in a serif because they are commentary rather than
product, and they never intercept a click meant for the interface underneath.

The fuller argument — which of the terminal's decisions survive the move to a window, and why the
obvious minimap fails on this corpus — lives in the two published design artifacts, not here.

## What is real and what is not

Conversation titles, paths and timings are invented, drawn from this project's own vocabulary.
Every figure cited as a measurement is real, from [`docs/DECISIONS.md`](../../docs/DECISIONS.md)
and [`docs/TUI-DESIGN.md`](../../docs/TUI-DESIGN.md): 2,963 conversations, 172k messages, 91% tool
traffic, the 1.4–6.4 ms query, the 250 ms count settle, the 293 MB index.

Interactions are clickable over canned data. The query box is display-only — but facet clicks do
rewrite it, because that is the behaviour worth demonstrating.

It renders in the system UI face at native metrics on purpose. Set a native app mock in a display
face and it stops telling the truth about density, which is the only reason it exists.

## Visual directions

The information design here was argued from measurements; the *look* never was. Four directions
answer that — `terminal` (the incumbent, and the control), `paper`, `blueprint` and `ink` — and
`directions.html` shows each on the row and the ribbon at real size in both themes, beside a table
of what it costs. The table is measured off the rendered page rather than read from the tokens,
because a row's height is a line box's opinion and not a sum of the padding you asked for.

Three things are fenced and all four directions hold them: the four message kinds stay on an even
~1.8× luminance ramp against the ribbon track, the quiet text tier clears 4.5:1 on **both** grounds
it lands on, and rows-per-screen does not drop. `python3 poc/ui/palette.py --verify` re-measures the
stylesheets and exits non-zero if one stops holding.

## Layout

| file | holds |
| --- | --- |
| `index.html` | the window shell |
| `DESIGN-BRIEF.md` | what the mockup is and what its marks must encode — for handing to a design tool with a screenshot |
| `gallery.html` · `gallery.js` · `gallery.css` | the component gallery — renders the app's own functions, so it cannot drift from what ships |
| `directions.html` · `directions.js` | four visual directions on the row and the ribbon at real size, in both themes, with what each costs |
| `directions.css` | those four as token sets — palette, faces, sizes, rhythm, radii; nothing structural |
| `palette.py` | solves each direction's luminance ramp, and re-measures the stylesheets with `--verify` |
| `styles.css` | tokens, the three views, the annotation layer |
| `data.js` | mock conversations, collections, sittings, annotation copy |
| `app.js` | rendering, the collection rule evaluator, interaction |

## Measured, not assumed

The row and preview design was settled against the live index rather than by argument. What the
corpus actually says, as of 2026-08-03:

| | |
| --- | --- |
| archetype vs source | ~87% of conversations are predicted by their agent badge alone |
| avg messages | chatgpt 7.2 · claude-code 132.0 · codex 182.1 |
| prose fraction | chatgpt 0.88 · claude-code 0.30 · codex 0.23 |
| where hits land | 66–85% in tool traffic, not conversation |
| segments per conversation | 2–8 typical; 20.4 msgs/steer claude-code, 37.0 codex |
| pauses over 4h | 1,088 corpus-wide, roughly one per agent conversation |
| agent asks you | 4.6% of assistant prose, ~1.4 per conversation |
| title == opening prose | 99.4% of codex, 11% of claude-code, 0.1% of chatgpt |

Two of these contradict assumptions in `TUI-DESIGN.md` §3; that divergence is filed as
`chat-search-4ar.8` rather than left to drift.

## Status

Nothing here is decided and no product code has been written against it. The prerequisite it keeps
running into is real: there is no `cs show` subcommand and `cs-core` exports no function returning
a conversation's messages, so every pane on the right-hand side of Search is currently
unbuildable. The same gap blocks a VS Code client (`chat-search-me9.9`).
