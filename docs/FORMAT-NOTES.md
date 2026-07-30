# Format notes

Edge cases other people have already hit, gathered 2026-07-28 from parser source, decompiled
product source and maintainer issue threads rather than blog posts.

The point of this file is that the risk in these formats is not parsing difficulty, it is
**unknown unknowns** — quirks that produce a plausible-looking index containing the wrong
thing. Where a claim has been checked against this corpus, the result is marked
**[verified here]** or **[not present here]**.

---

## Checked against our corpus

| claim | status here | action |
| --- | --- | --- |
| Codex zstd-compresses rollouts older than ~7 days | **0 `.zst` files today** — latent, not active | glob widened to `rollout-*.jsonl.zst` before it bites |
| Codex `history_mode: paginated` persists messages differently | **not present** (584 pre-mode, 90 `legacy`) | none |
| Claude Code writes cumulative chunks under one `message.id` | 2,029 multi-entry ids but **0 prefix-extensions** | none; revisit if duplicates appear |
| ChatGPT export sharded into `conversations-NNN.json` | **verified here** — 21 shards | already handled |
| Claude desktop keeps no conversations locally | **false here** — 30 MB of plaintext `audit.jsonl` | new source, see below |
| ChatGPT desktop store is decryptable with a derived key | **false** — Keychain access group, not a password | closed; export is the only route |
| The Codex merge moved chats to a new local store | **not present** — `~/Documents/Codex/` is artifacts only | none |

---

## Codex

- **Rollouts are zstd-compressed after ~7 days** by a background worker
  ([`codex-rs/rollout/src/compression.rs`](https://github.com/openai/codex/blob/main/codex-rs/rollout/src/compression.rs)).
  Plain and `.zst` siblings coexist mid-transition and **plain wins**. Glob `*.jsonl` and you
  lose everything older than a week; glob `*.jsonl*` and you double-import.
- **Three format generations.** Pre-2025-09-09 files have no envelope at all — a bare
  `{id, timestamp, instructions}` header plus bare `ResponseItem`s. Codex itself still cannot
  read them ([#26877](https://github.com/openai/codex/issues/26877), open). Disambiguate on
  the presence of a `payload` key. **18 such files here, currently skipped.**
- **Two current history modes.** `history_mode` ∈ `legacy` | `paginated`. Paginated wraps
  content in `event_msg → item_completed → TurnItem` rather than `user_message`/
  `agent_message`. A legacy-only parser extracts *zero* messages from paginated files.
- **Forks denormalize**: a fork copies the parent's entire transcript into the child file.
  Measured elsewhere at 16,414 physical sessions vs 8,807 logical roots. Detect via
  `forked_from_id`, never by content.
- **Multiple `session_meta` lines per file** — the first is canonical. Taking the last
  assigns the parent's identity to the child (25 of 32 files here).
- `compacted.payload.replacement_history` repeats `ResponseItem`s already in the file.
- `ThreadRolledBack` means the newest N user turns were logically discarded but remain on
  disk; indexing them presents retracted work as real.
- User messages carry an injected preamble; real text starts after `## My request for Codex:`.
- **Rust-specific:** `#[serde(flatten)]` over a tag/content enum makes
  `from_str::<RolloutLine>` reject lines that `from_str::<Value>` → `from_value` accepts.
  Two readers inside Codex still lose records to this
  ([#35746](https://github.com/openai/codex/issues/35746)). Deserialize value-first.
- Binary blobs inline (`data:image/png;base64`, `encrypted_content`); a 10.2 GB single
  rollout is reported. Set an explicit max line size.

## Claude Code

Best source: [jeremedia/claude-code-log-format](https://github.com/jeremedia/claude-code-log-format),
written against decompiled Claude Code 2.1.88.

- **Cumulative *and* additive chunks share one `message.id`.** Some later entries are a
  prefix-extension of earlier text, others are genuinely new blocks. Concatenating duplicates
  prose 2–5×; taking only the last drops content. Correct merge prefix-matches per block and
  resets at `stop_reason="end_turn"`.
- **`logicalParentUuid`** is a second parent pointer preserved across compaction boundaries.
  Ignoring it breaks chain reconstruction through a compact.
- **`type=attachment` with `attachment.type=queued_command`** — user prompts typed while a
  tool was running. **No `uuid`/`parentUuid`**, so they cannot join the DAG and must be
  spliced by timestamp. We currently skip attachments, so these are lost.
- **Many metadata record types** omit the standard envelope entirely: `summary`,
  `last-prompt`, `task-summary`, `tag`, `agent-name`, `pr-link`, `worktree-state`,
  `content-replacement`, `queue-operation` and more. A parser requiring `uuid` drops or throws.
- **`summary` records are not first-line-only** — observed at lines 393 and 1441, and
  backfilled retroactively by a startup task.
- **`toolUseResult` duplicates the `tool_result` content block — ~60% of file size.** Index
  both and you double-count. (We read only the content block.)
- Real corruption exists: duplicate `uuid`s, orphan `parentUuid`s, whitespace-only text blocks
  before extended thinking. Two independent repair tools exist.
- **Truncation heuristic**: last line invalid JSON **and** no trailing newline → truncated
  write. A newline-terminated invalid line is a complete malformed record.
- More injected markers to strip: `<local-command-caveat>`, `<local-command-stdout>`,
  `<bash-input>`, `<bash-stdout>`, `<user-prompt-submit-hook>`, `[Request interrupted…`,
  and skill-loading blocks starting `Base directory for this skill:`.
- Filename ≠ `sessionId` for resumed sessions.
- Tool results arrive **out of order**; match by `tool_use_id`, never position or timestamp.
  Timestamps are not unique — multiple events share a millisecond.

## ChatGPT export

- **Sharded since ~2026** into `conversations-000.json … NNN.json`. A parser globbing
  `conversations.json` imports zero from a 2026 export.
- **Content types beyond `text`**, most without `parts`: `code` (uses `text` + `language`),
  `execution_output`, `multimodal_text`, `tether_quote`, `tether_browsing_display`,
  `thoughts`, `reasoning_recap`, `user_editable_context`, `sonic_webpage`, `system_error`,
  `image_asset_pointer`, `audio_transcription`. Each indexes as empty if you only read `parts`.
- **Custom instructions are not in `content`** — the real text is
  `metadata.user_context_message_data.{about_user_message, about_model_message}`, affecting
  85%+ of conversations.
- **Canvas documents arrive as a JSON string inside `parts`** when
  `recipient == "canmore.create_textdoc"` and must be re-parsed.
- `current_node` can be null, absent, or dangling. Robust handling walks it with a seen-set
  and falls back to sorting all message-bearing nodes by `create_time`.
- **Duplicate `message.id` across distinct nodes occurs.**
- Roles include `memory` beyond user/assistant/system/tool.
- `weight` (1.0 = active) is weakly attested — treat as a hint, not a discriminator. The only
  reliable active-branch signal is the `current_node` → parent chain.
- Streaming JSON parsers emit `create_time` as `Decimal`; a strict type guard rejecting it
  caused another project to silently exclude entire export ZIPs
  ([PR #1871](https://github.com/Sinity/polylogue/pull/1871)).
- One real export ZIP was **2,068,415,407 bytes** — over 2³¹, so ZIP64 is required, and
  `conversations.json` is minified to one enormous line so line-oriented streaming is useless.
- The 2026 export is a **strict subset** of the older shape and pre-trims hidden system nodes,
  so the same conversation re-exported later has a different node count — content-hash
  idempotency needs care.
- Other conversation-bearing files most parsers ignore: `shared_conversations.json`,
  `model_comparisons.json`. `message_feedback.json` and `user.json` are **not**
  conversation-bearing.

## OpenCode

- **Three storage generations**, the newest being SQLite (drizzle) since ~v1.2.0, 2026-02-14.
  Message and part payloads stay as JSON inside a `data` column.
- **Migration copies, never deletes** — the old tree stays on disk, so broad globbing
  double-imports every pre-migration session. Migration can half-fail without advancing its
  marker.
- **Session ids sort backwards**: `ses_*` uses a bit-inverted timestamp (descending) while
  `msg_*`/`prt_*` ascend. Sorting sessions by id gives newest-first, and naive timestamp
  extraction yields garbage.
- Root is XDG (`~/.local/share/opencode`) **even on macOS**; DB path is channel-dependent.

## Gemini CLI

- One JSON *document* per file, not JSONL, **but appended to while the session is live**, so a
  scan races partial trailing writes — tolerate a malformed tail.
- Records with a repeated id **replace** the earlier one in place.
- Inline tool results live at `result[].functionResponse.response.output`.

## Claude desktop — local agent mode

Surveyed 2026-07-30 on this machine. The Cowork and Chat tabs write full transcripts to disk in
the clear; only the cloud-synced claude.ai conversations are absent. Earlier notes concluded
"needs export or API" after looking at IndexedDB alone, which is the wrong directory.

- Root is `~/Library/Application Support/Claude/local-agent-mode-sessions/<account>/<org>/`,
  nested by **account uuid then org uuid** — a two-level prefix before anything session-shaped.
  A glob anchored one level too shallow finds nothing.
- Each session is a **pair**: `local_<uuid>.json` (metadata) and `local_<uuid>/` (working
  directory). The transcript is `local_<uuid>/audit.jsonl`; the sibling `.json` holds `title`,
  `cwd`, `model`, `createdAt`, `lastActivityAt`, `initialMessage`, `systemPrompt`. **Neither is
  sufficient alone** — content without a title, or a title without content.
- **Not every session has an `audit.jsonl`.** 165 JSON files here yield only 14 transcripts;
  the rest are settings, plugin manifests and caches. Decline by shape, the way `gemini.rs`
  declines `logs.json`, rather than trusting the glob.
- Shape is **Claude Agent SDK stream-json, not Claude Code's JSONL**, and the divergence is
  snake_case: `session_id`, `parent_tool_use_id` here vs `sessionId`, `parentUuid` there.
  `type` ∈ `user` | `assistant` | `system` | `result` | `tool_use_summary` | `rate_limit_event`.
  Close enough to invite a glob widening, different enough that one would silently import zero.
- `parent_tool_use_id` is **not** a message-to-message parent pointer — it marks a subagent's
  messages as belonging to a tool call. There is no DAG here; the file is linear. Feeding it to
  `parent_native_id` would break `on_head_path`, which requires a parent the importer emitted.
- The working directory also holds `uploads/`, `outputs/` and plugin trees. Only `audit.jsonl`
  is conversation-bearing.
- The audit log is **HMAC-chained**. Nothing here needs to verify it, but it means the format
  is intended as tamper-evident and is unlikely to be rewritten in place.

## Claude.ai — export and internal API

- **Export**: Settings → Privacy → Export data, emailed, **link expires 24 h**. Minutes of
  latency, not hours. Whole-account snapshot, no incremental option, no scheduling.
- ZIP holds `conversations.json`, `projects.json`, `memories.json`, `users.json`.
  `message_feedback`-style files are absent; all four of these are worth reading except `users`.
- **Projects have no join key to conversations.** `projects.json` and `conversations.json`
  cannot be related without heuristics. Treat project membership as unavailable, not as a
  parsing problem to solve.
- **Attachment binaries are not in the export** — metadata only.
- `conversations.json` routinely exceeds 100 MB; it is one document, so stream it.
- Message shape: `{uuid, name, created_at, updated_at, chat_messages: [{uuid, sender:
  "human"|"assistant", text, created_at, attachments, files}]}`. Note `sender`, not `role`, and
  `human`, not `user`.
- **Internal API** (what the desktop app itself calls, read out of its own IndexedDB cache):
  `GET /api/organizations/<org>/chat_conversations` to list, then
  `?tree=True&rendering_mode=messages` per conversation for the full tree. `?tree=True` returns
  **branches the UI does not show**, so it is strictly richer than the export.
- Org id comes from the `lastActiveOrg` cookie; auth is `sessionKey`. Both live in the desktop
  app's own `Cookies` DB (Chromium Safe Storage, so Keychain-wrapped but derivable).
- **The rate limits are the real constraint**, not the parsing. `__cf_bm` rotates every ~30 min
  and `sessionKey` expires monthly. Prior art settles on batches of 5 with 500 ms between them,
  exponential backoff on 429, and Chrome TLS impersonation to survive Cloudflare's JA3
  fingerprinting — plain HTTP clients get challenged.

## Gemini Apps (Takeout)

- **Two Takeout products are named "Gemini" and the obvious one is wrong.** The top-level
  *Gemini* product exports Gems configuration. Conversations are under **My Activity → Gemini
  Apps**, landing at `Takeout/My Activity/Gemini Apps/MyActivity.json`.
- **JSON is not the default** — My Activity records export as HTML unless the format is changed
  explicitly in Takeout's per-product options.
- **It is an activity log, not a conversation tree.** Each record is one prompt plus one
  response plus a timestamp. There is no branching, and **no stable conversation identifier** —
  which is a genuine modelling problem, not a parsing one. ADR 2's stated revisit trigger is
  *"a source appears whose native ids are not stable across exports"*; this is that source.
  Grouping into conversations has to be synthesised, and a synthesised key must not become part
  of an id (ADR 16).
- **Schedulable**: "every 2 months for 1 year" delivered to **Google Drive**, which is the only
  automatable route any of these three surfaces offers. Email links expire in ~7 days and allow
  5 downloads; scheduling is unavailable under Advanced Protection.
- **Retention bounds the ceiling**: Gemini Apps Activity auto-deletes at 18 months by default.
  Anything older than the retention window was never exportable, so a gap there is not a bug.
- Google AI Studio is a **different source** with a better route — prompts save as JSON into a
  Drive folder literally named `Google AI Studio`, shaped as `chunkedPrompt.chunks`. Reachable
  live via the Drive API with no export step. It does not cover gemini.google.com.
- **No API exists for the Gemini app's history.** Verified absent from the Data Portability
  API's scope list — there is no `myactivity.gemini`, and the Gemini API's own interaction
  state is a different thing entirely.

## ChatGPT desktop — why the local store is out of reach

Recorded so this is not investigated a third time.

- `~/Library/Application Support/com.openai.chat/conversations-v3-<uuid>/*.data`, one file per
  conversation, 204 of them / 18 MB here. Confirmed high-entropy with no magic bytes.
- AES-256-GCM: 12-byte nonce, ciphertext, 16-byte tag.
- **The key sits behind a Keychain *access group*** (`2DC432GLL2.com.openai.shared`), not behind
  a `Safe Storage` password. This is the whole difference. Chrome's cookie key is a password
  that any process can read and stretch with PBKDF2; an access-group item is readable only by a
  binary code-signed with OpenAI's Team ID. **There is nothing to reimplement.**
- The v1 store was plaintext until July 2024, when it was encrypted after public disclosure.
  Anything written since is unreachable without a user-consented manual key export.
- `cass` is sometimes described as reading this store. It does not — it requires the user to
  supply `CHATGPT_ENCRYPTION_KEY` themselves.
- **The 2026-07-09 Codex merge did not change this.** The merged app writes artifacts to
  `~/Documents/Codex/<date>/<title-slug>/` — `outputs/`, `work/`, `research/` — with no
  transcript among them. The slug and date leak that a conversation happened, and nothing else.
  ChatGPT Atlas is discontinued 2026-08-09, so its Chromium profile is not worth targeting.

---

## A design idea worth stealing

Polylogue separates **`role`** (provider envelope truth) from **`material_origin`**
(authoredness): `human_authored`, `assistant_authored`, `operator_command`,
`runtime_protocol`, `runtime_context`, `tool_result`, `generated_context_pack`. Their reason:
*"`role` must not be used as a proxy for human-authored prose — Claude Code carries command
wrappers, task notifications, provider-generated context bundles, and tool-result protocol
envelopes through `role=user`."*

That is two orthogonal axes where this project currently has one. Our `Kind` conflates *what
a message is* with *who authored it*, and the `environment_context` and `isMeta` cases are
exactly the collision.

## Not found by the research

- No documented case of an actual **cycle** in a real ChatGPT `mapping` — parsers guard
  defensively, agentsview does not, and nobody could confirm cycles occur.
- No semantics for `message.channel` (`analysis`/`commentary`/`final`).
- No firm semantics for `weight`.
- **No parser anywhere handles the pre-2025-09-09 Codex format** — that would be from scratch.
- **No schema version field in any of these formats.** Generation must be sniffed
  structurally. Codex has an internal repo rule titled "Tell codex to avoid changing rollout
  format" — treated as fragile, not as a contract.
