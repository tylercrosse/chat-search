# `cs search --json`

The contract a non-Rust client decodes. ADR 14 made it load-bearing: every surface that is not
a Rust program spawns `cs search --json` and reads this, so a field's shape is part of the
interface whether or not anyone wrote it down.

This file exists because it was not written down. The Swift spike (`chat-search-me9.22`) was the
first client to decode this without reading the structs that produce it. It typed `title` as a
non-optional `String`, which is the obvious choice and passes every hand test, because no
untitled conversation appears in the first ten rows of anything. Then it threw at `results[54]`
of a `--limit 60` query. Exactly the page size a GUI asks for, and exactly the size nobody
checks by hand (`chat-search-me9.27`).

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
not about conversations. A message-level total would be a second counting rule no surface reads.

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
| `thread_count` | integer | never | How many strands of the conversation's DAG (ADR 4) its messages fall into — what a reader calls a fork. **1 on 4,379 of 4,426 and above 1 on only 45**, so draw it as a mark that appears rather than as a column that is almost always the digit one; the mockup shows it above 1 and not at all otherwise. `0` on the two conversations that hold no messages, which is neither one strand nor several. Where a row is a [`sitting`](#sitting) this is the opening record's and not the fold's sum: an activity log put back together is one linear chat, and adding its members' counts would report eight forks on a Gemini conversation that never branched. |
| `cwd` | string | **nullable** | Working directory, for sources that have one. |
| `model` | string | **nullable** | The model of the **last message that named one**, resolved in the indexer's rollup — a summary of the message-level field rather than a fact about the conversation (ADR 24). 166 conversations name more than one, so a cell drawing this is saying *the model this ended on* and should not imply the whole of it was that. Free-form as the source wrote it, from `gpt-4o` to `text-davinci-002-render-sha`: an open set to render, never to parse or map. See [The nullable fields](#the-nullable-fields) for what null means. The opening record's where a row is a sitting, as `thread_count` is. |
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
`{"band":"tool","n":12}` is four times the bytes for the same two facts.

**Full resolution, and it stays that way** (ADR 26). The runs are every drawn message of the
conversation, not a summary quantised to the ~200 columns a row strip has, even though they are
a third of this response and a median row already fits. Downsampling is the client's: only the
client knows how wide it is drawing, which of the four bands it is showing, and that the same
runs feed a row strip, a sitting card and a full-height minimap at three different widths. A
strip bucketed server-side to the narrowest of those cannot be widened again, and a client that
hides the tool band — 66–85% of the axis — would be drawing an axis measured at up to 2.75x its
true length with nothing in the payload to warn it.

`band` is one of `user` · `agent` · `reasoning` · `tool`. Open set: decode an unknown band as a
run you cannot colour rather than as an error.

The run lengths sum to the **drawn** message count, which is not `msg_count`: successful tool
results are omitted rather than counted, because a strip position a reader cannot click on is a
lie about where they are. So `kind_runs` and `match_seqs` are two different coordinate spaces
and cannot be drawn on one strip without the reconciliation `chat-search-me9.25` is filed for.

---

## The facet rail — `cs facets --json`

A second reply, from a second command, and the one place this file describes something other
than a search. `cs facets [QUERY] --json`:

```json
{
  "v": 1,
  "query": "borrow checker agent:codex",
  "index_state": "ready",
  "sources": {
    "keyword": "agent:",
    "all": { "selected": false, "query": "borrow checker" },
    "values": [
      { "value": "codex", "state": "include", "coverage": "live", "conversations": 665,
        "query": "borrow checker" },
      { "value": "claude-code", "state": "off", "coverage": "live", "conversations": 458,
        "query": "borrow checker agent:codex,claude-code" }
    ]
  },
  "dates": {
    "keyword": "date:",
    "all": { "selected": true, "query": "borrow checker agent:codex" },
    "values": [
      { "value": "today", "label": "Today", "state": "off", "conversations": 8,
        "query": "date:today borrow checker agent:codex" },
      { "value": ">1mo", "label": "Older", "state": "off", "conversations": 3763,
        "query": "date:>1mo borrow checker agent:codex" }
    ]
  },
  "dirs": {
    "keyword": "dir:",
    "all": { "selected": true, "query": "borrow checker agent:codex" },
    "indexed": 128,
    "undirected": 3303,
    "values": [
      { "value": "/Users/t/dev/projects/chat-search", "state": "off", "conversations": 138,
        "query": "dir:/Users/t/dev/projects/chat-search borrow checker agent:codex" }
    ]
  }
}
```

**Why it is a command and not a key on the envelope.** The rail's census has to stat the source
directories, which is a cost the per-keystroke search path should not carry, so it would have to
be a key that is sometimes present — and a sometimes-absent key is a second type to a decoder,
which is the thing [the envelope pin](#how-this-stays-true) exists to keep out. Two replies with
one shape each beat one reply with two.

**Why it exists at all.** docs/TUI-DESIGN.md §5: a facet bar is a *projection of the query text*,
never a selection kept beside it, and the rules that rewrite that text — widen an existing
`agent:`, drop a standing exclusion, put a new token in front of the free text — live in
`cs_core::query` with the grammar. A client in another process cannot call them, and one that
assembled `agent:` tokens itself would be the second, partial parser §5 costs out. So each chip
arrives carrying **the whole query text clicking it produces**. Put that string in the input box;
do not splice a token.

| key | type | null? | |
| --- | --- | --- | --- |
| `v` | integer | never | `1`. Moves under the same rule as the search envelope's. |
| `query` | string | never | The query this is a projection of, as parsed. |
| `index_state` | string | never | What is at the index path — the same four names `cs status` reports, `no_index` and `building` included. **This command never refuses**, unlike a search: on a first run the true rail is every configured source at zero, which is the state a client most needs to draw and the one an error would hide. This key is what says the counts are provisional. |
| `sources` | object | never | The `agent:` rail. |
| `dates` | object | never | The `date:` rail. |
| `dirs` | object | never | The `dir:` rail. |

`sources`:

| key | type | null? | |
| --- | --- | --- | --- |
| `keyword` | string | never | `agent:` — the token these chips write, so a client can label the section without hard-coding the grammar. |
| `all` | object | never | `selected` is true exactly when the query names no source, which is the only state in which every source is in the answer. `query` is the text with every `agent:` token gone, exclusions included. |
| `values` | array | never; may be empty | One chip per source the machine knows about, ordered by id so the rail does not reshuffle between runs. |

One chip:

| key | type | null? | |
| --- | --- | --- | --- |
| `value` | string | never | The source id, which is what `agent:` selects on and is permanent (ADR 16). |
| `state` | string | never | `include` · `exclude` · `off`. Three states because the query has three things to say about a source: an excluded one drawn like an untouched one is filtering the reader cannot see. Clicking an excluded chip includes it — the rewrite strips the value from every token first, negated ones included. A value the query both keeps and drops reads `exclude`, because in the SQL the exclusion is an `AND NOT` and survives the include beside it. |
| `coverage` | string | never | `live` · `missing` · `unconfigured` · `retired`, as `cs status` reports them. Read beside `conversations`: a configured source at zero is a broken importer, and a source nobody configured is a tool nobody uses. Drawn from the index alone the two are one absence, which is the failure `chat-search-a7k.29` exists to prevent. |
| `conversations` | integer | never | What the index holds for it. Zero is an answer, not an absence. |
| `query` | string | never | The whole query text after clicking this chip. |

`dates`:

| key | type | null? | |
| --- | --- | --- | --- |
| `keyword` | string | never | `date:`. |
| `all` | object | never | `selected` is true exactly when the query carries no `date:` token, including one this rail has no chip for. |
| `values` | array | never | The spans, newest first. Fixed vocabulary, not a census: an index that can count nothing still has four, because they are arithmetic. All but the last nest — today ⊂ this week ⊂ this month — and the last is the complement of the one before it, so the counts do not sum to the corpus. |

One span:

| key | type | null? | |
| --- | --- | --- | --- |
| `value` | string | never | The `date:` value the query text will carry: `today`, `week`, `month`, `>1mo`. |
| `label` | string | never | The same span in words. Carried because `>1mo` is syntax rather than something to click, and two clients writing "Older" beside it is one rule written twice. |
| `state` | string | never | `include` · `exclude` · `off`, as a source's. Two spellings of one span are one selection: `date:<7d` lights the week chip, because the comparison is of the window each resolves to and not of the text. |
| `conversations` | integer | never | What the index holds inside the span, counted the way `search` filters it — an undated conversation is in none of them. |
| `query` | string | never | The whole query text after clicking. It **replaces** whatever `date:` was there rather than widening, because two `date:` tokens intersect and the overlap of two spans is the smaller of them or nothing at all. That rule is `cs_core::query`'s; nothing about it is visible here beyond the string. |

`dirs`:

| key | type | null? | |
| --- | --- | --- | --- |
| `keyword` | string | never | `dir:`. |
| `all` | object | never | `selected` is true exactly when the query carries no `dir:` token. |
| `values` | array | never; may be empty | The busiest directories, most conversations first, ties broken by path. **Not every directory**: a rail is not a list, and the tail of this distribution is per-conversation scratch directories (`chat-search-6eb.26`). |
| `indexed` | integer | never | Distinct directories in the index, which may be more than there are chips — what lets a client say it is showing 12 of 128. |
| `undirected` | integer | never | Conversations recording no directory at all, which no `dir:` token can reach. Most of the corpus: only the agent sources have a working directory. A client that does not say so draws a facet that reads as a filter over everything. |

One directory:

| key | type | null? | |
| --- | --- | --- | --- |
| `value` | string | never | The `cwd` as recorded, which is what `dir:` selects on. A path, not a project name — deriving one collapsed seven unrelated directories onto a single label and the alternative read the live filesystem, which is why `chat-search-6eb.26` was closed. Shortening it for display is a client's business and reversible. |
| `state` | string | never | `include` · `exclude` · `off`. `dir:` is a case-insensitive substring in the SQL, so **one token lights every directory beneath it**: `dir:dev` includes all of them, and clicking one off takes that token out rather than leaving a filter its chip no longer reflects. |
| `conversations` | integer | never | What the index holds for it. |
| `query` | string | never | The whole query text after clicking. A second directory widens, as a second source does. |

**A directory whose click cannot be written is not offered.** Each click is reparsed before it is
handed over and the directory is dropped if the round trip does not name it, which costs a chip
and saves a chip that lies; `indexed` still counts it either way. A client needs to handle an
empty `values` under a non-zero `indexed`, and needs nothing else: the check is the grammar's own
reading rather than a list of characters, so what it drops changes as the grammar does.

**A value that needs quoting arrives quoted** (`chat-search-me9.8.16`). Whitespace ends a word and
a comma ends a value, so `dir:~/Mobile Documents` used to come back as a filter *and* a search
term and such a directory had no chip at all. `query` now reads `dir:"~/Mobile Documents"`, and
the quoting is inside the string a client pastes into its box — there is nothing to add, strip or
escape on the way. A value that does not need quotes does not get them. This is not a `v` bump:
`query` has always been an opaque string to paste whole, and a client that pastes it goes on
working. What moved is that the rail now offers chips it used to withhold.

**A value the query names and this reply has no chip for** — a directory outside the busiest,
`date:yesterday`, a bound someone typed — lights nothing and turns that section's `all.selected`
off. That pair is the whole statement: something is filtering, and it is not one of these. The
token itself is visible and editable where every filter is, in the box (docs/TUI-DESIGN.md §5).

**An absolute span is that case, and it is the one a client will meet** (`chat-search-me9.18`).
`date:` now takes a half-open window with either end optional — `date:2026-07-28..2026-08-02` —
which is what a timeline drag produces and what none of the four chips stands for. Nothing on
this wire changed and it is not a `v` bump: the span is a `date:` token like any other, so it
arrives inside `query` as text to paste whole, `dates.all.selected` is false while it is in
force, and each chip's own `query` still replaces it wholesale when clicked. What a client
cannot do from this reply is *write* such a span, since a scrubber has no chip to be handed one
— `cs_core`'s `Window::value_in` renders it, and `cs timeline` below is where it reaches a
client that spawns `cs` rather than linking it (`chat-search-me9.8.20`).

---

## The timeline — `cs timeline --json`

The third reply, and the facet rail for the one axis a rail cannot enumerate. `cs timeline
[QUERY] --json [--buckets N] [--drag FROM..UNTIL] [--prefix]`:

```json
{
  "v": 1,
  "query": "borrow checker",
  "ms": 12.1,
  "index_state": "ready",
  "from": 1675321200000,
  "until": 1786035600000,
  "from_date": "2023-02-02",
  "until_date": "2026-08-13",
  "bucket_days": 8,
  "sources": ["chatgpt-export", "claude-code", "codex", "gemini-cli", "google-takeout"],
  "buckets": [
    { "from": 1675321200000, "until": 1676012400000,
      "conversations": 14, "matches": 0, "sources": [14, 0, 0, 0, 0] },
    { "from": 1676012400000, "until": 1676703600000,
      "conversations": 21, "matches": 2, "sources": [19, 0, 2, 0, 0] }
  ],
  "undated": 4,
  "in_range": 3617,
  "total": 58,
  "window": null,
  "all": { "selected": true, "query": "borrow checker" },
  "drag": null
}
```

**Why a client cannot draw this from the rows it already has.** A search comes back at
`--limit`, and ranking is not chronological, so the page is a *biased* sample of exactly the
axis being drawn: the top sixty of 354 matches are not sixty of them spread evenly through
time. The mistake is invisible — the picture looks like a picture — which is why the counting
happens here and only counts cross the wire. A second consequence is that the reply is a fixed
size whatever the archive does, where one instant per conversation would grow with it forever,
on a path that runs per keystroke.

**The bars are drawn with `date:` left out of them.** `buckets` counts everything surviving
every *other* filter, so a window narrows the number beside the picture and never the picture.
That is the whole reason a scrubber is worth having: a timeline filtered by its own selection
draws a solid block and can never say what widening would get you (`poc/ui`'s `visible(true)`).
`window` is what to draw *over* the bars, and `in_range` is what the window holds.

**The axis is the corpus, not the query.** `from` and `until` are the whole index's dated span,
so the coordinate system does not move while somebody types. A bucket is a whole number of civil
days rather than a span divided by a count, which is what keeps it the same number of *days*
either side of a clock change.

**`conversations` and `matches` are two questions, not a series and its highlight.**
`conversations` ignores the free text — "when was I working on this" — and `matches` is the
conversations a term landed in. For a query with nothing searchable in it there is nothing to
have matched, so `matches` is zero throughout; `total` still counts, because a browse still has
an answer and the list below it is showing one.

| key | type | null? | |
| --- | --- | --- | --- |
| `v` | integer | never | `1`. Moves under the same rule as the search envelope's. |
| `query` | string | never | The query this is a distribution of, as parsed. |
| `ms` | number | never | What this took, rounded to two places by the same rule the search envelope uses. |
| `index_state` | string | never | As the search envelope's. Unlike `cs facets` this **does** refuse when there is no index, because a histogram of nothing is not a drawable state the way a rail of zeroes is. |
| `from` | integer | never | Epoch millis. The first bucket's start, which is a local midnight. `0` when the index holds nothing dated. |
| `until` | integer | never | The last bucket's end, half-open. Later than the newest conversation by up to one bucket, because the last bucket is whole rather than clipped. |
| `from_date` | string | **nullable** | `from` as a local `YYYY-MM-DD`, for labelling that end of the axis. On the wire for the reason `ended_date` is: the local-date bug happened because three clients each derived the day themselves, and a client formatting an instant is a fourth. Null exactly when `buckets` is empty — `0` renders honestly as `1970-01-01`, which is the one wrong label a reader cannot tell from a right one. |
| `until_date` | string | **nullable** | The same for `until`. |
| `bucket_days` | integer | never | Civil days per bucket; `0` when there are no buckets. Carried so an axis can be labelled without dividing the span by the bucket count and getting 5.97 days. |
| `sources` | array | never; may be empty | Source ids in the order every `Bucket.sources` counts them, sorted so a stacked bar does not reshuffle between keystrokes. Only sources the filters keep appear. |
| `buckets` | array | never; may be empty | Oldest first, abutting, covering the whole axis. Empty exactly when the index holds no dated conversation. |
| `undated` | integer | never | Conversations the filters keep that have no `ended_at` and are therefore in no bucket — four of this corpus's 4,426. Carried for the reason `dirs.undirected` is: a picture that silently drops what it cannot place is a picture claiming to be everything. |
| `in_range` | integer | never | Of what the bars draw, how many are inside `window`. Free text ignored, like the bars. Equal to the sum of `conversations` when `window` is null. |
| `total` | integer | never | How many conversations the query selects with `limit` ignored — **the same number, spelled the same way, as the search envelope's `total`**, and always settled. Counted here rather than read off a search because the two are two processes and can land a keystroke apart. |
| `window` | object | **nullable** | The `date:` window in force, described below. Null when the query names none, and **also null when the only one it names is negated**: the complement of a window is not a rectangle, and drawing it as one would put the selection over exactly the stretch the filter threw away. |
| `all` | object | never | `selected` is true exactly when the query carries no `date:` token; `query` is the text with every one of them gone. The same shape as a rail's All chip, because it is the same thing — the click that clears the selection. |
| `drag` | object | **nullable** | What a drag writes, described below. Null unless `--drag` was passed, which is the only thing that flag changes about this reply. |

**`--drag` is the scrubber's half of the grammar, and it exists because a rail's trick does not
reach.** Every chip above arrives carrying the query text clicking it produces, which is what
keeps a client from ever assembling an `agent:` token. A drag is two instants out of a continuum
and cannot be enumerated that way, so the trade is made the other way round: hand over
`FROM..UNTIL` in epoch millis, in whichever order the pointer visited them, and get the finished
text back. What that saves a client is `Window::value_in`'s two lossy rules — each edge rounds
*outward* to a whole second, and an edge on a midnight is written as the bare date — and those
are not rules to keep a second copy of in a language that cannot link `cs_core`.

Two behaviours follow from it being `Query::toggling` underneath, and both are wanted. **Dragging
the window already in force takes it back off**, which is what every chip in this interface does
and is the only gesture that clears a selection without a control of its own. And **a drag that
names no span clears rather than writing an empty filter**, which is `poc/ui`'s "a drag under 1%
of the span clears the selection" arrived at through the grammar instead of through a magic
fraction.

**What `--buckets` is for.** The bucket count is a *picture's* resolution and therefore a
property of a drawing surface rather than of the corpus — a terminal has room for 68 columns and
the macOS drawer draws 180. It has a default so a client with no opinion does not have to form
one, and the count that comes back may be one short of what was asked for, since the last bucket
is whole.

---

## The window, and the drag that writes one

`window` — what to draw over the bars:

| key | type | null? | |
| --- | --- | --- | --- |
| `from` | integer | nullable | Epoch millis, inclusive. **Null is an open edge, not the end of the axis**: `date:<7d` reaches to now and past it, and a client that clamped a missing bound to the axis's `until` would draw a rectangle stopping where the data stops rather than where the filter does. |
| `until` | integer | nullable | The same, exclusive. |
| `value` | string | nullable | The window written back as a `date:` value — `2026-07-28..2026-08-02`. Null for a window bounded at neither end, which this grammar cannot spell. Worth putting in a header: it is what a reader would have typed, and it spells a *relative* token's current meaning in absolute terms. |

`drag` — what `--drag FROM..UNTIL` answers:

| key | type | null? | |
| --- | --- | --- | --- |
| `value` | string | nullable | The `date:` value the two instants are typed as. Null when they name no span, which is what a drag narrower than one bucket produces. |
| `query` | string | never | The whole query text after the drag. Paste it into the box; do not splice a token. |

---

## `Bucket` — one bar

| key | type | null? | |
| --- | --- | --- | --- |
| `from` | integer | never | Epoch millis. Half-open `[from, until)`, so consecutive buckets tile without an instant falling in both or neither — the same reading as `date:`'s own span. |
| `until` | integer | never | Where the next bucket starts. The last one's is the axis's `until`. |
| `conversations` | integer | never | Rows surviving every filter but `date:`, free text ignored. Counted per *sitting* where the fold applies, so this and the number beside the results list are counting the same things. |
| `matches` | integer | never | Of a searchable query's matches, how many landed here. Never more than `conversations`, and zero throughout when the query has nothing searchable in it. |
| `sources` | array | never | `conversations` broken down by source, parallel to the reply's `sources` and summing to `conversations`. Carried rather than derived so a bar can be stacked in the palette's own source hues without a second census. |

---

## The nullable fields

Seven of them across the three row shapes, and in every case null is a fact about the conversation
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

### `model` — no message in this conversation named one

Null for **1,300 of 4,426**, measured 2026-08-06, and the shape of that number is the point: 1,280
of them are the whole of Google Takeout, whose activity records carry no model field at any nesting
level, and the other 20 are spread thin across every source that does.

| conversations | what they hold |
| --- | --- |
| 1,280 × `google-takeout` | Every row of the source. The export is an activity log of prompts and responses and does not record which model answered, so this is "not applicable" for a whole surface rather than a gap in 1,280 records. |
| 12 × three sources | No assistant turn at all — a session of slash commands, or a `New chat` opened and abandoned. There was nothing for a model to have produced. |
| 8 × `chatgpt-export`, `claude-code` | Assistant turns that declared no model. The field is optional in both formats and some conversations simply do not carry it, which is why this cannot be typed non-optional on the evidence of the other 4,406. |

**The inverse of `cwd`, and worth reading beside it.** `cwd` is null because a web page has no
working directory — an absence of the concept. This is null because nothing *said*, which is
ordinary missing data and a client should draw it as nothing at all rather than as `unknown`.

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
about them. `cs facets --json` and `cs timeline --json` are client seams and **are** promised —
each has a section above — because a GUI's facet bar and its scrubber are not optional chrome
and the rules behind both are the query grammar's.

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

**Seven of the field tables above are read by a test.** `crates/cs/tests/json_contract.rs`
parses them out of this file, builds an index whose conversations exercise every state described
— untitled, zero-message, no `cwd`, no `ts`, no destination — runs the real binary against it,
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

Adding a field to `Group`, `Match`, `Hit`, `Bucket` or either envelope therefore fails the suite
until a row lands here. That is the point: the failure is the reminder, because the version of
this file that goes stale is one nobody was forced to update.

**The facet rail's tables are the exception**, and the line between them is worth naming rather
than closing by habit. A rail is chips carrying opaque strings — a client pastes `query` into a
box and never reads it — so a key that moved fails loudly at the decoder. The timeline is
*numbers a client does arithmetic on*, and a bar that quietly stopped being counted the
documented way draws a wrong picture instead of failing, which is why it joined the pinned half.
