# chat-search PoC — shared spec

Two implementations (TypeScript, Rust) build the *same* index from the *same* sources
and expose the *same* JSON CLI. Any difference in output is a bug in one of them.

Goal is to measure, not to ship:

- index build throughput over the real corpus (~1.8 GB)
- resulting DB size
- **cold start + query latency** — decides whether the subprocess seam survives type-ahead
- importer ergonomics against messy real-world JSON (subjective, but felt)

## Sources (PoC subset)

Two JSONL sources, deliberately excluding OpenCode's 135k-file layout (measured separately).

| source | glob | native session id |
|---|---|---|
| `codex` | `~/.codex/sessions/*/*/*/rollout-*.jsonl` | `session_meta.payload.session_id` |
| `claude-code` | `~/.claude/projects/*/**/*.jsonl` | `sessionId` field on any event |

## Identity

IDs must be machine-independent and stable across re-imports. No rowid, no content hash,
no local path.

```
conversation.id = "<source>:<native_session_id>"
message.id      = "<conversation.id>:<native_message_id>"
```

`native_message_id` is `uuid` for claude-code.

Codex events have no stable per-event id, so the PoC uses the ordinal within the file.
An ordinal alone is **not** sufficient: a Codex subagent runs in its own rollout file but
shares its parent's `session_id`, so ordinals collide across files belonging to the same
conversation. Measured on this corpus: 13 session_ids span 28 files, and in every case the
extra files are subagents (`thread_source: "subagent"`).

Forks are separate and cleanly declared. A fork gets a new `session_id` plus
`forked_from_id` pointing at its parent (32 on this corpus, all resolvable), so forks
never collide.

So the codex message id is scoped by the rollout file:

```
native_message_id = "<rollout-file-stem>:<zero-padded-ordinal>"
```

The conversation stays keyed on `session_id` (it genuinely is one logical thread), and the
rollout file is the branch. Without this, ~3% of messages collide and which copy survives
depends on directory iteration order. The two implementations disagreed here, which is how
the collision was found.

## Kinds — the prose/tool split

Every message gets exactly one `kind`. Only `prose` is searched by default.

| kind | included in default search |
|---|---|
| `prose` | yes |
| `reasoning` | no |
| `tool_call` | no (opt-in via `--tools`) |
| `tool_result` | no (opt-in via `--tools`) |

### codex → kind

| event | kind | text |
|---|---|---|
| `event_msg` / `user_message` | `prose` (role=user) | `payload.message` |
| `event_msg` / `agent_message` | `prose` (role=assistant) | `payload.message` |
| `response_item` / `reasoning` | `reasoning` | concatenated summary text |
| `response_item` / `function_call` | `tool_call` | `name` + `arguments` |
| `response_item` / `function_call_output` | `tool_result` | flattened `output` |
| `response_item` / `message` | **skipped** | duplicates `event_msg`; role=developer is the 17 KB system prompt |
| everything else | skipped | |

### claude-code → kind

| event | kind | text |
|---|---|---|
| `user`, `isMeta` != true, content string or `text` blocks | `prose` (role=user) | text |
| `assistant`, `text` blocks | `prose` (role=assistant) | text |
| `assistant`, `thinking` blocks | `reasoning` | thinking |
| `assistant`, `tool_use` blocks | `tool_call` | `name` + JSON input |
| `user`, `tool_result` blocks | `tool_result` | flattened content |
| `isMeta: true` | skipped | synthetic system-injected turns |

`parentUuid` → `message.parent_id`, preserving the DAG. `isSidechain` is carried through
so subagent traffic can be filtered.

## Determinism

Two rules, both learned the hard way — the implementations disagreed until each was added.

1. **Sort the discovered file list.** Directory enumeration order is not guaranteed and
   differs between runtimes. Sort on the *string* form of the path: Rust's `Path: Ord`
   compares component-wise, which orders `<id>/subagents/x.jsonl` *before* `<id>.jsonl`,
   while a plain string compare does the opposite.

2. **Merge conversation metadata field-by-field, never first-write-wins.** A conversation
   spans several files (a session plus its subagent sidechains, or a resumed rollout) and
   only some carry the title, cwd or model. Upsert with
   `COALESCE(conversation.col, excluded.col)` so the result does not depend on which file
   is seen first.

## Schema

Identical DDL in both implementations.

```sql
CREATE TABLE conversation(
  id           TEXT PRIMARY KEY,
  source       TEXT NOT NULL,
  native_id    TEXT NOT NULL,
  title        TEXT,
  cwd          TEXT,
  git_branch   TEXT,
  model        TEXT,
  started_at   INTEGER,          -- epoch ms
  ended_at     INTEGER,
  msg_count    INTEGER NOT NULL DEFAULT 0,
  prose_count  INTEGER NOT NULL DEFAULT 0,
  forked_from  TEXT,             -- codex forked_from_id
  resume_cmd   TEXT
);

CREATE TABLE message(
  id           TEXT PRIMARY KEY,
  conv_id      TEXT NOT NULL,
  parent_id    TEXT,
  seq          INTEGER NOT NULL,
  role         TEXT NOT NULL,
  kind         TEXT NOT NULL,
  ts           INTEGER,
  is_sidechain INTEGER NOT NULL DEFAULT 0,
  text         TEXT NOT NULL
);

CREATE INDEX idx_message_conv ON message(conv_id, seq);

-- contentless: index only, join back to message by rowid
CREATE VIRTUAL TABLE fts_prose USING fts5(
  text, content='', tokenize="porter unicode61 remove_diacritics 2");
CREATE VIRTUAL TABLE fts_tools USING fts5(
  text, content='', tokenize="porter unicode61 remove_diacritics 2");
```

Both FTS tables are populated with `rowid` = `message.rowid`.

## CLI contract

Both binaries accept the same argv and emit the same JSON on stdout. Non-zero exit on error,
message on stderr.

```
<bin> index [--db PATH] [--limit N]      -> {"conversations":N,"messages":N,"prose":N,"ms":N,...}
<bin> search QUERY [--db PATH] [--limit N] [--tools]
                                          -> {"query":..,"ms":N,"results":[Result,...]}
<bin> stats [--db PATH]                   -> {"conversations":N,"messages":N,"by_source":{..},..}
```

```jsonc
// Result
{
  "conv_id": "codex:019f...",
  "msg_id":  "codex:019f...:000123",
  "source":  "codex",
  "title":   "…",
  "role":    "user",
  "kind":    "prose",
  "ts":      1785257068000,
  "score":   -8.42,          // raw bm25; lower is better
  "snippet": "…",
  "resume_cmd": "codex resume 019f…"
}
```

Ranking for the PoC is plain BM25 with a field-independent time decay applied in SQL, so
both implementations sort identically:

```
rank = bm25(fts) * (1.0 + 0.3 * (age_days / 365.0))
```

bm25 is negative (better = more negative), so multiplying by a factor > 1 for older
documents pushes them down. Deliberately simple — real weighting comes later.

## Out of scope for the PoC

Embeddings, OpenCode/Gemini importers, the ChatGPT export, incremental re-index,
redaction, and the authored-data log. All deferred on purpose. This measures the
runtime floor rather than the product.
