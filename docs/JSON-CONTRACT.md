# `cs search --json`

The contract a non-Rust client decodes. ADR 14 made it load-bearing: every surface that is not
a Rust program spawns `cs search --json` and reads this, so a field's shape is part of the
interface whether or not anyone wrote it down.

This file exists because it was not written down. The Swift spike (`chat-search-me9.22`) was the
first client to decode this without reading the structs that produce it, typed `title` as a
non-optional `String` — the obvious choice, and one that passes every hand test, because no
untitled conversation appears in the first ten rows of anything — and threw at `results[54]` of
a `--limit 60` query. Exactly the page size a GUI asks for, and exactly the size nobody checks
by hand (`chat-search-me9.27`).

**The rule this file is really for: nullability is part of the contract.** A key that is
sometimes null, sometimes absent and sometimes present is three different types to a decoder,
and the difference is invisible until a client hits a row that exercises it. So every field
below says which of the three it is, and `crates/cs/tests/json_contract.rs` fails if the answer
changes.

Changes here are additive (ADR 12). Adding a key is safe; making a non-null key nullable, or a
nullable key absent, is not, and the test is what makes that a decision rather than an accident.

> Counts below are from the reference corpus on this machine, 3,059 conversations as of
> 2026-08-04. They say how *rare* a state is, never whether it can happen — a client must
> handle every nullable field regardless of how few rows exercise it today. Rarity is the
> hazard here, not the reassurance.

---

## The envelope

One JSON object on stdout, pretty-printed. Exit status is part of the interface: a nonzero exit
means no JSON was produced and the reason is prose on stderr (`chat-search-me9.29` is the bead
for making that machine-readable; until it closes, a client cannot classify failures except by
substring-matching English).

| key | type | null? | |
| --- | --- | --- | --- |
| `query` | string | never | The query as typed, filter tokens included. Echoed verbatim so a client that fired several can tell which response it is holding. |
| `ms` | number | never | Wall time for the search, rounded to 2dp. Includes opening the index, excludes process start. |
| `count` | integer | never | `results.length`. Not the number of matches in the corpus — the limit truncates before this is taken. |
| `grouped` | `true` | **absent under `--flat`** | Present and always `true` in the default shape. `--flat` omits the key rather than emitting `false`, so a decoder modelling one envelope for both shapes must treat it as optional. See [Known warts](#known-warts). |
| `results` | array | never; may be empty | `Group[]` by default, `Hit[]` under `--flat`. |
| `unapplied_filters` | array of string | never; may be empty | Filter tokens whose *value* selects nothing, e.g. `agent:notathing`. They parsed as filters and were then not applied, so the result set is wider than the query asked for. Non-empty is not an error and the exit status stays 0; a client that ignores this silently shows unfiltered results for a filtered query (`chat-search-6eb.11`). |
| `index_state` | string | never | What is at the index path: `ready` or `rebuilding` on any answered search, and `no_index` or `building` in the error body of one that could not be. **Both answering states mean the results are complete** — since `chat-search-me9.28` a rebuild assembles a sibling and swaps it in whole, so a client never has to wonder whether a thin answer is a partial one. `rebuilding` says only that a newer index is on its way, which is what lets a client offer to ask again rather than presenting this as the last word. Branch on the name, never on the sentence beside it (ADR 12). |

Object keys are emitted in alphabetical order. That is an artefact of how the response is
built, not a promise — decode by name.

## `Group` — one conversation

The default shape: the conversation is the result and its matching messages nest beneath it.

| key | type | null? | |
| --- | --- | --- | --- |
| `conv_id` | string | never | `"<source>:<native_id>"`. Permanent (ADR 2, ADR 16), so it is safe as a client-side key across reindexes. |
| `source` | string | never | The watched location's id: `codex`, `claude-code`, `chatgpt-export`, `gemini-cli`, … It comes from the user's config, so treat it as an open set. |
| `native_id` | string | never | The source's own id. Permanent. |
| `destinations` | array of [`Destination`](#destination) | never; **may be empty** | Every way to reopen this conversation, best first. Empty means this source has no known way to reopen it — see [Empty is not null](#empty-is-not-null). |
| `title` | string | **nullable** | See [The nullable fields](#the-nullable-fields). |
| `ended_at` | integer | **nullable** | Epoch milliseconds of the last message. |
| `ended_date` | string | **nullable** | `ended_at` as a **local** `YYYY-MM-DD`. Rendered by the core on purpose: every client wants the day, and each one that derives it from the epoch value gets its own chance to derive it in UTC and name tomorrow for an evening session. Null exactly when `ended_at` is null. |
| `user_turns` | integer | never | What a human would call a turn — user prose, not raw message count. |
| `msg_count` | integer | never | Every message, tool traffic included. The denominator `match_seqs` positions are relative to. |
| `prose_count` | integer | never | Prose messages only: what a prose search could possibly have matched. |
| `cwd` | string | **nullable** | Working directory, for sources that have one. |
| `score` | number | never | Ranking score, and negative: it is bm25, which is negative here, divided by a recency decay. The list arrives sorted; re-deriving an order from this means reading `search::DECAY` first. Always finite — a message with no timestamp is scored as though it were current rather than producing a NaN, which would reach the wire as `null`. |
| `match_count` | integer | never | Total matching messages, including any not returned in `hits`. |
| `match_seqs` | array of integer | never; may be empty | Where the matches sit, as 0-based message positions, ascending. **Positions into `msg_count`**, which is *not* the coordinate space `kind_runs` uses — `chat-search-me9.25` is that gap. Empty when nothing matched. |
| `kind_runs` | array of [`Run`](#run) | never; **may be empty** | What the conversation is made of, in reading order, run-length encoded. |
| `deleted_upstream` | boolean | never | The conversation is gone from its source and kept here (ADR 9). |
| `hits` | array of [`Hit`](#hit) | never; may be empty | The best matching messages, up to `--nested` (default 3). Empty for an empty query, which returns conversations without matching anything. |

## `Hit` — one message

The elements of `results` under `--flat`, and of `Group.hits` otherwise. Identical in both
places, which is why one decoder serves both.

| key | type | null? | |
| --- | --- | --- | --- |
| `conv_id` | string | never | The conversation this message belongs to. |
| `msg_id` | string | never | `"<source>:<native_id>:<message_native_id>"`. |
| `source` | string | never | As `Group.source`. |
| `native_id` | string | never | The **conversation's** native id, not the message's. |
| `destinations` | array of [`Destination`](#destination) | never; may be empty | As `Group.destinations`. |
| `title` | string | **nullable** | The conversation's title, carried so a flat result renders without a second lookup. Null under exactly the same conditions as `Group.title`. |
| `role` | string | never | `user` · `assistant` · `tool` · `system`. |
| `kind` | string | never | `prose` · `reasoning` · `tool_call` · `tool_result`. `reasoning` is stored and never indexed (ADR 5, `chat-search-8mb`), so it cannot appear in a search result — only in `cs show`. |
| `ts` | integer | **nullable** | Epoch milliseconds. |
| `score` | number | never | As `Group.score`, for this message alone. |
| `snippet` | string | never | A window of the message around what matched, whitespace flattened. May begin with the literal prefix `⟨no match⟩ `, which means the match could not be located in the text and what follows is the head of the message rather than the matching part. It is a label, not content. |
| `snippet_spans` | array of [`Span`](#span) | never; may be empty | Where to highlight, as offsets into `snippet`. Empty says the same thing the `⟨no match⟩ ` prefix says, in the channel a machine reads. |
| `on_head_path` | boolean | never | False when the message sits on a branch that was edited away — still searchable, but not part of the conversation as currently displayed. Only ever false when `--include-off-path` was passed. |
| `is_sidechain` | boolean | never | The message came from a subagent thread, which runs parallel to the main one rather than being an abandoned branch. |
| `thread_key` | string | never | Which thread within the conversation, usually the transcript filename. Opaque — do not parse it. |
| `deleted_upstream` | boolean | never | As `Group.deleted_upstream`. |

## `Destination`

Tagged on `kind`, with a payload that differs by variant — a URL is not a command, and this
type exists to delete the `if cmd.starts_with("http")` guess that stood in for the distinction.

```json
{ "kind": "terminal", "argv": ["codex", "resume", "019f-main"] }
{ "kind": "web", "url": "https://chatgpt.com/c/68e5-…" }
```

| variant | payload | |
| --- | --- | --- |
| `terminal` | `argv`: array of string, never empty | Already split into words. Run it with no shell in between, in `Group.cwd` when there is one; splitting a pre-rendered string on whitespace is the guess this replaced. |
| `web` | `url`: string | Hand it to the platform opener (`open`, `xdg-open`). It is not a program name. |

`kind` is an open set — a new way to reopen a conversation is an additive change. Decode an
unknown variant as "cannot open this", not as an error.

## `Span`

```json
{ "start": 43, "end": 49 }
```

**UTF-8 byte offsets into `snippet`, not character offsets.** Half-open: `[start, end)`. This is
the only sane choice coming out of Rust and it is not what a Swift, Python or JavaScript client
assumes. Read them as `Character` or UTF-16 offsets and every snippet containing an em-dash
mis-highlights, which in this corpus is most of them — the Swift spike found this beside the
nullability defect.

Nothing on the wire says so, which is the difference between this contract and `cs show --json`:
that one emits `mark_offsets: "utf8-bytes"` beside its offsets, precisely because a silently
wrong offset highlights the wrong word rather than failing. `chat-search-me9.33`.

Spans index the *returned string*, ellipsis and any `⟨no match⟩ ` prefix included. Nothing
downstream can re-derive them: the window has been cut out of the message and its whitespace
flattened, and the term that matched need not appear in the query (`commits` marks `Commit`).

## `Run`

```json
["tool", 12]
```

A two-element array rather than an object: `[band, length]`. There are a great many of these —
the list is per result row and the corpus's longest conversation is 2,553 messages — and
`{"band":"tool","n":12}` is four times the bytes for the same two facts. `chat-search-me9.31` is
the open question of whether this payload earns its 35% of a search response at all.

`band` is one of `user` · `agent` · `reasoning` · `tool`. Open set: decode an unknown band as a
run you cannot colour rather than as an error.

The run lengths sum to the **drawn** message count, which is not `msg_count`: successful tool
results are omitted rather than counted, because a strip position a reader cannot click on is a
lie about where they are. So `kind_runs` and `match_seqs` are two different coordinate spaces
and cannot be drawn on one strip without the reconciliation `chat-search-me9.25` is filed for.

---

## The nullable fields

Six of them, and in every case null is a fact about the conversation rather than a gap in the
data. None is being papered over at the source, and that is deliberate: `""` would erase a
distinction the importers spend real code creating, and omitting the key would say "not
applicable" about something that is merely unknown.

### `title` — nothing in this conversation can serve as a name

Null for **11 of 3,059** conversations. Every one is a conversation whose title candidates were
all machinery rather than prose, and the importers decline the title on purpose:

| conversations | what they hold |
| --- | --- |
| 3 × `codex` | Nothing but slash commands: `/status` twice, `/model luna` once. Codex has no rename and no auto-title, so the opening user message is all there is, and `codex::title_from` declines a turn aimed at the harness rather than at the model. |
| 1 × `codex` | An IDE context block whose `## My request for Codex:` section is empty, because the request was in an attached file. Titling it would have produced a Chrome tab list. |
| 6 × `claude-code` | Nothing but slash-command expansions and their `<local-command-stdout>` — `/fast`, `/model`, `/mcp`, `/login`, and one custom command invoked with no arguments. `claude_code::title_candidate` strips the injected markup and finds nothing left; a conversation titled `opus` or `Fast mode OFF` is worse than an untitled one. |
| 1 × `gemini-cli` | Three `system` messages about an auth flow and no user turn at all, so there is no opening message to fall back to. |

The precedence is custom rename → tool-generated → first user message (ADR 8,
`Titles::resolve`), and null means all three were absent. **A client renders this, it does not
repair it.** `cs` prints `(untitled)`; a GUI should be equally explicit, because "this session
was `/status` and nothing else" is true and useful information about the row.

### `ended_at` and `ended_date` — no message ever landed

Null for **2 of 3,059**, always together. Both are ChatGPT export entries titled `New chat` with
zero messages: a conversation opened and abandoned before anything was said. `ended_at` is
`MAX(message.ts)` over the conversation, and no messages means no maximum.

The same null would appear for a conversation whose messages all lacked timestamps, which is why
this reads as "unknown" rather than "not applicable", and why the key is emitted as null rather
than omitted. Those two rows also carry `msg_count: 0` and `kind_runs: []`, so a client
bucketing or sorting by date needs somewhere to put them.

### `cwd` — this source has no such thing

Null for **2,023 of 3,059** — 2,011 ChatGPT and 12 Gemini CLI. Overwhelmingly the common case,
and not missing data: a conversation that happened on a web page has no working directory to
have. Render it as an absence of the concept rather than an absence of the value
(`chat-search-6eb.26`). Derived, so ADR 16 forbids it ever reaching an id.

### `Hit.title` — the conversation's title

Null under exactly the conditions above, for the same 11 conversations.

### `Hit.ts` — the source recorded no timestamp

Never null on the current corpus, and nullable nonetheless: every importer reads it from an
optional field and emits `None` when the record carries none. Ranking already handles it — an
undated message is scored as though it were current rather than infinitely old — and a client
has to as well. This is the field most likely to be typed non-optional on the evidence of a
`SELECT`, and the one where that would be a bet on the corpus rather than on the contract.

## Empty is not null

Four arrays are never null and routinely empty, and empty means something specific in each:

- **`destinations: []`** — this source has no known way to reopen a conversation. All 12
  `gemini-cli` conversations, today. Not "we failed to build one": there is no `gemini --resume`.
  A client should disable its open affordance rather than construct a fallback.
- **`kind_runs: []`** — the conversation has nothing to draw. Only the 2 zero-message rows.
  `kind_runs` is *also* empty when the caller did not ask for the shape (`SearchOptions::shape`
  carries a real cost), but `cs search --json` always asks, so over this transport the empty
  array only ever carries the first meaning.
- **`match_seqs: []` and `hits: []`** — nothing matched, because nothing was searched for. An
  empty query returns the corpus by recency, which is the no-query browse list, and it is not a
  failed search.

## Known warts

Documented rather than changed, because changing them changes a published interface:

- **`grouped` is absent under `--flat`, not `false`.** A decoder modelling both shapes as one
  envelope has to make it optional. `chat-search-me9.32` is the decision about emitting it in
  both.
- **The output is pretty-printed**, always. 97,606 bytes where the compact form is 62,547 —
  1.56× — on a path carrying 111 KB per keystroke at `--limit 60`. Folded into
  `chat-search-me9.29`.

## Not covered here

`cs show --json` is the second client contract ADR 12 published, and it has no written
statement of its shape either — `chat-search-me9.34`. Nothing in it is ever null, which is a
sentence worth writing down rather than deriving from Rust, and it is otherwise the
better-specified of the two: it carries a `v` and it names its offset encoding on the wire as
`mark_offsets`, both of which this contract lacks (`chat-search-me9.33`). This file documents
`snippet_spans` in prose because there is nowhere on the wire that says it.

`cs status`, `cs scan`, `cs index`, `cs archive`, `cs needs` and `cs explain` also take
`--json`; those are operator output rather than a client seam, and nothing has been promised
about them.

## How this stays true

**The three field tables above are read by a test.** `crates/cs/tests/json_contract.rs` parses
them out of this file, builds an index whose conversations exercise every state described —
untitled, zero-message, no `cwd`, no `ts`, no destination — runs the real binary against it,
and checks both directions:

- every key the binary emits has a row here, and every row here names a key the binary emits;
- a key this file calls `never` is null nowhere in the response;
- a key this file calls **nullable** *is* null somewhere. That is the half that stops this
  document rotting: a field made non-null at the source would otherwise leave the prose telling
  a client to handle a state that can no longer occur.

So the tables are the contract rather than a description of it, and the parser is deliberately
literal: a field row opens with the key backticked in the first column, and the third column is
read for the words *nullable* and *absent*. Reformatting a table breaks the test rather than
silently unhooking it, which is the failure mode worth having — a pin that quietly stops
matching anything is worse than no pin, because it still reads as one.

Adding a field to `Hit` or `Group` therefore fails the suite until a row lands here. That is
the point: the failure is the reminder, because the version of this file that goes stale is one
nobody was forced to update.
