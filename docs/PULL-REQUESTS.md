# Pull requests

What a pull request here has to carry, and — for the two surfaces that draw something — how to
photograph it rather than describe it.

This is the one place the rule lives. `.github/PULL_REQUEST_TEMPLATE.md` is the short form GitHub
prefills; the `bd-worker` contract points here rather than restating it. Anything that disagrees
with this file is stale.

## The body

The history already sets the shape — read three merged PRs before writing your first. They lead
with the problem rather than the change, they name the one decision that was actually hard, and
they say what was left undone. None of them is a checklist, and a checklist is not what to
produce instead.

Four things, in whatever order the change wants:

**What was wrong.** Assume the reader has never seen the bead. `#36` opens with "this executable
creates `NSApplication` by hand and never built an `NSMenu`, so there was no app menu to hang
`Settings…` on" — one sentence, and the rest of the PR follows from it.

**What shipped.** The change itself, at the density the diff cannot supply.

**The evidence.** For a behaviour change, the gate. For a performance change, the numbers and the
method that took them — `#39` reports medians over ten *alternating* rebuilds precisely because
the first attempt measured the machine warming up rather than the schema. For a UI change, the
pictures; see below.

**What you left out.** Limitations, follow-ups filed, the branch of the acceptance criteria you
did not take. A PR that claims to be complete and is not costs more than one that says where it
stopped.

Close with `Closes <bead-id>.` No AI attribution, no `Co-Authored-By`, and no "Generated with"
footer — the same rule the commits are under.

## Showing a UI change

There are two surfaces and they photograph differently. Both do it themselves; neither needs a
screen recording permission, a foreground window, or a human holding the machine.

### The TUI — three renderings of the same buffer

```bash
cs tui --shot --db ~/.chat-archive/index.db --out /tmp/tui --size 140x40
```

Writes `tui-{rest,no-preview,expanded,typing}` in three formats each — the four states that render
differently rather than merely taller. `typing` is the frame between a keystroke and the keyboard
going quiet, which since `j1n` is a state of its own: the preview read is deferred, so the pane
says so rather than drawing whatever ranked first. It is taken by replaying the query's last
keystroke, which is the only way to catch a window a quarter of a second wide. **Which format to
reach for depends on the size of the change, and getting this wrong is how a review misses
something.**

| | what it is | use it for |
| --- | --- | --- |
| `.txt` | characters, no styling | small changes: what moved, wrapped or truncated. Diffs line by line, pastes into a fence, needs no hosting |
| `.ans` | the same buffer with SGR escape codes | the truth. `cat` it and your terminal resolves it exactly as the running app would, because it is the same bytes |
| `.svg` | the ANSI resolved against a stated palette | larger changes, and anything a reviewer has to *see*. Renders in a browser; `rsvg-convert` turns it into a PNG for a PR body |

A plain text frame answers "what moved" and nothing else. That is most of what a small change
needs and almost none of what a large one does — if the styling *is* the change, a monochrome
frame that says nothing about it still reads as though the render was checked. Say which format
you looked at.

**The SVG is *a* rendering, not *the* rendering.** `theme.rs` styles in named ANSI colours —
`Color::LightBlue`, `Modifier::DIM` — which resolve against whatever palette the reader's terminal
carries. The Swift app has concrete tokens and one true set of pixels; the TUI does not. So the
SVG names the palette it used (xterm's 16-colour defaults) and a disagreement about a *colour* is
settled in a terminal against the `.ans`, never in the picture. `DIM` in particular has no SVG
equivalent and is approximated with opacity.

This replaces hand-typing an approximation of the screen. Every TUI PR before this one carried a
hand-typed frame, which is an assertion about the render made by the party least able to check it,
and it goes stale the first time the layout moves underneath it.

### The macOS app — PNGs

```bash
apps/macos/.build/release/chat-search --shot \
  --db ~/.chat-archive/index.db --bin target/release/cs \
  --out /tmp/app.png --size 1200x800
```

Writes fifteen frames — the reader, one per fidelity preset, scrolled, scrubbed, typed-on, each of
the three grouping axes open and folded, and the timeline drawer shut — plus minimap geometry,
main-thread frame lag, and the three claims about the fidelity model that no frame makes.
`--settings`, `--theme`, `--folded`, `--group` and `--longest` reach the states a script otherwise
cannot.

`Measure.capture` draws the view hierarchy through `cacheDisplay`, so there is no window server
involved. That is why it works from a background session with nothing granted to it, and it is the
reason an agent can be asked for these at all.

**What it cannot show is the window's own chrome, and that is now a smaller gap than it was.** The
capture is `cacheDisplay` on `window.contentView`, and since `chat-search-me9.8.30` gave the window
`.fullSizeContentView` that view is the whole window — so the titlebar band, which the app draws
itself, is in every frame. The traffic lights still are not: they live in the window frame above
the content view. A change to *those* — their inset, or how a direction reads behind them — has to
be photographed with `screencapture -l` against a real window, which is a thing this repository has
scripts for only under `.shots/me9.8.28-probe/` and untracked. Say so in the body rather than
describing a picture you did not take.

### Both sides, and getting them into a body

```bash
scripts/shot.sh                 # HEAD against its merge-base with main
scripts/shot.sh --tui           # skip the Swift build, which is most of the time
scripts/shot.sh --after-only    # a new surface, with nothing to compare against
scripts/shot.sh --upload        # PNGs to somewhere a PR body can reach
```

It builds the base revision in a throwaway worktree and takes the same frames from it, then
reports what moved. The base build is the whole cost — a cold `swift build -c release` is minutes,
which is why this is run deliberately and not by the gate.

**When a before is needed:** when the change alters something already on screen. When it adds a
surface that did not exist, `--after-only` is the honest answer and a fabricated "before" is worse
than none.

**Text frames need no hosting** — paste them. PNGs cannot be embedded by `gh pr create --body`, so
`--upload` puts them on a GitHub release tagged `shots`, which is an asset store rather than a
release of anything. Committing PNGs into the repository is the alternative, and it is worse: they
are generated, they are large, and they would sit in the history forever.

## After the merge

A merged branch leaves its worktree behind, and the worktree keeps its `target/` — about 1.5 GB
per bead, for commits that are already in `main`. Nothing reclaims it on its own, so it is worth
one command per review round:

```bash
git fetch                             # the sweep does not, and merged means merged on the remote
scripts/sweep-worktrees.sh            # what would go
scripts/sweep-worktrees.sh --apply    # remove it
```

It removes worktrees that are merged, hold nothing uncommitted, and have no agent working in
them. Read the header in that file before changing it — the liveness check is subtler than it
looks, and getting it wrong deletes a running worker's tree out from under it.

Two things it reports rather than does, both of which are the interesting output:

**Merged is judged against `main` and `origin/main` together.** A branch that landed as a pull
request is in `origin/main` immediately and in your local `main` only once you pull, so a sweep
run against local `main` alone will keep worktrees whose commits are demonstrably in the remote.
The `git fetch` above is the part the script deliberately leaves to you — a network call that
changes which directories get deleted should be somebody's decision, not a side effect.

**It sweeps a fixed list of roots** — `.claude/worktrees/`, `.codex/worktrees/`, and a sibling
`.chat-search-worktrees/` — and prints any merged, idle worktree outside all of them as `outside`,
with its size, instead of skipping it silently. Those lines are worktrees somebody made by hand
somewhere else; the script will not delete them, and naming them is how the disk stays visible.
If a run shows the same `outside` path every round, that is a new root worth adding to `ROOTS`.

This is deliberately a command and not a merge hook. Whether a worktree is in use can only be
read at the moment you look, so the safe time to sweep is when somebody is already reviewing.

## For agents

Everything above applies to whatever is opening the PR — `bd-worker`, an ad-hoc background
session, or a person. Three things that are specifically about working unattended:

**Take the shots before closing the bead**, while the branch is still checked out and built. A
worker that closes, then discovers it needs a picture, has to rebuild to get one.

**Never describe a screen you did not render.** If `--shot` failed, or the corpus was unavailable,
or the change is in a surface neither harness reaches, say that in the body in one line. An
invented description of a screen is the worst failure mode available here, because it is
indistinguishable from a real one until somebody merges it.

**A missing picture is a line in the body, not a blocker.** Do not file a beads gate over it. Say
which frames you could not take and why, and let the reviewer decide whether it matters.
