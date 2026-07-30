# Eval

This is how you find out whether the ranking is any good. You write down queries whose answers you can recognise, grade what comes back, and get a number you can compare across changes to the ranking.

**Deferred as of 2026-07-30, and the reason is worth knowing before you use any of this.** A query invented by reading the corpus has no information need behind it, so asked which result is "the one you meant", the honest answer is usually that several are fine and none is the one. Judging a set like that measures the queries, not the ranking.

So `cs search` and `cs pick` now record what you actually search for and actually open, and the next set gets built from that (`chat-search-6eb.21`). `cs needs` shows what has accumulated. Roughly 50–100 picks across 20+ distinct queries is worth converting; below that this hand-written set is still the better of the two.

Everything below works and the harness is unchanged. The 24 seeded queries are a usable stopgap if you want numbers now — just know what they are.

```
cs eval sheet     # write one gradeable file per query into sheets/
$EDITOR evals/sheets/*.md
cs eval collect   # read the grades back and record them
cs eval run       # score the ranking
```

Each takes `--set`, defaulting to `evals/ranking.toml`.

## What lives where

`ranking.toml` holds the queries. You write it by hand and the tool never rewrites it, so any comment you leave explaining why a query is in the set is still there months later.

`ranking.judgments.jsonl` holds the grades. `collect` appends one JSON object per grade and never edits or deletes, so when two lines cover the same query and conversation the later one counts. That is what makes changing your mind cheap: re-grade the row, collect again, and the new line wins while the old one stays in the file as a record of what you thought before.

`sheets/` holds the working files. They are disposable. Every grade in them is either already in the log or about to be, so deleting the directory costs one `cs eval sheet`.

All three are gitignored. The queries say what you were working on and the judged ids point straight at the conversations, so an eval set is conversation content in all but name. Only `example.toml` and this file get committed.

## Grades

|                |                                                           |
| -------------- | --------------------------------------------------------- |
| 3 · exact      | the conversation the query was reaching for               |
| 2 · useful     | would have been a good answer, though not the one in mind |
| 1 · related    | same subject, would not have answered it                  |
| 0 · irrelevant | nothing to do with the query                              |

MRR and success@k count only 2 and 3 as hits. Grade 1 sits below that cutoff on purpose. A search can return things genuinely about the right subject, never return the conversation you actually wanted, and still look reasonable when you check it one result at a time. If `related` counted as a hit, that failure would score well.

## Judging

`cs eval sheet` writes one file per query into `sheets/`, and you fill in the first column:

```
_  chatgpt-export:676190a9-0420-4188-b4b9-87afcd99e1a5
   Camping Trip Options: Seattle
   chatgpt-export · 2024-05-22 · 3 turns
   We live in Seattle and want to go on a two-night camping trip in June…
```

Replace the `_` with a digit. `cs eval collect` reads the columns back and appends them to the log. Anything that is not in the first column is ignored, so you can leave yourself notes anywhere in the file.

The reason for a file rather than a prompt is that relevance is a comparative judgement. Whether a result deserves 2 or 1 depends on what else came back: if the exact conversation is sitting three rows down, the merely-plausible one above it is `related`, and if nothing better turned up it might be `useful`. You need to see the whole pool to make that call, and a one-at-a-time prompt makes you do it from memory.

Two things about the format worth knowing. Conversation ids are pre-filled, and `collect` checks every id against the index and refuses the whole batch if one does not resolve, since a hand-typed id is nearly always a typo. And a grade character that is not 0-3 or `_` is an error rather than a skipped line, because dropping it silently would lose a judgement you actually made.

Candidates are listed newest first and carry no rank. Rank order would anchor your judgement to the ranking being measured, which is the bias the pool exists to remove, and it would also reshuffle the file every time the ranking changed.

You can stop whenever. Re-running `sheet` brings every grade already in the log back pre-filled, so nothing is lost, and it refuses to overwrite a sheet holding grades you have not collected yet. To change a grade, edit the digit and collect again. Corrections append, so the file keeps what you thought the first time.

## The pool

A sheet lists candidates from three rankers: the default one, one with the recency decay turned off, and one with repeat-match damping turned off.

The reason is that judging is the only way a conversation enters the scored set at all. Anything you never see, you never grade, and anything ungraded counts as irrelevant. So if every candidate came from the current ranker, a conversation the current constants push below rank 10 would be permanently invisible, and later attempts to tune those constants would be scored against judgements that already assumed the current values were right. The decay and the damping are the two constants `chat-search-6eb.13` exists to move, which is why the pool varies exactly those two.

## Reading the output

`nDCG@10` is the main number. It compares the order you got against the best order your judgements would have allowed, so 1.000 means nothing you graded higher is sitting below something you graded lower. `MRR` answers how far down the list you have to go before something useful turns up. `success@1` and `success@5` are the blunt version: did anything useful show up at all.

Three things below the numbers are reasons to distrust them, which is why `run` prints them last.

- Coverage below roughly 80% means most of the top ten carries no grade, and ungraded results score as irrelevant. Judge more before you compare two configurations against each other.
- An unscorable query is one where nothing you graded came out above irrelevant. Those are left out of the nDCG average instead of counting as zero, because otherwise adding a fresh query to the set would drag the score down before you had judged it.
- A dangling judgement points at a conversation the index no longer holds, and it counts as a miss. Usually a reimport changed the native ids and the set has drifted away from the index.

## Sweeping

`run` takes the tuning constants as flags, so trying different values is a shell loop and needs no rebuild:

```sh
for w in 0.0 0.15 0.25 0.4 1.0; do
  cs eval run --set evals/ranking.toml --repeat-weight "$w" --json |
    jq -r "\"repeat_weight $w  nDCG \(.report.ndcg)  MRR \(.report.mrr)\""
done
```

Judge before you sweep. When coverage is low, the gaps between runs mostly reflect which results happened to get graded, not which constants are better.
