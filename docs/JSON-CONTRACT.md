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

Nullability is also the first thing an extension can quietly break, so what may change here
without warning, and what may not, has [its own section](#extending-this-contract).

Since `chat-search-me9.36` the reply has an author: `cs_core::answer` builds it and the CLI
serializes it, the way `cs_core::blocks` already builds what `cs show --json` prints. Nothing
below is assembled at the print site, so the terminal listing and this contract cannot come to
describe different searches.

> Counts below are from the reference corpus on this machine, 3,059 conversations as of
> 2026-08-04. They say how *rare* a state is, never whether it can happen — a client must
> handle every nullable field regardless of how few rows exercise it today. Rarity is the
> hazard here, not the reassurance.

---

## The envelope

One JSON object on stdout, pretty-printed. Exit status is part of the interface: a nonzero exit
means no search was answered, and what stdout carries instead is the [refusal](#the-refusal).

`--flat` answers a different question — matching *messages* rather than conversations — and has
[its own envelope](#the---flat-envelope). The two are separate shapes on purpose: one envelope
covering both would need a discriminator every decoder has to model for a case it never asks
for, and a `count` that means something different depending on the value of another key.

| key | type | null? | |
| --- | --- | --- | --- |
| `v` | integer | never | Contract version, `1`. What moves it and what does not is [Extending this contract](#extending-this-contract); a decoder that reads no further can treat an unexpected value as "this reply was written for someone else". |
| `query` | string | never | The query **as parsed**, with `--source` desugared into it: `--source codex` reaches the ranker as the token `agent:codex` and is reported that way. So this is what was actually run, not a flag and a string a client would have to recombine. Echo it back to tell which of several responses is in hand, and paste it into `cs explain` unchanged. |
| `ms` | number | never | Wall time for the ranking pass and the building of this reply, rounded to 2dp. Excludes opening the index, process start, and the second pass behind `settled`. Measured once, in core: three clients used to round their own copy and one of them fed the query log. |
| `count` | integer | never | `results.length`. Not how many conversations match — `--limit` truncates before this is taken. |
| `total` | integer | never | How many conversations the query selects with `--limit` ignored. **Read it beside `settled`**: a hundred rows out of a hundred and a hundred out of two thousand are the same hundred rows, and this is what says which. |
| `settled` | boolean | never | Whether `total` is the whole number. False means it is a **floor and a poor one** — measured on 2026-08-04 the typeahead `the` came back at 1,025 against a true 4,243 — to be ranged rather than displayed. Only `--prefix` can produce it: an ordinary `cs search` pays for the second pass before printing, because a one-shot caller has no later moment to spend it in. |
| `unapplied_filters` | array of string | never; may be empty | Filter tokens whose *value* selects nothing, e.g. `agent:notathing`. They parsed as filters and were then not applied, so the result set is wider than the query asked for. Non-empty is not an error and the exit status stays 0; a client that ignores this silently shows unfiltered results for a filtered query (`chat-search-6eb.11`). |
| `index_state` | string | never | What was at the index path: `ready` or `rebuilding` on any answered search. **Both mean the results are complete** — since `chat-search-me9.28` a rebuild assembles a sibling and swaps it in whole, so a client never has to wonder whether a thin answer is a partial one. `rebuilding` says only that a newer index is on its way, which is what lets a client offer to ask again rather than presenting this as the last word. Branch on the name, never on the sentence beside it (ADR 12). |
| `mark_offsets` | string | never | How to read every `snippet_spans` below. `utf8-bytes` today, and spelled exactly as `cs show --json` spells it, because the two contracts move together or not at all. See [`Span`](#span). |
| `results` | array of [`Group`](#group--one-conversation) | never; may be empty | The conversations that matched, best first. Always conversations. |

Object keys are emitted in alphabetical order. That is an artefact of how the response is
built, not a promise — decode by name.

## The `--flat` envelope

`cs search --flat --json`: matching **messages**, ungrouped, each naming its own conversation.

| key | type | null? | |
| --- | --- | --- | --- |
| `v` | integer | never | As above, and the same number: one contract, two shapes. |
| `query` | string | never | As above. |
| `ms` | number | never | As above. |
| `count` | integer | never | `hits.length`. |
| `unapplied_filters` | array of string | never; may be empty | As above. |
| `index_state` | string | never | As above. |
| `mark_offsets` | string | never | As above. |
| `hits` | array of [`Hit`](#hit--one-message-under---flat) | never; may be empty | The messages that matched, best first. |

No `total` or `settled`. The number worth settling counts *conversations*, and this envelope is
not about conversations; a message-level total would be a second counting rule no surface reads.

An empty query comes back with nothing in it. The fallback for a query too short to rank is a
list of recent conversations, and there is no honest way to answer a question about messages
with one.

## `Group` — one conversation

The default shape: the conversation is the result and its matching messages nest beneath it.
Identity is stated once, here, and never repeated inside `matches`.

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
| `score` | number | never | **Opaque ordering.** The rows arrive best first and that order *is* the ranking; a client never re-sorts on this number and never parses it. Today it is damped bm25 divided by a recency factor, which makes it negative and meaningful only against the other rows of the same response — none of which is promised, because the ranking is expected to grow past term matching (GLOSSARY.md's Indexer entry already says "and later vectors") and that moves the sign and the range without changing what the number is *for*. Always finite: a message with no timestamp is scored as though it were current rather than producing a NaN, which would reach the wire as `null`. |
| `match_count` | integer | never | Total matching messages, including any not returned in `matches`. |
| `match_seqs` | array of integer | never; may be empty | Where the matches sit, as 0-based message positions, ascending. **Positions into `msg_count`**, which is *not* the coordinate space `kind_runs` uses — `chat-search-me9.25` is that gap. Empty when nothing matched. |
| `kind_runs` | array of [`Run`](#run) | never; **may be empty** | What the conversation is made of, in reading order, run-length encoded. |
| `deleted_upstream` | boolean | never | The conversation is gone from its source and kept here (ADR 9). |
| `sitting` | [`Sitting`](#sitting) | **nullable** | Set only when this row stands for several conversations that were one chat. Null for every row that is one conversation, which is the whole corpus bar the Google Takeout records. |
| `matches` | array of [`Match`](#match--one-matching-message) | never; may be empty | The best matching messages, up to `--nested` (default 3), best first. Truncate this rather than sampling it: the first entry is the message that won the ranking. Empty for an empty query, which returns conversations without matching anything. |

## `Match` — one matching message

An element of `Group.matches`. **Message fields only** — everything about the parent
conversation is stated on the `Group` that holds it, so `conv_id`, `source`, `native_id`,
`title`, `destinations` and `deleted_upstream` are one level up rather than repeated under every
nested match.

`thread_key` is not among them. A conversation is a DAG (ADR 4) and the strand is a property of
the message, so stating it on the group would assert a fact about several threads about one.

| key | type | null? | |
| --- | --- | --- | --- |
| `msg_id` | string | never | `"<source>:<native_id>:<message_native_id>"`. |
| `role` | string | never | `user` · `assistant` · `tool` · `system`. |
| `kind` | string | never | `prose` · `reasoning` · `tool_call` · `tool_result`. `reasoning` is stored and never indexed (ADR 5, `chat-search-8mb`), so it cannot appear in a search result — only in `cs show`. |
| `ts` | integer | **nullable** | Epoch milliseconds. |
| `score` | number | never | As `Group.score` — opaque in the same way — for this message alone. |
| `snippet` | string | never | A window of the message around what matched, whitespace flattened. May begin with the literal prefix `⟨no match⟩ `, which means the match could not be located in the text and what follows is the head of the message rather than the matching part. It is a label, not content. |
| `snippet_spans` | array of [`Span`](#span) | never; may be empty | Where to highlight, as offsets into `snippet` in the units `mark_offsets` names. **May be empty**, which is an ordinary outcome rather than a failure: there is nothing in this snippet to mark. Today that happens only when the match could not be located, and a ranking that matches on meaning rather than on words will produce it whenever a result holds no query term at all — so draw the snippet unmarked rather than re-tokenizing it, and do not infer the `⟨no match⟩ ` prefix from it. The prefix is a sentence for a reader, these are offsets for a decoder, and the two coincide only while matching is lexical. |
| `on_head_path` | boolean | never | False when the message sits on a branch that was edited away — still searchable, but not part of the conversation as currently displayed. Only ever false when `--include-off-path` was passed. |
| `is_sidechain` | boolean | never | The message came from a subagent thread, which runs parallel to the main one rather than being an abandoned branch. |
| `thread_key` | string | never | Which thread within the conversation, usually the transcript filename. Opaque — do not parse it. |

## `Hit` — one message, under `--flat`

The elements of the flat envelope's `hits`, and the one place a message row names its own
parent. That is right in a list with no parent rows in it, and wrong the moment there are — so
this shape appears under `--flat` and nowhere else.

Every key [`Match`](#match--one-matching-message) has, meaning the same thing, plus six that
restate the conversation:

| key | type | null? | |
| --- | --- | --- | --- |
| `conv_id` | string | never | The conversation this message belongs to. |
| `msg_id` | string | never | As `Match.msg_id`. |
| `source` | string | never | As `Group.source`. |
| `native_id` | string | never | The **conversation's** native id, not the message's. |
| `destinations` | array of [`Destination`](#destination) | never; may be empty | As `Group.destinations`. |
| `title` | string | **nullable** | The conversation's title, carried so a flat result renders without a second lookup. Null under exactly the same conditions as `Group.title`. |
| `role` | string | never | As `Match.role`. |
| `kind` | string | never | As `Match.kind`. |
| `ts` | integer | **nullable** | As `Match.ts`. |
| `score` | number | never | As `Match.score`. |
| `snippet` | string | never | As `Match.snippet`. |
| `snippet_spans` | array of [`Span`](#span) | never; may be empty | As `Match.snippet_spans`, empty on the same terms. |
| `on_head_path` | boolean | never | As `Match.on_head_path`. |
| `is_sidechain` | boolean | never | As `Match.is_sidechain`. |
| `thread_key` | string | never | As `Match.thread_key`. |
| `deleted_upstream` | boolean | never | As `Group.deleted_upstream`, for this message's conversation. |

## The refusal

When there is nothing to search, stdout carries this instead of an envelope and the exit status
is nonzero:

```json
{ "error": { "code": "no_index", "message": "no index at /… — run `cs index` to build one" } }
```

| key | | |
| --- | --- | --- |
| `code` | `no_index` · `building` · `stale` | The contract. Switch on it. |
| `message` | string | Free to be reworded, and never parsed. |

The code alone is what makes a failure classifiable: before it existed a client had one English
sentence on stderr, and the first non-Rust reader of this contract classified index health by
substring-matching another program's prose (`chat-search-me9.22`). `no_index` and `building` are
the two `index_state` values that never appear in an answer, because in both there is no
readable index to answer out of.

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

The envelope says so on the wire, in `mark_offsets`, spelled exactly as `cs show --json` spells
it (`chat-search-me9.33`). Read the encoding from that key rather than from this paragraph: a
silently wrong offset highlights the wrong word rather than failing, so the value a client
branches on has to travel with the offsets it describes.

Spans index the *returned string*, ellipsis and any `⟨no match⟩ ` prefix included. Nothing
downstream can re-derive them: the window has been cut out of the message and its whitespace
flattened, and the term that matched need not appear in the query (`commits` marks `Commit`).

## `Sitting`

```json
{ "members": ["google-takeout:2026-07-28T16:27:48.790Z", "google-takeout:2026-07-28T16:31:02.104Z"], "gap_ms": 1800000 }
```

Present on a `Group` whose row stands for more than one conversation. Google Takeout exports an
activity log with no conversation key at any nesting level, so a twenty-turn Gemini chat arrives
as twenty conversations; `cs_core::sittings` reads the silences between them and puts them back
together at read time (`chat-search-o1i.5`).

| key | type | null? | |
| --- | --- | --- | --- |
| `members` | array of string | never | Every `conv_id` folded into this row, earliest first. The first is the row's own `conv_id`, and each is a real permanent id. |
| `gap_ms` | integer | never | The silence that delimited the sitting. Carried so a client can say what produced the fold rather than implying the export drew the boundary. |

**This is a reconstruction, and a client should say so.** The boundary is a heuristic over
timestamps, not a fact the export recorded, which is why it is reported here instead of being
folded silently into the row. It has no id and never gets one: an id derived from a gap
threshold would change the day the threshold did, and conversation ids are permanent (ADR 16).

**Every member opens the whole sitting, and the two halves agree** (`chat-search-o1i.8`).
`cs show` and `cs explain` resolve their argument the same way the ranker groups it, so
`cs show <members[7]>` renders one continuous transcript of every record, in the sitting's
order, and its message count is the row's `msg_count`. Pick any member: the id is real, and none
of them is a worse choice than the first.

Two consequences for a client that holds a member id:

- **The transcript names the opener, not the id you asked for.** `Transcript.conv_id` and
  `Explain.conv_id` come back as `members[0]` — the same id the row carries — because that is
  what the answer is about. Do not assume the reply echoes the request.
- **`Transcript.seq` is the position in the sitting**, not in the record, which is the
  coordinate space `Group.match_seqs` already used. The two line up; the record a message came
  from is still recoverable from its `msg_id`.

`cs show --json` carries a `sitting` of this same shape, so the seam survives the fold. Those
records were separate HTTP requests with no shared context on Google's side, and a client that
wants to draw a light rule between them has what it needs — but the fold is not structural, and
a reader who does not care never has to see it, which is the whole point of putting the chat
back together.

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

Six of them across the three row shapes, and in every case null is a fact about the conversation
rather than a gap in the data. None is being papered over at the source, and that is deliberate:
`""` would erase a distinction the importers spend real code creating, and omitting the key
would say "not applicable" about something that is merely unknown.

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

Null under exactly the conditions above, for the same 11 conversations. `Match` has no `title`
at all: the group that holds it already stated one, nullably.

### `Match.ts` and `Hit.ts` — the source recorded no timestamp

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
- **`match_seqs: []` and `matches: []`** — nothing matched, because nothing was searched for. An
  empty query returns the corpus by recency, which is the no-query browse list, and it is not a
  failed search.

## Known warts

Documented rather than changed, because changing them changes a published interface:

- **The output is pretty-printed**, always. 97,606 bytes where the compact form is 62,547 —
  1.56× — on a path carrying 111 KB per keystroke at `--limit 60`. Folded into
  `chat-search-me9.29`.

## Not covered here

`cs show --json` is the second client contract ADR 12 published, and it has no written
statement of its shape either — `chat-search-me9.34`. One field in it is nullable and the rest
never are: `sitting` is null for every conversation that is one conversation, which is the whole
corpus bar the folded Takeout records, and the sentence is worth writing down rather than
deriving from Rust. The two contracts now agree on the things they share: both carry a `v`, both
name their offset encoding on the wire as `mark_offsets`, and both answer for the same unit —
see [`Sitting`](#sitting) for what that means for an id copied from one to the other.

`cs status`, `cs scan`, `cs index`, `cs archive`, `cs needs` and `cs explain` also take
`--json`; those are operator output rather than a client seam, and nothing has been promised
about them.

## Extending this contract

**Additions are silent** (ADR 12). A key that appears in a later release is a key an older
decoder ignores, so a new field on a row, a new `Destination` kind, a new `Run` band and a
source nobody has watched before all reach a shipped client without breaking it. That is why
the open sets above say to decode an unknown variant as one this client cannot act on rather
than as an error: the alternative makes every addition a coordinated release.

**`v` moves only when a field changes meaning** — the one change a decoder cannot detect for
itself. A rename fails loudly at the first missing key and an addition is ignored, so neither is
a bump. A field that keeps its name and its type while its units, its sign, its coordinate space
or its nullability move is a client that goes on working and starts lying. Making a non-null key
nullable, or a nullable key absent, is that same change wearing a smaller hat, which is why the
pin below reads nullability in both directions rather than one.

Two fields above are therefore promised in the weakest form that is still useful to a client:
`score` and `snippet_spans` both narrow as the ranking grows past term matching, and neither
narrowing adds or removes a key. Promised now, that release is an addition. Left unstated until
a client had shipped against what the fields happen to do today, it would be a bump — and a bump
is a second decoder in every client, kept alive for as long as an old binary might still be on
someone's machine.

## How this stays true

**The five field tables above are read by a test.** `crates/cs/tests/json_contract.rs` parses
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

Adding a field to `Group`, `Match` or `Hit` therefore fails the suite until a row lands here.
That is the point: the failure is the reminder, because the version of this file that goes
stale is one nobody was forced to update.
