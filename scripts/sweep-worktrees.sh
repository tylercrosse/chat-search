#!/usr/bin/env bash
# Remove the worker worktrees whose branch already landed, and nothing else.
#
# `bd-dispatch` gives every parallel worker its own worktree under `.claude/worktrees/`, and
# each one builds its own `target/` — about 1.5 GB per bead. `/bd-review` merges the branch and
# stops there, so the worktree survives its own purpose and the disk cost is permanent. On
# 2026-08-05 that reached zero bytes free mid-build and `swift build` failed with "you can't
# save output-file-map.json because the volume is out of space" (chat-search-64i).
#
# Deleting merged worktrees is easy to get *nearly* right, which is the dangerous part. Three
# gates, and the third is the one that bites:
#
#   merged        — HEAD is an ancestor of the base ref, so the commits are provably in main.
#   no work       — nothing uncommitted. Untracked `__pycache__`/`.DS_Store` do not count as
#                   work; they are the only reason `git worktree remove` refuses on an
#                   otherwise-dead tree, and reaching for --force to get past them would also
#                   discard real edits.
#   no session    — an agent may be *standing in* a merged, clean worktree. Matching process
#                   command lines is not enough: a background worker's argv often does not name
#                   its own worktree, only its cwd does. The first draft of this script matched
#                   argv alone and proposed deleting two worktrees with live agents in them.
#                   `lsof -a -d cwd` is what makes this safe.
#
# The name is anchored for the same reason. Unanchored, `me9-8-2` matches `me9-8-22`'s session
# and each id inherits the liveness of every longer id it prefixes — which, with ids shaped like
# this project's, is most of them.
#
# Liveness is a point-in-time read: a worker starting between the check and the remove would be
# missed. That is why this is a command somebody runs after reviewing, not a hook on merge.
#
#   scripts/sweep-worktrees.sh            # say what would go, touch nothing
#   scripts/sweep-worktrees.sh --apply    # actually remove them
#   BASE=develop scripts/sweep-worktrees.sh --apply   # merged into something other than main
set -uo pipefail

APPLY=0
[ "${1:-}" = "--apply" ] && APPLY=1
BASE="${BASE:-main}"

cd "$(git rev-parse --show-toplevel)" || exit 1

# Untracked paths that are noise rather than work.
JUNK='__pycache__|\.DS_Store'
rc=0

while read -r wt; do
    # Only ever consider the dispatcher's own directory. A worktree somebody made by hand
    # elsewhere is not this script's business.
    case "$wt" in
        *"/.claude/worktrees/"*) ;;
        *) continue ;;
    esac
    name=$(basename "$wt")

    head=$(git -C "$wt" rev-parse HEAD 2>/dev/null) || {
        echo "skip  $name — cannot read HEAD"
        continue
    }

    git merge-base --is-ancestor "$head" "$BASE" 2>/dev/null || {
        echo "keep  $name — not merged into $BASE"
        continue
    }

    work=$(git -C "$wt" status --porcelain 2>/dev/null | grep -vE "$JUNK" | wc -l | tr -d ' ')
    [ "$work" = "0" ] || {
        echo "keep  $name — $work uncommitted path(s)"
        continue
    }

    live=$( { pgrep -fl "worktrees[-/]${name}(/|\$|[[:space:]])" 2>/dev/null
              lsof -a -d cwd -Fn 2>/dev/null | grep -x "n${wt}"; } | wc -l | tr -d ' ')
    [ "$live" = "0" ] || {
        echo "keep  $name — live session"
        continue
    }

    size=$(du -sh "$wt" 2>/dev/null | cut -f1)
    if [ "$APPLY" = "1" ]; then
        find "$wt" -type d -name __pycache__ -prune -exec rm -rf {} + 2>/dev/null
        if git worktree remove "$wt" 2>/dev/null; then
            echo "REMOVED $name ($size)"
        else
            echo "FAILED  $name — left in place"
            rc=1
        fi
    else
        echo "would remove  $name ($size)"
    fi
done < <(git worktree list --porcelain | awk '/^worktree /{print $2}')

if [ "$APPLY" = "1" ]; then
    git worktree prune
fi

exit $rc
