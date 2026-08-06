<!--
Prompts, not a checklist — delete the headings you do not need and write prose.
docs/PULL-REQUESTS.md has the long form, and the merged history is the better guide:
read #36 or #39 before your first one.

Agents: `gh pr create --body` does NOT read this file. Open docs/PULL-REQUESTS.md and
follow it, then pass the finished body with --body-file.
-->

<!-- What was wrong. One sentence, for a reader who has never seen the bead. -->

<!-- What shipped, at the density the diff cannot supply. -->

## The one decision this needed
<!-- Delete if there wasn't one. If there was, this is the section reviewers read first. -->

## Evidence
<!--
Behaviour change  → the gate: `cargo test --workspace`
Performance       → the numbers AND how they were taken (interleave the arms; see #39)
UI                → frames, below
-->

## What it looks like
<!--
Delete unless this changes something drawn.

TUI  — `cs tui --shot --out /tmp/tui` writes .txt, .ans and .svg per frame.
       Small change → paste the .txt in a fence. Larger, or the styling IS the change →
       the .svg (rsvg-convert it to PNG for here). Settle colour questions on the .ans.
App  — `scripts/shot.sh --upload` and link the PNGs
Both — `scripts/shot.sh` takes the before side too, by building the merge-base

Before/after when the change alters something already on screen; after-only when it adds a
surface that did not exist. If you could not take a frame, say so in a line — never describe
a screen you did not render.
-->

## What I left out
<!-- Limitations, follow-up beads filed, the branch of the acceptance criteria not taken. -->

Closes <!-- bead-id -->.
