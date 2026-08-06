#!/usr/bin/env bash
# Take the pictures a UI pull request has to carry, on both sides of the change.
#
# The two surfaces already know how to photograph themselves — `--shot` on the Swift app since
# me9.8.2, and `cs tui --shot` since this script existed. Neither knows what the change *was*,
# which is the whole difficulty: an "after" on its own asks a reviewer to remember a screen they
# last saw a week ago. This builds the base revision too and takes the same frames from it.
#
# The base build is the entire cost here — a cold `swift build -c release` is minutes, and it is
# why this is a script somebody runs rather than something the gate does on every commit.
#
#   scripts/shot.sh                          # HEAD against its merge-base with main
#   scripts/shot.sh --base HEAD~1            # against one commit back
#   scripts/shot.sh --after-only             # no base build; use when the surface is new
#   scripts/shot.sh --tui                    # skip the Swift app, which is most of the time
#   scripts/shot.sh --upload                 # put the PNGs somewhere a PR body can reach
#
# docs/PULL-REQUESTS.md says which of these a given change needs.
set -euo pipefail

REPO="$(git rev-parse --show-toplevel)"
OUT="${TMPDIR:-/tmp}/chat-search-shots"
DB="${CS_SHOT_DB:-$HOME/.chat-archive/index.db}"
QUERY="borrow checker"
SIZE_TUI="140x40"
SIZE_APP="1200x800"
BASE=""
AFTER_ONLY=0
UPLOAD=0
WANT_TUI=1
WANT_APP=1
# One release used as an asset store. Not a release of anything — it exists because `gh pr create
# --body` cannot upload an image, and the alternative is committing PNGs into the history of a
# repository whose whole ethos is that generated things do not get committed.
SHOT_TAG="shots"

while [ $# -gt 0 ]; do
  case "$1" in
    --base) BASE="$2"; shift 2 ;;
    --out) OUT="$2"; shift 2 ;;
    --db) DB="$2"; shift 2 ;;
    --query) QUERY="$2"; shift 2 ;;
    --after-only) AFTER_ONLY=1; shift ;;
    --upload) UPLOAD=1; shift ;;
    --tui) WANT_APP=0; shift ;;
    --app) WANT_TUI=0; shift ;;
    -h|--help) sed -n '2,18p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "unknown flag: $1" >&2; exit 2 ;;
  esac
done

[ -f "$DB" ] || { echo "no index at $DB — run 'cs index' first, or pass --db" >&2; exit 1; }

# Resolved rather than assumed: a worktree branched from a stale origin/main would report a diff
# against a base the reviewer is not merging into.
if [ -z "$BASE" ] && [ "$AFTER_ONLY" -eq 0 ]; then
  BASE="$(git merge-base HEAD origin/main 2>/dev/null || git merge-base HEAD main 2>/dev/null || true)"
  [ -n "$BASE" ] || { echo "could not find a merge-base; pass --base or --after-only" >&2; exit 1; }
fi
if [ -n "$BASE" ] && [ "$(git rev-parse "$BASE")" = "$(git rev-parse HEAD)" ]; then
  echo "base and HEAD are the same commit — nothing to compare, taking the after side only"
  AFTER_ONLY=1
fi

# Shoot one checkout into one directory. Called for HEAD in place, and for the base in a throwaway
# worktree — the same function both times, so the two sides cannot drift in how they were taken.
shoot() {
  local root="$1" dest="$2" label="$3"
  mkdir -p "$dest"
  echo "── $label ($(git -C "$root" rev-parse --short HEAD))"

  if [ "$WANT_TUI" -eq 1 ]; then
    ( cd "$root" && cargo build --release -p cs >/dev/null 2>&1 )
    # A revision only photographs itself if it already knew how. The base of the very PR that
    # adds a shot mode does not, and neither does any base predating a flag this run passes —
    # so the capability is probed rather than assumed. Reported and skipped, never invented:
    # a missing before side is a fact about the comparison, and the alternative is a diff
    # against frames that were never taken.
    if "$root/target/release/cs" tui --help 2>&1 | grep -q -- '--shot'; then
      "$root/target/release/cs" tui --shot \
        --db "$DB" --query "$QUERY" --out "$dest" --size "$SIZE_TUI" | sed 's/^/   /'
    else
      echo "   this revision has no 'cs tui --shot' — no TUI frames from this side"
    fi
  fi

  if [ "$WANT_APP" -eq 1 ]; then
    ( cd "$root" && cargo build --release -p cs >/dev/null 2>&1 )
    ( cd "$root/apps/macos" && swift build -c release >/dev/null 2>&1 )
    # `capture` draws the view hierarchy with `cacheDisplay`, so there is no window server in
    # this and no Screen Recording grant to ask for. It runs headless, in the background, out of
    # a detached session — which is what makes it usable by an agent at all.
    if "$root/apps/macos/.build/release/chat-search" --help 2>&1 | grep -q -- '--shot'; then
      "$root/apps/macos/.build/release/chat-search" --shot \
        --db "$DB" --bin "$root/target/release/cs" \
        --query "$QUERY" --out "$dest/app.png" --size "$SIZE_APP" 2>/dev/null \
        | grep -E '\.png|→' | sed 's/^/   /' || true
    else
      echo "   this revision has no 'chat-search --shot' — no app frames from this side"
    fi
  fi
}

rm -rf "$OUT"
shoot "$REPO" "$OUT/after" "after — this branch"

if [ "$AFTER_ONLY" -eq 0 ]; then
  WORK="$(mktemp -d)/base"
  # Detached, so this never moves a branch anybody is on. Removed in the trap even on failure —
  # a leaked worktree shows up in `/bd-review` as a mystery and costs somebody a minute.
  git worktree add --detach "$WORK" "$BASE" >/dev/null 2>&1
  trap 'git worktree remove --force "$WORK" >/dev/null 2>&1 || true' EXIT
  echo "   (building the base revision — this is the slow half)"
  shoot "$WORK" "$OUT/before" "before — $(git rev-parse --short "$BASE")"
fi

echo
echo "── what moved"
if [ "$AFTER_ONLY" -eq 1 ]; then
  echo "   after-only run; nothing to compare against"
elif [ -z "$(ls -A "$OUT/before" 2>/dev/null)" ]; then
  # Distinguished from a run where every frame is genuinely new: one says the change added a
  # surface, the other says the comparison never happened. A PR body that confuses the two
  # claims a before/after it does not have.
  echo "   the base revision produced no frames — this is an after-only run in effect."
  echo "   Say so in the PR body rather than presenting these as a before/after."
else
  # Text frames diff honestly. PNGs get a size comparison and nothing more — a byte difference in
  # a PNG says the pixels changed, not what changed, and pretending otherwise would be worse than
  # saying plainly that a human has to look.
  for a in "$OUT"/after/*; do
    [ -e "$a" ] || continue
    name="$(basename "$a")"
    b="$OUT/before/$name"
    if [ ! -e "$b" ]; then
      echo "   NEW    $name"
    elif cmp -s "$a" "$b"; then
      echo "   same   $name"
    elif [ "${name##*.}" = "txt" ]; then
      echo "   CHANGED $name ($(diff "$b" "$a" | grep -c '^[<>]') lines)"
    else
      echo "   CHANGED $name ($(wc -c <"$b" | tr -d ' ') → $(wc -c <"$a" | tr -d ' ') bytes) — look at it"
    fi
  done
fi

echo
echo "── frames are under $OUT"
if [ "$UPLOAD" -eq 1 ]; then
  branch="$(git rev-parse --abbrev-ref HEAD | tr '/' '-')"
  if ! gh auth status >/dev/null 2>&1; then
    echo "   gh is not authenticated — nothing uploaded. Text frames need no hosting; paste them."
    exit 0
  fi
  gh release view "$SHOT_TAG" >/dev/null 2>&1 || \
    gh release create "$SHOT_TAG" --title "PR screenshots" \
      --notes "Asset store for pull-request screenshots. Not a release." >/dev/null
  echo "   uploading as $branch-*"
  for side in before after; do
    for f in "$OUT/$side"/*.png; do
      [ -e "$f" ] || continue
      asset="$branch-$side-$(basename "$f")"
      cp -f "$f" "${TMPDIR:-/tmp}/$asset"
      gh release upload "$SHOT_TAG" "${TMPDIR:-/tmp}/$asset" --clobber >/dev/null 2>&1
      echo "   https://github.com/$(gh repo view --json nameWithOwner -q .nameWithOwner)/releases/download/$SHOT_TAG/$asset"
    done
  done
fi
