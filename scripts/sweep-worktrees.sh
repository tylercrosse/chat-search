#!/usr/bin/env bash
# Remove the worker worktrees whose branch already landed, and nothing else.
#
# A parallel worker gets its own worktree, and each one builds its own `target/` — about 1.5 GB
# per bead. `/bd-review` merges the branch and stops there, so the worktree survives its own
# purpose and the disk cost is permanent. On 2026-08-05 that reached zero bytes free mid-build
# and `swift build` failed with "you can't save output-file-map.json because the volume is out
# of space" (chat-search-64i).
#
# Deleting merged worktrees is easy to get *nearly* right, which is the dangerous part. Three
# gates, and the third is the one that bites:
#
#   merged        — HEAD is an ancestor of a base ref, so the commits are provably in main.
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
# Two ways the first version of this script reported "nothing to reclaim" while the disk filled,
# both of which are silence rather than error, and so worth stating plainly:
#
#   the base was local `main`, which is behind whatever GitHub merged. A branch merged through a
#   pull request is in `origin/main` the moment it lands and in local `main` only after somebody
#   pulls; with local `main` 30 commits behind, nine already-merged worktrees read as "not merged"
#   and stayed. So the default base is every main-ish ref that resolves, and a tree is dead if its
#   HEAD is an ancestor of *any* of them — being contained in the remote's main is as good a proof
#   of merged as being contained in the local one.
#
#   only `.claude/worktrees/` was considered, and every other tree was skipped without a word.
#   The dispatchers have since grown two more homes; 7.7 GB of provably-merged worktrees sat in
#   them, invisible. Roots are now a list, and a merged, idle tree outside every root is reported
#   with its size rather than passed over — an unswept gigabyte you can see is a decision, and one
#   you cannot is the bug above wearing a different hat.
#
#   scripts/sweep-worktrees.sh            # say what would go, touch nothing
#   scripts/sweep-worktrees.sh --apply    # actually remove them
#   BASE=develop scripts/sweep-worktrees.sh --apply   # merged into something other than main
#
# `origin/main` is only as fresh as the last fetch, so `git fetch` first if the sweep is the first
# thing you do in a session. This script does not fetch on its own: a network call that changes
# which directories get deleted belongs in the hands of whoever is deciding to delete them.
set -uo pipefail

APPLY=0
[ "${1:-}" = "--apply" ] && APPLY=1

# The main checkout, not the current one — this script is frequently run from inside one of the
# worktrees it is reasoning about, where --show-toplevel answers with that worktree instead.
repo=$(dirname "$(git rev-parse --path-format=absolute --git-common-dir)") || exit 1
cd "$repo" || exit 1

# Where the dispatchers put worktrees. A tree outside all of these belongs to somebody else and
# is only ever reported, never removed.
ROOTS=(
    "$repo/.claude/worktrees"
    "$repo/.codex/worktrees"
    "$(dirname "$repo")/.$(basename "$repo")-worktrees"
)

# Every ref that means "main" here. Unset BASE takes both, because a branch can be merged into
# either one first depending on whether it landed by pull request or by hand.
BASES=()
for ref in ${BASE:-main origin/main}; do
    git rev-parse --verify --quiet "$ref^{commit}" >/dev/null && BASES+=("$ref")
done
[ ${#BASES[@]} -gt 0 ] || { echo "no base ref resolved from '${BASE:-main origin/main}'" >&2; exit 1; }
echo "merged means: contained in ${BASES[*]}"

# Untracked paths that are noise rather than work.
JUNK='__pycache__|\.DS_Store'
rc=0
outside=0

while read -r wt; do
    [ "$wt" = "$repo" ] && continue          # the main checkout is not a worktree to reclaim
    name=$(basename "$wt")

    head=$(git -C "$wt" rev-parse HEAD 2>/dev/null) || {
        echo "skip  $name — cannot read HEAD"
        continue
    }

    merged=0
    for base in "${BASES[@]}"; do
        git merge-base --is-ancestor "$head" "$base" 2>/dev/null && { merged=1; break; }
    done
    [ "$merged" = "1" ] || {
        echo "keep  $name — not merged into ${BASES[*]}"
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

    # Dead by every measure, so the only question left is whether it is ours to delete. Say so
    # either way: a tree this script will not touch is the disk it is failing to reclaim, and
    # reporting it is how the next root gets added before the volume fills again.
    ours=0
    for root in "${ROOTS[@]}"; do
        case "$wt" in "$root"/*) ours=1; break ;; esac
    done
    [ "$ours" = "1" ] || {
        echo "outside $name ($size) — merged and idle, but not under a swept root: $wt"
        outside=$((outside + 1))
        continue
    }

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

[ "$outside" = "0" ] || echo "$outside merged worktree(s) sit outside every swept root; add a root or remove them by hand"

exit $rc
