# Prior art and competitive analysis

Surveyed 2026-07-28.

**Verification status matters here, so it is marked per row.** Tier 1 storage and search
claims were verified by reading the projects' own source and docs. Tier 2 and Tier 3 are
README- and marketing-level only. "unknown" means *not determined*, not *absent*.

Two claims were checked directly against the repositories rather than taken second-hand,
because they are load-bearing for whether this project should continue: that Polylogue and
cass are genuine archives, and which sources each covers.

---

> **Revised after a second pass found the category leader that the first pass missed.**
> The surveying agent also reported that GitHub's search endpoint silently under-returned —
> `agentsview` did not match a query its own description should have matched. Treat every
> coverage claim here as a **lower bound**; more tools exist that this survey did not find.

## Headline: three priors were wrong

Going in, the assumption was that existing tools read live source files, that none survive
the source being deleted, and that ingesting both the ChatGPT export and CLI-agent
transcripts was an open gap. All three are false.

- **agentsview, cass and Polylogue are true archives**, copying content into durable stores.
- **agentsview covers all five of this project's target sources**, ingests the ChatGPT export
  ZIP, and ships a desktop app and web UI over its archive.
- **SQLite FTS5 is the common choice** — agentsview, Polylogue and Agent Sessions all use it.

The live-read prior holds only below the top three.

**agentsview is a strict superset of this project's MVP as scoped.** Verified from source
rather than README: `internal/parser/chatgpt.go` opens with *"Parses ChatGPT export archives
(conversations-\*.json) into structured session data with DAG linearization"* — the same job,
including the DAG handling, as the importer built here. 4,595 stars, MIT, Go, releasing
weekly.

---

## Tier 1 — multi-provider agent-transcript tools

Source-verified.

| Tool | Sources | Surfaces | Storage | Search | **Archive or live-read** | Licence | Last activity | Price |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| **[agentsview](https://github.com/kenn-io/agentsview)** | **~40 agents, all five of ours** — Claude Code, Codex, Gemini CLI, OpenCode + **ChatGPT export ZIP** and Claude.ai export. **Does not** decrypt the Antigravity store — see the correction below | CLI, daemon, **web UI**, **Tauri desktop app**, MCP, Docker, S3/Postgres/DuckDB targets | SQLite primary archive (`messages.content`, `thinking_text` copied in); optional Postgres/DuckDB | **FTS5 + sqlite-vec**, hybrid | **Archive.** ChatGPT is import-only, so it exists *only* as a copy. No reaping of vanished sources found | MIT | 2026-07-28 · v0.39.0 · **4,595 stars** | Free OSS. Sends an anonymous `daemon_active` telemetry ping, opt-out by env var |
| [Polylogue](https://github.com/Sinity/polylogue) | ChatGPT **export**, Claude.ai export, Claude Code, Codex, Gemini CLI, AI Studio, Hermes, Antigravity. **No OpenCode** | CLI, Python API, local HTTP reader, MCP server, MV3 capture extension, daemon | Split SQLite: `source.db` (raw evidence), `index.db` (derived), `embeddings.db`, `user.db` (overlays), `ops.db` (disposable) + SHA-256 content-addressed blob store | FTS5 contentless + BM25; lanes for dialogue/actions/semantic; hybrid via RRF | **Archive.** `source.db` is "the rebuild root"; ingest idempotent by content hash; documented backup/restore and tier-durability classes | MIT | 2026-07-28 | Free OSS |
| [cass](https://github.com/Dicklesworthstone/coding_agent_session_search) | 23 agents incl. Codex, Claude Code, Gemini CLI, **OpenCode**, Cursor, Aider, Cline, Copilot, Antigravity + the **ChatGPT desktop app store** — but only with a key you supply yourself, see the correction below | TUI, CLI, HTML export; third-party MCP bridges | SQLite archive + Tantivy index generations + raw mirror per source | Tantivy BM25 with edge n-grams + optional MiniLM ONNX + hybrid RRF | **Archive.** "SQLite is the source of truth"; rsync sync additive-only, remote deletions never propagate; has a `remote_source_pruned` gap code | **MIT + "Anthropic Rider"** — GitHub reports NOASSERTION, not OSI-clean | 2026-07-28 · 1,027 stars | Free OSS |
| [claude-code-history-viewer](https://github.com/jhlee0409/claude-code-history-viewer) | 28 providers, CLI-agent only. No ChatGPT web | Tauri desktop app + headless server | **None** | Plain scan; no FTS, no index, no embeddings | **Live-read.** Reads provider files in place every time | MIT | 2026-07-23 · **1,957 stars** | Free OSS |
| [Agent Sessions](https://github.com/jazzyalex/agent-sessions) | Codex, Claude Code, OpenCode, Cursor Agent, Hermes, Copilot CLI, Antigravity. No Gemini CLI, no ChatGPT | macOS desktop app | `~/Library/Application Support/AgentSessions/index.db` | SQLite FTS5, external-content tables | **Live-read despite having a DB.** Stores only a 48k-char sample per session, prunes tool IO by recency, deletes rows for vanished files, re-reads originals to display | MIT | 2026-07-24 · 740 stars | Free OSS |
| [fast-resume](https://github.com/angristan/fast-resume) | 11 agents incl. Claude Code, Codex, OpenCode, Cursor CLI. No Gemini CLI, no ChatGPT | TUI + CLI | `~/.cache/fast-resume/tantivy_index` | Tantivy, typo-tolerant, title-boosted | **Live-read.** Prunes vanished sessions; rebuilds on schema change; its own path says `.cache` | MIT | 2026-07-24 · 135 stars | Free OSS |
| [aivault](https://github.com/KaulikMakwana/aivault) | Claude, Gemini, Copilot, Continue, Cursor, aichat | CLI | unknown | Full-text, engine unknown | Likely archive — self-describes as versioned backup. **Not verified** | Apache-2.0 | 2026-07-12 · 0 stars | Free OSS |
| [hasna/sessions](https://github.com/hasna/sessions) | unknown | unknown | unknown — "across machines" | unknown | unknown | Apache-2.0 | 2026-07-27 · 0 stars | unknown |

## Tier 2 — Claude Code only

README-level, not source-verified. Live-read over `~/.claude/projects` unless noted.

| Tool | Surfaces | Search | Archive or live-read | Licence | Last activity |
| --- | --- | --- | --- | --- | --- |
| [claude-history](https://github.com/raine/claude-history) | TUI/CLI | Fuzzy | Live-read | MIT | 2026-07-21 · 422 stars |
| [claude-code-log](https://github.com/daaain/claude-code-log) | CLI → HTML/MD | None (renderer) | **Partial archive** — converted output survives | MIT | 2026-07-28 · 1,173 stars |
| [claude-historian-mcp](https://github.com/Vvkmnn/claude-historian-mcp) | MCP server | Scan + rank | Live-read | MIT | 2026-06-15 · 182 stars |
| [ClaudeHistoryMCP](https://github.com/jhammant/ClaudeHistoryMCP) | MCP server | BM25 + TF-IDF | unknown | MIT | 2026-02-27 · 65 stars |
| [claude-conversation-search-mcp](https://github.com/ticpu/claude-conversation-search-mcp) | MCP server | Indexed; filters file dumps, keeps reasoning | unknown | **GPL-3.0** | 2026-03-19 · 7 stars |
| [claude-conversation-extractor](https://github.com/ZeroSumQuant/claude-conversation-extractor) | CLI + UI | Real-time | **Partial archive** — exports to disk | unknown | on PyPI |
| [local-claude-chat-history-mcp](https://github.com/daniellmorris/local-claude-chat-history-mcp) | MCP server | Scan | Live-read (explicit) | MIT | 2026-07-09 · 2 stars |
| [0dust/claude-code-history-search](https://github.com/0dust/claude-code-history-search) | CLI | Semantic | unknown | MIT | 2026-04-04 · 1 star |
| [chist](https://github.com/luongnamkhanh/chist) · [sessionlens](https://github.com/PregnantPenguins789/sessionlens) · [cctree](https://github.com/vine77/cctree) · [yudppp](https://github.com/yudppp/claude-code-history-mcp) | CLI / MCP | varies | Live-read | MIT / none | 2025-07 – 2026-07 · 0–11 stars |

## Tier 3 — web-chat tools

README- and marketing-level only.

| Tool | Sources | Surfaces | Search | Archive or live-read | Licence | Price |
| --- | --- | --- | --- | --- | --- | --- |
| [LLMnesia](https://www.llmnesia.com/) | 12 web tools; **claims** local Claude Code support | Chrome extension only | Local keyword index | Partial — caches visited chats in browser storage; dies with the profile | **Proprietary** | Free |
| [AI Toolbox](https://www.ai-toolbox.co/) | Claude, ChatGPT, Gemini | Browser extension | Full-text + bookmarks | Partial, via export | **Proprietary** | freemium |
| [mirableio/chat-history](https://github.com/mirableio/chat-history) | ChatGPT + Claude **exports** | CLI | FTS + optional semantic | Reads your export; adds no durability of its own | MIT | Free OSS |
| [llm-conversations-viewer](https://github.com/TomzxCode/llm-conversations-viewer) | ChatGPT/Claude exports | Client-side web app | Keyword | Partial — persists into IndexedDB | unknown | Free OSS |
| Chrome extensions: [Claude Toolbox](https://chromewebstore.google.com/detail/claude-toolbox-chat-histo/camddjjmcemmmlndbciaodchkodhgibh), [Search for Claude Chats](https://chromewebstore.google.com/detail/search-for-claude-chats/lnfpjppihlpggjclgilbflbfccdliphd), [ChatGPT History Search](https://chromewebstore.google.com/detail/chatgpt-conversation-hist/jhllfdbccclcdiafljibcabipbmkfoem) | one web UI each | Extension | DOM scrape | Partial, fragile | **Proprietary** | free/freemium |

---

## Corrections — three tools do less than the tables above claimed

Re-checked 2026-07-30 against the projects' own source and READMEs. Each of these was
overstated in the first survey, and each overstatement pointed at work that looked solved.

- **agentsview does not decrypt the Antigravity store.** Its README says the `.pb` files are
  encrypted and that it invents no schema. Full transcripts require running **`agy-reader`**
  separately, which does not decrypt either — it asks Antigravity's own language-server daemon
  for `GetCascadeTrajectory` and writes the answer to a sidecar that agentsview's watcher picks
  up. Without it, agentsview degrades to heuristic "summary mode": prompts and tool names.
  The one project that *does* decrypt directly (`antigravity_decryptor`) reports AES-128-CTR
  where agentsview reports AES-GCM, so the generations differ — trust neither over our own files.
- **cass does not read the encrypted ChatGPT desktop store unaided.** It documents a
  `CHATGPT_ENCRYPTION_KEY` env var that the user must supply, and refuses key files looser than
  0600. Nobody has extracted this key, because it is behind a Keychain access group rather than
  a derivable password — see [FORMAT-NOTES](./FORMAT-NOTES.md).
- **Polylogue's extension is not continuous capture.** Its README says plainly that it does not
  watch page mutations or capture while you type. It is button-driven — *Capture page*, *Sync
  open tabs* — plus an explicitly-started backfill with cursor and lease state in IndexedDB and
  MV3 alarms to resume after the service worker is killed. That last part is the genuinely hard
  engineering and the reason to read it, but it is not the always-on capture the row implied.

**Worth reusing rather than writing.** [`rusty-leveldb`](https://crates.io/crates/rusty-leveldb)
reads Chromium `Local Storage`/`IndexedDB` trees in pure Rust.
[`cookie-scoop`](https://github.com/jimmystridh/cookie-scoop) already implements the macOS
Chromium cookie path — Safe Storage password from the Keychain, PBKDF2-SHA1 at 1003 iterations,
AES-128-CBC — which is exactly what reading a desktop app's own session cookie requires.

## Where these tools cluster

**Cluster 1 — browser extensions, forced by access.** ChatGPT and Claude.ai keep
conversations server-side behind an authenticated session, and there are exactly two routes
in: the official account export (asynchronous, emailed, hours of latency, no incremental
update) or scraping the authenticated page. For anything that must feel live, the export is
unusable, so an extension is the only viable shape. That constraint also caps the cluster —
an extension cannot read `~/.claude/projects`. Polylogue hit the same wall and reached the
same answer, shipping an MV3 capture extension citing "Cloudflare friction on Claude.ai".

**Cluster 2 — session managers where search is a byproduct.** These were not built to
search; they were built to answer "which session do I `--resume`?" The naming gives it away:
`fast-resume`, `chist` (search/resume/export), Agent Sessions' "resume commands", `cass
resume`. Search arrived because the session list outgrew eyeballing. CLI agents write
plaintext JSONL to local disk with no auth barrier, so the marginal cost of provider #12 is
one adapter file — which is why this cluster has 20–28 providers while the web cluster stalls
at the handful of sites worth scraping.

Because Cluster 2 optimises for *resume*, it optimises for the **recent** session. Retention
is off-strategy — an expired session cannot be resumed, so why keep it. That is why three of
five Tier 1 tools prune. Only cass and Polylogue reconceived
the problem as evidence retention, and both had to build a durable store to do it.

## Gaps — closed and open

**Closed. Every one of these was called a differentiator at some point in this project and
every one is occupied.**

- All five sources in one archiving tool — **agentsview, 5/5**.
- Cross-provider web-export **and** CLI ingestion — agentsview and Polylogue.
- Archival durability surviving source deletion — agentsview, cass, Polylogue.
- SQLite FTS5 as the search layer — agentsview, Polylogue, Agent Sessions.
- Hybrid lexical + vector search — all three leaders.
- MCP exposure — saturated; agentsview ships one and six-plus standalone servers exist.
- Provider breadth — 20–40 is table stakes.
- Clean MIT licence at the top — agentsview and Polylogue both.
- Cross-machine sync — agentsview (S3, Postgres), cass (rsync mirrors).
- **A GUI over a real archive** — called open in an earlier revision; wrong. agentsview ships
  a Tauri desktop app *and* a web UI over its SQLite archive.

**Genuinely open — features, not a product wedge.**

- **Retention-clock-aware capture.** Nobody schedules ingestion against a known expiry. The
  Claude Code 30-day window is exactly the constraint no tool models. cass comes closest with
  a `remote_source_pruned` gap code, but that is post-hoc diagnosis rather than preventive
  capture. **This is the one idea the survey did not find anywhere.**
- **Strict zero-network operation.** agentsview pings PostHog by default (opt-out available);
  cass and Polylogue arguably already occupy the absolutist position.
- **Continuous web-chat capture without a manual export.** Export ZIPs are point-in-time, and
  the gap is wider than first written: Polylogue's MV3 extension is the only attempt, and it is
  button-driven rather than continuous. **Nobody captures a web surface unattended.** ADR 21
  takes the cookie-jar route at this, which no surveyed tool tries.

## Not found

No cloud/SaaS product doing cross-provider agent-transcript search — the commercial layer is
entirely extension-shaped. No Obsidian or Logseq plugin importing CLI-agent transcripts. No
published benchmark comparing any of these on corpus scale or query latency.

Two claims are unverified and load-bearing if relied upon: LLMnesia's Claude Code support
(implausible for a Chrome extension without an undisclosed native helper) and `aivault`'s
durability.

---

## What this means for this project

Written after the survey, and it is not entirely comfortable reading.

**Polylogue independently arrived at nearly this exact architecture** — durable source store
separated from a derived index, content-addressed blobs, FTS5 contentless with BM25, hybrid
RRF for semantic, and a separate store for user-authored overlays. That is strong validation
that the decisions in [DECISIONS.md](./DECISIONS.md) are sound. It also means the
importer-plus-index-plus-search layer is not novel work.

What remains genuinely ours:

- **The archiver is complementary, not competitive.** It is retention-aware capture running
  on a schedule against a known deletion clock, which the survey found nobody models. It also
  produces a plain mirrored tree of original files — which could feed cass or Polylogue just
  as well as our own index. That work stands whatever happens next.
- **OpenCode plus the ChatGPT export in one tool** is the narrow remaining gap.
- Rust practice was an explicit goal, and that is not invalidated by someone else having
  shipped a similar thing.

**The honest recommendation** is to install agentsview, point it at the real corpus, and see
whether it finds the conversations that are currently hard to find. That is an hour, and it
answers the question the whole project exists to answer.

Three outcomes, all fine:

1. **It works.** The actual goal is met today. This project narrows to the retention-aware
   archiver, the one idea the survey found nowhere, feeding it.
2. **It nearly works.** Contribute the gap upstream. It is MIT and shipping weekly.
3. **It does not.** Continue, with far better information about what "better" has to mean.

The archiver survives all three, because its output is a plain mirrored tree any of these
tools can read.
