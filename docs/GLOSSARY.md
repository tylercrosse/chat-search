# Glossary

The vocabulary this project uses, and how it maps onto what each vendor calls things.

This exists because during design we called Codex subagent threads "resumes" and "forks." The wrong name propagated into the spec, both PoC implementations, and several recommendations before the data contradicted it — the code was internally consistent with the wrong word, so nothing caught it. Four distinct mechanisms in these sources all look like "a conversation branching," and they need four names.

Terms are marked **[schema]** where they correspond to a table, column, or enum value.

---

## Core model

**Conversation** **[schema]** The logical thread of exchange we key on and search over. Identified as `<source>:<native_session_id>`. One conversation may span several files and contain several threads. This is the unit a search result points at and the unit a user opens.

**Thread** **[schema, planned]** A single linear stream of messages inside a conversation. Every conversation has one _main thread_; each subagent invocation adds another. This is the level that maps one-to-one with a transcript file for Codex and Claude Code, which is why "one file, one conversation" is wrong.

**Message** **[schema]** One immutable node. Never updated in place; corrections arrive as new messages. Carries `parent_id`, making the messages of a conversation a DAG rather than a list.

**Kind** **[schema]** What a message _is_, as distinct from who sent it. Exactly one of:

| kind          | searched by default | share of corpus text |
| ------------- | ------------------- | -------------------- |
| `prose`       | yes                 | **8.4%**             |
| `reasoning`   | no                  | 0.5%                 |
| `tool_call`   | no — opt-in         | 15.4%                |
| `tool_result` | no — opt-in         | 75.7%                |

The split exists because tool traffic is 91% of the text. Indexing it alongside prose means BM25 ranks tool output against the content you actually wanted.

`reasoning` comes **entirely from Codex**. Claude Code persists no reasoning text: every one of its `thinking` blocks is `{"thinking": "", "signature": "…"}` — the text is blanked and only an encrypted signature is stored, so those messages fall out at the empty-text rule.

**Head** **[schema]** The currently-selected leaf message of a conversation — `conversation.head_id`. The one mutable field in the entire schema. "The conversation as displayed" is the walk from `head_id` up through `parent_id` to the root. Messages not on that path still exist and are still searchable; they are on abandoned branches.

---

## The four branching mechanisms

These are genuinely different and must not be collapsed.

| term | what happens | new session id? | same file? |
| --- | --- | --- | --- |
| **Fork** | a new conversation derived from another at a point in time | **yes** | no |
| **Subagent thread** | a child agent runs its own thread inside the parent conversation | no — _shares_ parent's | no |
| **Sidechain** | Claude Code's name for a subagent thread | no — _shares_ parent's | no |
| **Edit-branch** | user edits/regenerates a message; a sibling node appears under the same parent | no | yes |

**Fork** **[schema: `conversation.forked_from`]** Cross-conversation lineage. Codex declares it: a fork gets a fresh `session_id` plus `forked_from_id` naming its parent. Measured: 32 forked rollouts, every parent resolvable on disk. Forks never collide, because the id is new.

**Subagent thread** / **Sidechain** **[schema: `message.is_sidechain`]** Intra-conversation, parallel, _not_ a branch of the main thread's DAG. Both tools spawn a separate transcript file that reuses the parent's session id:

- Codex — `thread_source: "subagent"`, and `source.subagent.thread_spawn.parent_thread_id` names the parent. Measured: 13 session ids spanning 28 files.
- Claude Code — file lives at `<sessionId>/subagents/agent-*.jsonl`, carries the parent's `sessionId`, and sets `isSidechain: true` on every message. Measured: 31 files.

This shared session id is why per-file message ordinals collide (7,637 messages, 5% of the corpus) and why message ids must be scoped by transcript file.

**Edit-branch** Intra-conversation, intra-file. Editing a ChatGPT message does not overwrite it — a new node appears under the same parent and the old one is retained. Measured across 2,011 conversations: **287 branch points** (nodes with more than one child) in 234 conversations, leaving **869 message nodes off the `current_node` path**. This is why the model is a DAG: it turns the hardest-looking mutation into an append.

---

## Storage

**Raw archive** The immutable, append-only capture of source transcripts. The source of truth. Never rewritten. Namespaced by machine (`raw/<machine>/<source>/…`) so multiple machines can merge. Syncing this — never a database file — is how multi-machine works.

**Index** **[`index.db`]** Disposable. A pure function of _(raw archive, importer version)_. Deleting and rebuilding it must always be safe; a full rebuild is ~7s for 1.8 GB. Anything that cannot survive `rm index.db` is in the wrong file.

**Library** **[`library.db`]** Precious, tiny, backed up. Holds only **authored** data as an append-only event log. Merging two machines is concatenate-and-fold with last-write-wins per key.

**Derived** vs **Authored** The central invariant: every mutable thing is one or the other, never both and never neither. Derived state is recomputed on rebuild and never merged. Authored state is appended and never overwritten.

**Tombstone** **[schema: `conversation.deleted_upstream_at`]** Marks that a conversation is gone from its source — you deleted it upstream, or retention pruned it. The archive _keeps_ the content; outliving upstream deletion is the point. Distinct from forgetting.

**Forget** The only deliberately destructive operation: remove a conversation from raw _and_ index, and record a tombstone so the next scan does not resurrect it from a source file that still exists.

---

## Pipeline

**Source** One upstream tool in one location — `codex`, `claude-code`, `opencode`, `gemini-cli`, `chatgpt-export`. Each has one importer.

**Archiver** Copies raw transcript bytes from live source directories into the raw archive, append-only, and records what it saw in the manifest. **Deliberately dumb: it never parses content.** It knows paths, sizes, mtimes, hashes and per-source layout policy — nothing about conversations or messages. One per source _location_.

If the archiver fails, data is lost permanently, which is why it is kept simple and why it can ship complete before any importer works.

**Importer** Pure function: archived transcript bytes → normalized records. Understands exactly one source's format — conversations, messages, kinds, lineage. No database, no search, no knowledge of where the bytes came from. One per source _format_. Testable with golden files, which is what keeps it cheap to test and portable across a stack change.

The split from the archiver is what makes **retroactive reparse** possible: fix an importer and rebuild, and every conversation back to the first captured byte gets the improvement. Fused, improvements would only ever apply going forward.

**Indexer** Consumes normalized records and writes `index.db` — tables, FTS5, and later vectors. Bound tightly to search (they share the tokenizer, schema and ranking), so the two are one module.

**Archive reader** Thin layer between archive and importer that yields `(logical_path, bytes)` regardless of whether a source is stored mirrored or bundled. Keeps layout policy (ADR 18) from leaking into importers — an importer always sees the source's original shape.

**Normalized record** The contract between importers and the indexer. A type in code; serialized to JSONL only as test fixtures, so the seam stays language-portable without paying to materialise it in production.

**Manifest** Append-only event log inside the archive recording what the archiver observed: `{ts, op: seen|appended|rewritten|vanished, source, path, size, mtime, prefix_hash}`. Holds the facts that cannot be recomputed from a snapshot — `first_seen` and when a file vanished upstream — which is why it lives in the archive rather than the disposable index.

**Bundle** Append-only JSONL file packing many small source fragments into one, for sources like OpenCode that store 170k tiny files. One line per fragment: `{"path": …, "content": …}`. Lossless and reversible; grouping is done by path structure alone so the archiver still never parses content.

**Rebuild** vs **Tail** Rebuild reprocesses everything (~7s). Tail processes only appended bytes of recently touched files, for the session you are in right now. Detection is `(path, size, mtime, hash of first 64 KB)`: prefix hash matches and size grew means pure append; prefix hash changed means the file was rewritten and needs full re-import.

**Resume command** **[schema: `conversation.resume_cmd`]** How to reopen a conversation in its native tool — `codex resume <id>`, `claude --resume <id>`, `https://chatgpt.com/c/<id>`. Stored per conversation so every UI surface can "open it somewhere" without knowing anything tool-specific.

---

## Vendor translation

| our term | Codex | Claude Code | ChatGPT export |
| --- | --- | --- | --- |
| Conversation | `session_id` | `sessionId` | conversation `id` |
| Thread (transcript file) | rollout file | session / subagent file | — (single file) |
| Message | `response_item` / `event_msg` | `user` / `assistant` entry | `mapping` node |
| Head | — | — | `current_node` |
| Fork | `forked_from_id` | — | — |
| Subagent thread | `thread_source: "subagent"` | `isSidechain: true` | — |
| Edit-branch | — | — | sibling under one parent |
| Title (authored) | — | `custom-title` event | `title` — origin not recorded |
| Title (generated) | — | `ai-title` event | `title` — origin not recorded |

**Title precedence** is a fold over the append-only log, not a stored value:

```
authored override  >  custom-title  >  ai-title  >  first user message
```

`custom-title` is re-emitted on every save (observed up to 44 times in one session), so last-wins. `ai-title` was never observed to change across 179 sessions that re-emit it.

---

## Non-terms

Words to avoid, because they are ambiguous across sources:

- **"Session"** unqualified — native tools use it for both the conversation and a single process run. Say _conversation_ or _thread_.
- **"Chat"** — fine in UI copy, not in code or schema.
- **"Branch"** unqualified — say _fork_, _subagent thread_, or _edit-branch_.
- **"Resume"** — Codex's subagent files were misread as resumes. If a genuine resume mechanism turns up, name it then, with evidence.
