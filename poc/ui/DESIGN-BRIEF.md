# chat-search — design brief

Paste this alongside a screenshot of `poc/ui/index.html`.

> **Four directions already answer this brief.** `poc/ui/directions.html` shows
> `terminal` (the incumbent, and the control), `paper`, `blueprint` and `ink` on the row
> and the ribbon at real size in both themes, with a measured table of what each costs;
> `poc/ui/index.html?dir=paper` puts one of them on the whole prototype. They are worked
> answers rather than the answer — the brief below is still the ask, and a fifth
> direction that ignores all four is a better outcome than a refinement of one.

**The visual language here has never had a designer look at it.** It was built by
working outward from measurements of the data, which is why the information design is
well-argued and the *look* is not. Treat the current appearance as one working answer,
not a spec. The parts worth preserving are the **jobs each mark has to do** — those came
from measuring a real archive of 3,059 conversations and 184,099 messages. How those
jobs get done visually is wide open, and a confident reimagining is the point of handing
this over.

---

## What this is

A search tool over one person's entire history of AI conversations — ChatGPT, Claude,
Claude Code, Codex, Gemini — collected into one local archive. It answers *"where did I
work that out?"* and *"what did I do in that project?"*

It is a power tool for a single expert user, used daily, keyboard-first, on a large
screen. Density matters because the archive is large and the value is recognising the
row you want among many. Today it reads austere and terminal-ish; that is a starting
point, not a requirement. It could be warmer, more crafted, more editorial — the
constraint is that it stay **dense and calm**, not that it stay severe.

The corpus spans a 350× range in one list: two thirds are short conversational threads
(ChatGPT averages 7.2 messages), one third long agentic sessions (Codex averages 182,
the largest is 2,553). Whatever the design becomes has to survive that spread.

---

## What you are looking at

**Title bar** — window controls, product name, two view tabs (`Search`, `Library`), and
three toggles on the right.

**Search bar** — the query line showing every active filter as a removable token, a
`group` control, and a result count. The query line is the single source of truth for
filtering; clicking a facet anywhere rewrites it. A filter that narrows the list without
appearing here would be a bug.

**Left rail** — facets, ordered by how much of the corpus each one covers: When (100%),
Sources (100%), Projects (34%), Topics (76%).

**Centre list** — the result rows. Everything else exists to serve this.

**Right drawer** — the selected conversation: title, provenance, topics, a summary of
what the conversation *did*, controls for how much detail to show, then the transcript
with a minimap.

**Bottom drawer** — a timeline of whatever survives the current filters, with a scrubber.

---

## The row

The unit of the product. Three lines, four when there is a query:

```
[icon] claude-opus-5   91 msgs  −30 +42  ▊▎▍▎▏▍▎    ⟨gutter⟩   …/chat-search   today
Can we start to think about what a native app might look like?
Web frontend  ·  macOS and hardware
⚙ Bash(cargo test) › "…the matching fragment…"          ← only when there is a query
```

Line 1 is currently a strict grid so the eye can run down a column, split into two
clusters by a gutter — *who and how big* on the left, *where and when* on the right. It
carries: source, model, message count, lines changed, the ribbon, path, and age.

Two details with reasons behind them, in case they're useful: the path is
**middle**-elided rather than tail-elided, because the leaf is the identifying part
(`…/chat-search`, never `/Users/…/chat-sea`); and the "lines changed" cell is empty on
55% of rows, so it needs to disappear rather than sit there as a gap.

---

## The ribbon

The one genuinely unusual mark, and the thing most worth designing well. It is a density
strip of an entire conversation on a message-index axis, and it answers: *was my hit in
something I said, or buried forty messages into a tool run?*

It currently encodes six things on one axis:

- **kind of message** — you / agent prose / reasoning / tool traffic, run-length encoded
- **match positions** — the only query-dependent layer; absent with no query
- **a pause of more than four hours**
- **a compaction** — the point where the conversation's earlier half stopped being
  verbatim, so the assistant past it knows different things
- **a failed tool call**
- **the agent asking you a question**

plus **length**, as the drawn width: log-scaled, so a 10-message conversation draws
short and a 2,553-message one draws long.

That is a lot for one 200×9px mark, and it is the strongest candidate for reinvention —
a different shape, a different arrangement, more than one row of it, something else
entirely. What has to survive is the *reading*: at a glance, is this conversation mostly
me or mostly the machine, where are my hits, and did anything go wrong.

**One finding worth carrying forward.** The kinds were originally separated by hue
alone. Measured, two of the four sat 1.12:1 apart in luminance — the same colour to the
eye — because hue is the channel that degrades fastest at 2px. They are now on an even
luminance ramp (~1.8× per step) with hue as the redundant second channel. The specific
colours are not sacred; the lesson is: at this size, **luminance does the work**.

---

## Jobs the marks have to do

Stated without reference to how they currently look, so a redesign can answer them
differently:

1. **Tell four message kinds apart at ~2px, in both themes.** Tool traffic is 66–85% of
   the volume, so it has to be the quietest or the mark becomes a wall; a user turn is
   rare and structural, so it has to read as a separator.
2. **Make a search match findable** without it competing with everything else.
3. **Make a failure and an unanswered question spottable** in a long conversation.
4. **Distinguish two kinds of discontinuity** — a pause (you went away) from a
   compaction (the context was cut). They are not the same event.
5. **Convey scale** across 4 to 2,553 messages.
6. **Identify the source tool** without relying on colour alone — today that is shape,
   hue and a word, of which the row can afford two.
7. **Keep counts, dates and paths comparable down the column.** This is why everything
   tabular is monospaced; a proportional face would need a different mechanism.
8. **Keep the drawer readable as prose.** It is the one place the product stops being a
   list, and it has had the least attention of anything here.

---

## How it looks today

Reported, not prescribed:

- **Type** — system sans for UI, system mono for anything tabular, five sizes
  (13 / 12.5 / 11.5 / 10 / 9px). The faces were never chosen; they are defaults.
- **Colour** — teal and slate, two full themes. Three meanings are currently reserved
  and used consistently: amber for a match, red for a failure, teal for selection. The
  *consistency* is worth keeping; the hues are not.
- **Shape** — small radii (mostly 4–6px), hairline borders at low contrast, no shadows
  except one floating overlay, no cards or elevation in the list.
- **Layout** — at a 1560px window: rail 252px, list 706px, drawer 602px (about a
  78-character measure), ribbon 200px, row 66px. These are current facts. A different
  split, a different information hierarchy, or a different number of panes are all fair
  game.

---

## Where to push

- **The overall register.** Austere and terminal-ish today. Warmer, more crafted, more
  editorial are all available.
- **Type pairing.** Nothing here was chosen. A better mono alone would change the feel
  of the whole product.
- **Spacing and vertical rhythm.** Currently ad hoc.
- **Rules, borders and radii** — where a hairline earns its place versus where space
  alone would separate.
- **The ribbon**, per above — the highest-leverage thing on the page.
- **The source marks**, currently monochrome masks tinted from the palette.
- **The drawer's reading experience.**
- **Empty states and microcopy voice.** There are only a few and they are all recent.
- **The row itself.** Nine cells on one line is a lot; the current grid is a solution to
  a width problem, not a considered layout.

## What would actually break it

Short list, and all functional rather than aesthetic:

- Losing rows-per-screen. Anything that adds height has to earn it.
- Losing the ability to compare counts, dates and paths down the column.
- Making the four message kinds indistinguishable at small size — the specific failure
  that was measured and fixed once already.
- Making the quiet text tier illegible; it carries dates, counts and labels, not
  decoration, and currently sits at 4.6:1 in both themes. Note *both grounds*: it lands
  on the page and on the drawer, `--panel` is the darker of the two in the light theme,
  and checking only the page is how it stayed at 4.23:1 in the drawer through one fix
  already.
- Encoding anything in colour alone.
