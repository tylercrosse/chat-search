#!/usr/bin/env python3
"""Export a slice of the real index for the UI prototype.

Throwaway measurement instrument, not product code — the same status as poc/rust and
poc/ts. It exists so the prototype can be judged against real conversations without
waiting for `cs show`, and so that decisions about the ribbon, the segment fold and
cluster proposals are made against the corpus rather than against fixtures I invented.

    python3 poc/ui/export.py                 # ~/.chat-archive/index.db -> real-data.json
    python3 poc/ui/export.py --limit 300

Opens the index READ-ONLY. Writes poc/ui/real-data.json, which is gitignored: it is
conversation text, and .gitignore's existing rule about transcripts covers it.

What it does NOT do: rank. There is no BM25 here. The prototype searches the exported
text by substring in the browser, which is enough to place match ticks and pick a best
match, and is not a claim about what `cs search` would return.
"""

import argparse
import json
import math
import os
import re
import sqlite3
import sys
from collections import Counter, defaultdict

HOME = os.path.expanduser("~")
DEFAULT_DB = os.path.join(HOME, ".chat-archive", "index.db")
OUT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "real-data.js")

# Text budgets. tool_result carries no text at all unless it failed, which mirrors what
# the preview actually renders (TUI-DESIGN §8) and is where most of the bytes are.
PROSE_CHARS = 1200
TOOL_CHARS = 120
ERR_CHARS = 200

KIND_CODE = {"prose": "p", "reasoning": "r", "tool_call": "c", "tool_result": "o"}

# A compaction boundary: the point where the harness threw the earlier context away and
# replaced it with a summary. Everything above it is no longer verbatim to the agent, so
# it is a harder discontinuity than the >4h pause the ribbon already marks.
#
# Measured: 11 conversations, 12 boundaries, all claude-code, all mid-stream (seq 51-924,
# never at the head — so this splits a conversation rather than starting one, and
# forked_from is null on every one). Rare overall at 0.4% of the corpus, but 23% of
# claude-code conversations over 600 messages and 0% of those under 100 — it appears
# exactly where a reader is most lost.
#
# String-matching a harness sentence, which is what makes this a heuristic and not a
# fact: it will drift when the wording changes, and it finds nothing for Codex even
# though Codex compacts too. The durable version records the boundary in the importer.
COMPACTED = re.compile(
    r"This session is being continued from a previous conversation that ran out of context"
    r"|The summary below covers the earlier portion of the conversation", re.I)
ROLE_CODE = {"user": "u", "assistant": "a", "tool": "t", "system": "s"}

# Stop words for the cluster proposals. Ordinary English plus the vocabulary this corpus
# is saturated with — without the second group every cluster is "the file code function".
STOP = set("""
a an and are as at be been but by can could did do does for from had has have he her his
how i if in into is it its me my no not of on or our out she so some such than that the
their them then there these they this to too us was we were what when where which who why
will with would you your about after all also am any because before being between both
each few more most other over same should some through under up very
file files code function functions line lines run running use used using like just get
got make made need needs want add added change changed set sets new now one two also
work works working try trying see look looks going think know say says thing things
right ok okay yes yeah sure let lets going want need able way ways time
tool tools exec output delta action transcript agent agents session sessions message
messages user assistant response responses request prompt context token tokens call calls
codex claude gpt chatgpt gemini cli src crates dev repo directory folder path paths
task tasks item items thing stuff bit lot lots
""".split())

WORD = re.compile(r"[a-z][a-z0-9_\-\.]{2,}")

# Tool-call arguments are the richest unclaimed signal in the index: 30,373 of 63,585
# tool_calls carry a file path. Extracting them makes "which conversations touched
# preview.rs" answerable with no new machinery at all.
FILE_RE = re.compile(
    r"[\w./\-]*?([\w\-]+\.(?:rs|py|ts|tsx|js|jsx|md|toml|json|sql|sh|ya?ml|html|css|txt))")

# The same extensions, keeping the *path* rather than the basename. Scoped to a project
# `crates/cs-core/src/preview.rs` is a different fact from `preview.rs`, and the corpus
# has four `README.md`s that mean four unrelated things.
PATH_RE = re.compile(
    r"(?:[\w.\-]+/)+[\w\-]+\.(?:rs|py|ts|tsx|js|jsx|md|toml|json|sql|sh|ya?ml|html|css|txt)")

# Agent scaffolding, present in every repo and therefore distinguishing none of them.
SCAFFOLD = {"SKILL.md", "CLAUDE.md", "AGENTS.md"}

# Harness state, not project files. `~/.claude/plans/<slugified-first-message>.md` and
# `~/Documents/Codex/<date>/<slug>/` both encode the conversation in the path, so they
# read as a file the work touched when they are really the tooling talking about itself —
# the same failure the MARKUP strip exists for, one layer down.
HARNESS = re.compile(r"(^|/)\.(claude|codex)/|/Documents/Codex/|(^|/)\.git/")

# Harness scaffolding. The importer strips this from *titles* (TUI-DESIGN §4, where it
# polluted 24% of visible rows) but not from message text — so without stripping it here
# the largest cluster this script produced was "command-message, command-name, beads,
# bd-dispatch", which is not a topic, it is the tooling talking to itself.
MARKUP = re.compile(
    r"<(command-[a-z]+|environment_context|ide_[a-z]+|system-reminder|local-command[^>]*)>.*?"
    r"</\1>|<[a-z_-]+/>|Caveat: The messages below.*?$|Base directory for this skill:.*?$"
    r"|<\/?[a-z_-]+>",
    re.S | re.M | re.I)


def strip_markup(text):
    return MARKUP.sub(" ", text or "")


def tokens(text):
    for w in WORD.findall(strip_markup(text).lower()):
        w = w.strip(".-_")
        if len(w) >= 3 and w not in STOP and not w.isdigit():
            yield w


def connect(path):
    if not os.path.exists(path):
        sys.exit(f"no index at {path} — run `cs index` first, or pass --db")
    return sqlite3.connect(f"file:{path}?mode=ro", uri=True)


# --------------------------------------------------------------- projects ---
# A cwd is not a project. Grouping the raw column, as the first prototype did, produced
# 111 groups that misrepresented the corpus in four measured ways:
#
#   * worktrees split a project across rows. chat-search is 98 conversations in the repo
#     plus 15 across 10 worktree directories, shown as 11 rows scattered by count.
#   * 30 directories are Codex scratch: ~/Documents/Codex/<date>/<slugified-first-message>.
#     The path *is* the opening line of the conversation, so it is not a project and it is
#     redundant with the title. 43 conversations.
#   * 2 are per-run temp directories under /private/var/folders.
#   * one project moved on disk. /dev/career (89) and /dev/projects/career (65) are the
#     same work in two rows at opposite ends of a list sorted by count.
#
# The first three are string rules over the archive, so they stay pure (ADR 1) — no
# filesystem probing, nothing to go stale. The fourth is not derivable and is left alone:
# merging paths is an authored fact and belongs in library.db (ADR 3), not here.
CODEX_SCRATCH = re.compile(r"/Documents/Codex/\d{4}-\d{2}-\d{2}/")
CLAUDE_WORKTREE = re.compile(r"^(.*?)/\.claude/worktrees/[^/]+/?$")
CODEX_WORKTREE = re.compile(r"^(/Users/[^/]+)/\.codex/worktrees/[0-9a-f]+/(.+)$")


def project_of(cwd, basenames=None):
    """cwd -> project path, or None when the directory is not a project at all.

    `basenames` maps a directory's last segment to its full path; it is what lets a
    detached Codex worktree (~/.codex/worktrees/<hash>/mars-attacks) rejoin the project
    it was cut from. Without it the worktree is kept as its own project rather than
    guessed at.
    """
    if not cwd or CODEX_SCRATCH.search(cwd) or cwd.startswith("/private/var/"):
        return None

    m = CLAUDE_WORKTREE.match(cwd)
    if m:
        return m.group(1)

    m = CODEX_WORKTREE.match(cwd)
    if m:
        name = m.group(2).split("/")[-1]
        return (basenames or {}).get(name, cwd)

    return cwd.rstrip("/")


def corpus_projects(conn):
    """Project rollup over the whole index, not over the sample.

    The topic counts in the left rail read thinner than the corpus is because they are
    per-sample; repeating that mistake here would make every project look like a handful
    of conversations. So the row shows the true count and the export says how much of it
    it carried.
    """
    cur = conn.cursor()
    rows = list(cur.execute(
        """SELECT id, cwd, source, ended_at, msg_count FROM conversation
           WHERE cwd IS NOT NULL AND cwd <> '' AND deleted_upstream_at IS NULL"""))

    # Two passes: the first learns which directories exist, the second can then attach a
    # detached worktree to the project whose basename it shares.
    basenames = {}
    for _, cwd, _, _, _ in rows:
        p = project_of(cwd)
        if p and not CODEX_WORKTREE.match(p):
            basenames.setdefault(p.split("/")[-1], p)

    members, meta = defaultdict(list), {}
    for cid, cwd, source, ended, n in rows:
        p = project_of(cwd, basenames)
        if not p:
            continue
        members[p].append(cid)
        m = meta.setdefault(p, {"n": 0, "msgs": 0, "first": None, "last": None,
                                "sources": set(), "dirs": set()})
        m["n"] += 1
        m["msgs"] += n or 0
        m["sources"].add(source)
        m["dirs"].add(cwd)
        if ended:
            m["first"] = min(m["first"] or ended, ended)
            m["last"] = max(m["last"] or 0, ended)

    ids = [cid for v in members.values() for cid in v]
    qmarks = ",".join("?" * len(ids))
    owner = {cid: p for p, v in members.items() for cid in v}

    # Files and topic vocabulary in one pass. tool_call text is where the paths are;
    # title plus user prose is the same basis seeded_topics() uses, so a project's
    # fingerprint and the topic rail cannot disagree about what a conversation is about.
    files = defaultdict(Counter)
    bags = defaultdict(set)
    for cid, title in cur.execute(
            f"SELECT id, title FROM conversation WHERE id IN ({qmarks})", ids):
        bags[cid].update(tokens(title or ""))

    for cid, kind, role, text in cur.execute(
        f"""SELECT conv_id, kind, role, text FROM message
            WHERE conv_id IN ({qmarks})
              AND (kind = 'tool_call' OR (kind = 'prose' AND role = 'user'))""", ids):
        if kind == "tool_call":
            p = owner[cid]
            for path in PATH_RE.findall(text or ""):
                if path.split("/")[-1] not in SCAFFOLD and not HARNESS.search(path):
                    files[p][path] += 1
        else:
            bags[cid].update(tokens(text))

    out = []
    for p, m in meta.items():
        # Counted in conversations, not in term hits. Ranking a project's topics by raw
        # frequency made every one of them read "Rust and cargo 1392, Prose and editing
        # 1113" — the length bias of a 2,553-message conversation, not a fingerprint.
        # This is the same membership rule the topic rail uses, so the two agree.
        hits = Counter()
        for cid in members[p]:
            for name, seed in TOPIC_SEEDS.items():
                if seed & bags[cid]:
                    hits[name] += 1

        # Subjects before modes. Ranked by count alone the fingerprint of three of the
        # six largest projects opened with "Git and version control" — true, and useless:
        # a mode fires on every agentic project, so it distinguishes none of them. The
        # facet split that resolved the near-duplicate topics does the same job here.
        ranked = sorted(hits.items(),
                        key=lambda kv: (TOPIC_GROUPS.get(kv[0], ("", "subject"))[1] != "subject",
                                        -kv[1]))

        # Paths are stored absolute but read as noise that way: every row would open with
        # the same 22 characters of home directory. PATH_RE has no leading slash to match,
        # so an absolute hit arrives as `Users/…` and needs the prefix stripped too.
        prefix = p.rstrip("/") + "/"
        def rel(f):
            for pre in (prefix, prefix.lstrip("/")):
                if f.startswith(pre):
                    return f[len(pre):]
            return f
        merged = Counter()
        for f, n in files[p].items():
            merged[rel(f)] += n
        top = merged.most_common(10)
        out.append({
            "path": p,
            "name": "/".join(p.split("/")[-2:]) if p.count("/") > 2 else p,
            "n": m["n"], "msgs": m["msgs"], "first": m["first"], "last": m["last"],
            "dirs": len(m["dirs"]),
            "sources": sorted(m["sources"]),
            "topics": [{"name": k, "n": v,
                        "facet": TOPIC_GROUPS.get(k, ("", "subject"))[1]}
                       for k, v in ranked[:8]],
            "files": [{"path": f, "n": n} for f, n in top],
        })
    return sorted(out, key=lambda p: -(p["last"] or 0))


def pick(conn, limit, projects=None, per_project=0):
    """Stratified by source, then recency, plus the largest few, plus depth per project.

    Recency alone was wrong: it returned 144 claude-code, 25 codex and *no* ChatGPT,
    when ChatGPT is two thirds of the corpus. Every cross-tool claim in the design —
    collections spanning companies, sittings crossing them — was untestable on that
    sample. Stratifying is the difference between exercising the premise and confirming
    a selection effect.

    The project stratum is the same argument one level down. Source-stratified sampling
    gave 9 projects, 6 of them with four conversations or fewer — enough to draw a list
    of projects, not enough to exercise what goes *inside* one: the run dividers, the
    20-row cap, or the claim that a project's conversations are hard to tell apart.
    """
    cur = conn.cursor()
    sources = [r[0] for r in cur.execute(
        "SELECT source FROM conversation GROUP BY source ORDER BY COUNT(*) DESC")]
    per = max(8, limit // max(1, len(sources)))

    seen, out = set(), []
    for src in sources:
        for (cid,) in cur.execute(
            """SELECT id FROM conversation
               WHERE source = ? AND msg_count >= 4 AND deleted_upstream_at IS NULL
               ORDER BY ended_at DESC LIMIT ?""", (src, per)):
            if cid not in seen:
                seen.add(cid)
                out.append(cid)

    # The largest, wherever they came from: they are where the ribbon and the segment
    # fold are under real pressure, and a fixture cannot fake a 2,553-message thread.
    for (cid,) in cur.execute(
        """SELECT id FROM conversation
           WHERE msg_count >= 4 AND deleted_upstream_at IS NULL
           ORDER BY msg_count DESC LIMIT 12"""):
        if cid not in seen:
            seen.add(cid)
            out.append(cid)

    def from_project(p, want):
        rows = cur.execute(
            """SELECT id, cwd FROM conversation
               WHERE cwd LIKE ? AND msg_count >= 4 AND deleted_upstream_at IS NULL
               ORDER BY ended_at DESC LIMIT ?""", (p["path"] + "%", want * 3))
        taken = 0
        for cid, cwd in rows:
            if taken >= want:
                break
            # LIKE 'x%' also matches a sibling named x-old, so re-check the rule that
            # actually defines membership.
            if project_of(cwd) != p["path"] and not cwd.startswith(p["path"] + "/"):
                continue
            taken += 1
            if cid not in seen:
                seen.add(cid)
                out.append(cid)

    ranked = projects or []
    for p in ranked[:10]:
        from_project(p, per_project)

    # And a few of the small ones. 38 of 67 projects hold three conversations or fewer —
    # more than half the list — so a sample drawn only from the biggest ten shows a
    # corpus where every project is large, and the fold that exists for the tail never
    # appears to be exercised.
    for p in [x for x in ranked if x["n"] <= 3][:22]:
        from_project(p, 2)
    return out


def load_conversations(conn, ids):
    cur = conn.cursor()
    qmarks = ",".join("?" * len(ids))
    meta = {
        r[0]: dict(id=r[0], source=r[1], native_id=r[2], title=r[3], title_origin=r[4],
                   cwd=r[5], model=r[6], started_at=r[7], ended_at=r[8],
                   msg_count=r[9], prose_count=r[10], user_turns=r[11], thread_count=r[12])
        for r in cur.execute(
            f"""SELECT id, source, native_id, title, title_origin, cwd, model,
                       started_at, ended_at, msg_count, prose_count, user_turns, thread_count
                FROM conversation WHERE id IN ({qmarks})""", ids)
    }

    msgs = defaultdict(list)
    # Same ordering the TUI uses: seq is per-thread, so ordering by it alone interleaves
    # every strand at once and a 9-thread conversation reads as nonsense.
    for r in cur.execute(
        f"""SELECT conv_id, role, kind, seq, is_error, is_sidechain, ts, on_head_path, text
            FROM message WHERE conv_id IN ({qmarks}) AND on_head_path = 1
            ORDER BY conv_id, is_sidechain, thread_key, seq""", ids):
        conv_id, role, kind, seq, is_err, side, ts, on_path, text = r
        m = {"k": KIND_CODE.get(kind, "p"), "r": ROLE_CODE.get(role, "a"), "n": len(text or "")}
        if ts:
            m["t"] = ts
        if is_err:
            m["e"] = 1
        if side:
            m["s"] = 1

        text = (text or "").strip()
        if COMPACTED.search(text):
            m["z"] = 1
        if kind == "prose" or kind == "reasoning":
            m["x"] = text[:PROSE_CHARS]
        elif kind == "tool_call":
            # The call name is the first line; the arguments follow as JSON. The preview
            # only draws the name, but the arguments are where the file paths live, so
            # they are mined here and dropped rather than carried.
            head = text.splitlines()[0] if text else ""
            paths = FILE_RE.findall(text)
            m["x"] = (head + (" " + paths[0] if paths else ""))[:TOOL_CHARS]
            # Carried explicitly rather than re-parsed out of `x`, which is truncated at
            # 120 chars and would lose the name on a long first line. It is what the act
            # classification keys on: 47,479 calls run something, 9,460 change a file,
            # 4,502 look at one — a distinction the single "tool" band cannot make.
            m["c"] = head.split("(")[0].split()[0][:24] if head else "tool"
            # Full paths, not basenames: `crates/cs-core/src/preview.rs` is a different
            # fact from `preview.rs`, and the corpus holds four unrelated `README.md`s.
            full = [p for p in PATH_RE.findall(text)
                    if p.split("/")[-1] not in SCAFFOLD and not HARNESS.search(p)]
            if full:
                m["f"] = sorted(set(full))[:4]
            elif paths:
                m["f"] = sorted(set(paths))[:4]
        elif is_err:
            m["x"] = text[:ERR_CHARS]
        msgs[conv_id].append(m)

    out = []
    for cid in ids:
        if cid not in meta or not msgs.get(cid):
            continue
        c = meta[cid]
        c["msgs"] = msgs[cid]
        files = Counter(f for m in c["msgs"] for f in m.get("f", []))
        c["files"] = [f for f, _ in files.most_common(12)]
        out.append(c)
    return out


def lineages(convs, gap_days=3):
    """A line of inquiry: consecutive work in one directory, split on long gaps.

    Neither a topic nor a sitting. A sitting is one afternoon; a topic is a subject.
    This is "the stretch where I was working on that" — which is how you actually
    remember a piece of work, and it is free from cwd plus ended_at.
    """
    by_dir = defaultdict(list)
    for c in convs:
        if c.get("cwd"):
            by_dir[c["cwd"]].append(c)

    out = []
    for cwd, group in by_dir.items():
        group.sort(key=lambda c: c.get("ended_at") or 0)
        chain = []
        for c in group:
            if chain and (c.get("ended_at") or 0) - (chain[-1].get("ended_at") or 0) > gap_days * 86400000:
                if len(chain) > 1:
                    out.append((cwd, chain))
                chain = []
            chain.append(c)
        if len(chain) > 1:
            out.append((cwd, chain))

    return sorted(
        [{"cwd": cwd, "ids": [c["id"] for c in ch],
          "start": ch[0].get("ended_at"), "end": ch[-1].get("ended_at"),
          "sources": sorted({c["source"] for c in ch})} for cwd, ch in out],
        key=lambda l: -len(l["ids"]))[:14]


# ---------------------------------------------------------------- topics ---
# Seeded by hand. Started from ~/dev/sandbox/chat-history/organizer's nine, which were
# derived from the raw ChatGPT export, then split them.
#
# The nine were too broad to explore: "AI and machine learning" reached 899 of 3,059
# conversations and "Programming and software" 872. A topic covering 29% of the corpus
# does not narrow anything. Splitting takes the median topic to ~5% while overlap stays
# low — measured max Jaccard 0.34 across 27 topics, and nothing contained in anything.
#
# Overlap between topics is expected and fine: a GPU programming conversation genuinely
# is systems and deep learning at once. What is NOT fine is two topics sharing a seed
# *word*, which manufactures overlap that is not real — "agent" sat in both Agents and
# Reinforcement learning, "embedding" in both Transformers and Embeddings. Those are
# bugs; conceptual overlap is not.
#
# Splitting sub-topics out automatically did not work. Mining distinctive terms inside
# "Programming and software" returned "completed, status, local, error, implement" —
# the vocabulary is not separable that way at this granularity. Hand-written seeds are
# the honest tool, and this dict is meant to be edited.
#
# This replaces the k-means pass that was here. k-means assigns *every* conversation,
# so it always produced a leftovers bucket — 68 conversations at 0.22 cohesion whose
# top terms were "practice, interview, amazon, seems". Seeded topics do not assign:
# a conversation matches zero, one, or several, and 35% of this corpus matches none.
# That fraction is a fact to show, not a failure to hide.
#
# And they cost nothing structurally: a seeded topic is already expressible as a
# collection, `matches(query) ∪ pinned − excluded`. These are nine queries.
TOPIC_GROUPS = {
    # (group, facet). Facet is the axis the topic lives on: a *subject* is what a
    # conversation was about, a *mode* is how you were working. "Prose and editing"
    # and "Technical writing and docs" read as near-duplicates in a flat list and share
    # only 0.13 Jaccard on the corpus — because one is a mode and one is a subject.
    "Rust and cargo": ("Systems", "subject"),
    "SQLite and FTS5": ("Systems", "subject"),
    "Embeddings and vector search": ("Systems", "subject"),
    "Terminal UI": ("Systems", "subject"),
    "Agents and tooling": ("Systems", "subject"),
    "Git and version control": ("Systems", "mode"),
    "Shell and CLI": ("Systems", "subject"),
    "macOS and hardware": ("Systems", "subject"),
    "Web frontend": ("Systems", "subject"),
    "GPU and CUDA": ("Machine learning", "subject"),
    "Deep learning training": ("Machine learning", "subject"),
    "Transformers and architecture": ("Machine learning", "subject"),
    "Reinforcement learning": ("Machine learning", "subject"),
    "LLM evaluation": ("Machine learning", "mode"),
    "AI safety and alignment": ("Machine learning", "subject"),
    "Security and exploits": ("Craft", "subject"),
    "Data analysis": ("Craft", "mode"),
    "Math and proofs": ("Craft", "subject"),
    "Technical writing and docs": ("Craft", "subject"),
    "Prose and editing": ("Craft", "mode"),
    "Image generation": ("Craft", "mode"),
    "Interviews and hiring": ("Life", "subject"),
    "Resume and portfolio": ("Life", "subject"),
    "Planning and decisions": ("Life", "mode"),
    "Health and fitness": ("Life", "subject"),
    "Travel": ("Life", "subject"),
    "Money and finance": ("Life", "subject"),
}

TOPIC_SEEDS = {
    # Systems and this project
    "Rust and cargo": {"rust", "cargo", "crate", "crates", "clippy", "rustc", "borrow",
                       "trait", "serde", "lifetime"},
    "SQLite and FTS5": {"sqlite", "fts5", "rowid", "bm25", "contentless", "wal", "pragma",
                        "sql", "schema"},
    "Embeddings and vector search": {"embedding", "embeddings", "vector", "vectors", "cosine",
                                     "semantic", "faiss", "sqlite-vec", "similarity", "ann"},
    "Terminal UI": {"tui", "ratatui", "terminal", "ansi", "curses", "pane", "keybinding", "tmux"},
    "Agents and tooling": {"mcp", "subagent", "harness", "orchestration", "claude-code",
                           "codex", "skill", "workflow"},
    "Git and version control": {"git", "commit", "rebase", "merge", "worktree", "diff",
                                "changelog", "pull-request"},
    "Shell and CLI": {"bash", "shell", "zsh", "stdout", "stdin", "argv", "subcommand", "flag"},
    "macOS and hardware": {"macos", "apple", "homebrew", "laptop", "ssd", "thunderbolt",
                           "m1", "m2", "m3"},
    "Web frontend": {"react", "typescript", "css", "html", "tailwind", "component", "dom", "vite"},

    # Machine learning
    "GPU and CUDA": {"gpu", "cuda", "nvidia", "vram", "triton", "flops", "h100", "a100", "kernel"},
    "Deep learning training": {"training", "gradient", "optimizer", "epoch", "finetune",
                               "fine-tuning", "checkpoint", "overfit", "loss"},
    "Transformers and architecture": {"transformer", "attention", "softmax", "residual",
                                      "positional", "logits", "decoder", "encoder"},
    "Reinforcement learning": {"reinforcement", "reward", "ppo", "rlhf", "episode", "rollout",
                               "bandit", "q-learning"},
    "LLM evaluation": {"eval", "evals", "benchmark", "judge", "rubric", "hallucination",
                       "baseline", "ablation"},
    "AI safety and alignment": {"alignment", "jailbreak", "adversarial", "misalignment",
                                "deceptive", "interpretability", "red-team", "refusal"},

    # Craft and work
    "Security and exploits": {"exploit", "vulnerability", "malware", "cve", "injection",
                              "credential", "threat", "attacker"},
    "Data analysis": {"pandas", "dataframe", "csv", "matplotlib", "regression", "histogram",
                      "correlation"},
    "Math and proofs": {"proof", "theorem", "lemma", "eigen", "calculus", "topology",
                        "probability", "matrix"},
    "Technical writing and docs": {"readme", "adr", "spec", "documentation", "docstring",
                                   "changelog", "docs"},
    "Prose and editing": {"draft", "essay", "tone", "rewrite", "prose", "voice", "paragraph",
                          "editing", "style"},
    "Image generation": {"midjourney", "dalle", "diffusion", "render", "illustration", "logo",
                         "aspect-ratio", "image"},

    # Life
    # No "offer" or "screen": both are ordinary English, and they put "Interviews and
    # hiring 50" on chat-search — a TUI project talks about screens constantly. A seed
    # word shared with the general vocabulary manufactures membership the same way a
    # seed word shared with another topic manufactures overlap.
    "Interviews and hiring": {"interview", "interviewer", "recruiter", "candidate",
                              "hiring", "onsite"},
    "Resume and portfolio": {"resume", "portfolio", "linkedin", "cover-letter", "application"},
    "Planning and decisions": {"decision", "tradeoff", "roadmap", "milestone", "priority", "plan"},
    "Health and fitness": {"workout", "fitness", "sleep", "nutrition", "running", "injury",
                           "diet", "health"},
    "Travel": {"flight", "hotel", "itinerary", "visa", "airbnb", "trip", "travel"},
    "Money and finance": {"budget", "salary", "tax", "invest", "equity", "mortgage", "expense"},
}


def seeded_topics(convs):
    """Membership by seeded keyword, display terms by what the members actually say.

    The seeds decide who is in; the card shows the members' own strongest terms, so it
    reports what the group turned out to be rather than echoing the query that found it.
    A topic you cannot explain is a topic you cannot accept or dismiss.
    """
    bags = []
    for c in convs:
        text = (c.get("title") or "") + " "
        text += " ".join(m.get("x", "") for m in c["msgs"] if m["k"] == "p" and m["r"] == "u")
        bags.append(Counter(tokens(text)))

    df = Counter()
    for b in bags:
        df.update(b.keys())
    n = len(bags) or 1

    out = []
    tagged = set()
    for name, seed in TOPIC_SEEDS.items():
        members = [i for i, b in enumerate(bags) if seed & set(b)]
        if len(members) < 2:
            continue
        tagged.update(members)

        # A seeded topic explains itself by the seeds that actually hit, ranked by how
        # many members each one reached. Mining "distinctive" terms was the wrong instinct
        # here: that is what you do for a *discovered* cluster, where nobody knows why the
        # group exists. These groups have a stated reason, and on this sample 65 of 138
        # conversations match three or more topics, so there is very little distinctive
        # vocabulary to find anyway.
        seed_hits = Counter()
        for i in members:
            for w in seed & set(bags[i]):
                seed_hits[w] += 1
        terms = [f"{w} {c}" for w, c in seed_hits.most_common(5)]


        grp, facet = TOPIC_GROUPS.get(name, ("Other", "subject"))
        out.append({
            "name": name,
            "group": grp,
            "facet": facet,
            "seed": sorted(seed),
            "matched_seed_terms": [w for w, _ in seed_hits.most_common(8)],
            "terms": terms,
            "members": [convs[i]["id"] for i in members],
        })

    out.sort(key=lambda t: -len(t["members"]))
    untagged = [convs[i]["id"] for i in range(len(convs)) if i not in tagged]
    # Diagnostics that reflect what actually goes wrong. Counting how many topics a
    # conversation matches was the wrong measure — multi-membership is the point of
    # tags. What matters is whether topics divide the space: a pair with high Jaccard
    # is one topic wearing two names, and a topic covering a quarter of the corpus is
    # not a filter.
    sets = {t["name"]: set(t["members"]) for t in out}
    worst = 0.0
    worst_pair = None
    names = list(sets)
    for i, a in enumerate(names):
        for b in names[i + 1:]:
            u = len(sets[a] | sets[b])
            if not u:
                continue
            j = len(sets[a] & sets[b]) / u
            if j > worst:
                worst, worst_pair = j, [a, b]
    sizes = sorted(len(v) for v in sets.values()) or [0]
    return out, {
        "untagged": len(untagged),
        "untagged_ids": untagged,
        "median_topic_share": round(sizes[len(sizes) // 2] / max(len(convs), 1), 3),
        "largest_topic_share": round(sizes[-1] / max(len(convs), 1), 3),
        "worst_overlap": round(worst, 2),
        "worst_overlap_pair": worst_pair,
    }


def sittings(conn, ids, window_ms=2 * 3600 * 1000, max_span_ms=6 * 3600 * 1000):
    """Real sittings: conversations close in ended_at, bounded in total span.

    The self-join the design argued for, run against the corpus rather than asserted —
    and immediately corrected by it. A gap rule alone chains: every consecutive pair
    being under two hours apart merged eighteen conversations spanning a whole day into
    one "sitting". A sitting has to be bounded in total span as well as in gap, or the
    word stops meaning anything.
    """
    cur = conn.cursor()
    qmarks = ",".join("?" * len(ids))
    rows = cur.execute(
        f"""SELECT id, source, title, ended_at, msg_count FROM conversation
            WHERE id IN ({qmarks}) AND ended_at IS NOT NULL
            ORDER BY ended_at""", ids).fetchall()
    groups, cur_group = [], []
    for r in rows:
        too_far = cur_group and r[3] - cur_group[-1][3] > window_ms
        too_long = cur_group and r[3] - cur_group[0][3] > max_span_ms
        if too_far or too_long:
            groups.append(cur_group)
            cur_group = []
        cur_group.append(r)
    if cur_group:
        groups.append(cur_group)

    out = []
    for g in groups:
        if len(g) < 3 or len({r[1] for r in g}) < 2:
            continue  # a sitting worth showing crosses at least two tools
        out.append({
            "start": g[0][3], "end": g[-1][3],
            "items": [{"id": r[0], "source": r[1], "title": r[2],
                       "ended_at": r[3], "msg_count": r[4]} for r in g],
        })
    out.sort(key=lambda s: -len(s["items"]))
    return out[:8]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--db", default=DEFAULT_DB)
    ap.add_argument("--limit", type=int, default=160)
    ap.add_argument("--per-project", type=int, default=24,
                    help="depth to sample from each of the ten largest projects")
    ap.add_argument("--out", default=OUT)
    a = ap.parse_args()

    conn = connect(a.db)
    projs = corpus_projects(conn)
    biggest = sorted(projs, key=lambda p: -p["n"])
    ids = pick(conn, a.limit, biggest, a.per_project)
    convs = load_conversations(conn, ids)

    basenames = {p["path"].split("/")[-1]: p["path"] for p in projs}
    for c in convs:
        c["project"] = project_of(c.get("cwd"), basenames)
        # Relativise and re-merge. The same file arrives both absolute and relative
        # depending on how the tool was called, so `docs/DECISIONS.md` and
        # `Users/…/docs/DECISIONS.md` were two entries for one file.
        if c["project"]:
            prefix = c["project"].rstrip("/") + "/"
            merged = Counter()
            for f in c["files"]:
                for pre in (prefix, prefix.lstrip("/")):
                    if f.startswith(pre):
                        f = f[len(pre):]
                        break
                # A worktree is the project it was cut from, so its paths are the
                # project's paths; leaving the prefix on made every chip open with
                # `.claude/worktrees/...` and pushed the filename out of view.
                f = re.sub(r"^\.?claude/worktrees/[^/]+/", "", f)
                if HARNESS.search(f):
                    continue
                merged[f] += 1
            c["files"] = list(merged)[:12]
    carried = Counter(c["project"] for c in convs if c["project"])
    for p in projs:
        p["sampled"] = carried.get(p["path"], 0)
    topics, coverage = seeded_topics(convs)
    sits = sittings(conn, [c["id"] for c in convs])
    lins = lineages(convs)
    allfiles = Counter(f for c in convs for f in c["files"])

    corpus = conn.execute(
        "SELECT COUNT(*), SUM(msg_count) FROM conversation WHERE deleted_upstream_at IS NULL"
    ).fetchone()

    payload = {
        "generated_from": os.path.basename(a.db),
        "corpus": {"conversations": corpus[0], "messages": corpus[1]},
        "conversations": convs,
        "topics": topics,
        "projects": projs,
        "lineages": lins,
        "top_files": [{"path": f, "n": n} for f, n in allfiles.most_common(60)],
        "coverage": coverage,
        "sittings": sits,
    }
    # A script assigning a global, not a .json file: fetch() is blocked on file:// and
    # the whole point of this prototype is that it opens without a server.
    with open(a.out, "w") as f:
        f.write("window.REAL_DATA=")
        json.dump(payload, f, separators=(",", ":"))
        f.write(";\n")

    size = os.path.getsize(a.out) / 1e6
    msgs = sum(len(c["msgs"]) for c in convs)
    print(f"wrote {a.out}")
    print(f"  {len(convs)} conversations, {msgs:,} messages, {size:.1f} MB")
    withfiles = sum(1 for c in convs if c["files"])
    print(f"  {len(topics)} seeded topics · {len(sits)} cross-tool sittings · "
          f"{len(lins)} lineages · {withfiles} conversations touch a known file")
    inproj = sum(1 for c in convs if c["project"])
    deep = sum(1 for p in projs if p["sampled"] >= 10)
    print(f"  {len(projs)} projects in the corpus ({sum(p['n'] for p in projs)} conversations, "
          f"{100 * sum(p['msgs'] for p in projs) // max(corpus[1], 1)}% of all messages) · "
          f"{inproj} carried here across {len(carried)} of them, {deep} with 10 or more")
    pct = 100 * coverage["untagged"] // max(len(convs), 1)
    print(f"  {coverage['untagged']} match no topic ({pct}%) · "
          f"median topic {coverage['median_topic_share']:.0%} of the sample · "
          f"largest {coverage['largest_topic_share']:.0%}")
    if coverage["worst_overlap_pair"]:
        print(f"  most overlapping pair: {coverage['worst_overlap']} "
              f"{' × '.join(coverage['worst_overlap_pair'])}")
    print(f"  corpus: {corpus[0]:,} conversations / {corpus[1]:,} messages")


if __name__ == "__main__":
    main()
