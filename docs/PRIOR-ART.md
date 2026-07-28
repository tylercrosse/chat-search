# Prior art and competitive analysis

Surveyed 2026-07-28.

**Verification status matters here, so it is marked per row.** Tier 1 storage and search
claims were verified by reading the projects' own source and docs. Tier 2 and Tier 3 are
README- and marketing-level only. "unknown" means *not determined*, not *absent*.

Two claims were checked directly against the repositories rather than taken second-hand,
because they are load-bearing for whether this project should continue: that Polylogue and
cass are genuine archives, and which sources each covers.

---

## Headline: two priors were wrong

Going in, the assumption was that existing tools read live source files and none would
survive the source being deleted, and that ingesting both the ChatGPT export and CLI-agent
transcripts was an open gap. Both are false.

- **cass and Polylogue are true archives.** Both copy content into their own durable store
  with content-hash idempotency, and both explicitly design for the source disappearing.
- **Polylogue already ingests ChatGPT account exports *and* CLI transcripts.**
- **SQLite FTS5 is not an unusual choice** — Polylogue and Agent Sessions both use it.

The live-read prior does hold for everything below the top two.

---

## Tier 1 — multi-provider agent-transcript tools

Source-verified.

| Tool | Sources | Surfaces | Storage | Search | **Archive or live-read** | Licence | Last activity | Price |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| [Polylogue](https://github.com/Sinity/polylogue) | ChatGPT **export**, Claude.ai export, Claude Code, Codex, Gemini CLI, AI Studio, Hermes, Antigravity. **No OpenCode** | CLI, Python API, local HTTP reader, MCP server, MV3 capture extension, daemon | Split SQLite: `source.db` (raw evidence), `index.db` (derived), `embeddings.db`, `user.db` (overlays), `ops.db` (disposable) + SHA-256 content-addressed blob store | FTS5 contentless + BM25; lanes for dialogue/actions/semantic; hybrid via RRF | **Archive.** `source.db` is "the rebuild root"; ingest idempotent by content hash; documented backup/restore and tier-durability classes | MIT | 2026-07-28 | Free OSS |
| [cass](https://github.com/Dicklesworthstone/coding_agent_session_search) | 23 agents incl. Codex, Claude Code, Gemini CLI, **OpenCode**, Cursor, Aider, Cline, Copilot, Antigravity + **ChatGPT desktop app store** (not the export ZIP) | TUI, CLI, HTML export; third-party MCP bridges | SQLite archive + Tantivy index generations + raw mirror per source | Tantivy BM25 with edge n-grams + optional MiniLM ONNX + hybrid RRF | **Archive.** "SQLite is the source of truth"; rsync sync additive-only, remote deletions never propagate; has a `remote_source_pruned` gap code | **MIT + "Anthropic Rider"** — GitHub reports NOASSERTION, not OSI-clean | 2026-07-28 · 1,027 stars | Free OSS |
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

The consequence matters: because Cluster 2 optimises for *resume*, it optimises for the
**recent** session. Retention is off-strategy — an expired session cannot be resumed, so why
keep it. That is why three of five Tier 1 tools prune. Only cass and Polylogue reconceived
the problem as evidence retention, and both had to build a durable store to do it.

## Gaps — closed and open

**Closed. Do not treat these as differentiators.**

- Cross-provider web-export **and** CLI ingestion — Polylogue does it.
- True archival durability — cass and Polylogue both survive source deletion, with
  content-hash idempotency and explicit anti-pruning design.
- SQLite FTS5 as the search layer — Polylogue and Agent Sessions both use it.
- MCP exposure of chat history — at least six independent servers.
- Breadth of provider support — 20–28 providers is table stakes in Cluster 2.

**Genuinely open.**

- **All five of this project's sources in one archiving tool.** Polylogue covers 4/5 and is
  missing OpenCode entirely. cass covers all five but reaches ChatGPT only through the
  desktop app's local store — unencrypted v1 only, with v2/v3 needing manual setup — not the
  export ZIP. The intersection is real but narrow.
- **Retention-aware capture.** Everyone treats ingestion as a scan. Nobody schedules against
  a known deletion clock. cass gets closest with a `remote_source_pruned` signal, but that is
  diagnostic after the fact rather than preventive.
- **Licence cleanliness at the top.** cass is the strongest competitor and its licence is not
  OSI-clean (NOASSERTION on GitHub).
- **A GUI over a real archive.** The best storage (Polylogue, cass) has the weakest UI; the
  best UI (claude-code-history-viewer, 1,957 stars) has no storage and no index at all.

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

**The honest recommendation** is to spend an hour with cass and Polylogue against the real
corpus *before* writing more importers. If either finds the conversations you cannot find
today, that is the actual goal met, and this becomes an archiver feeding it rather than a
whole search stack. If neither does — for the OpenCode gap, the licence, or because their
ranking is not better than ours — the path continues with much better information.

That check is cheap and it is the sort of thing that only gets more expensive to do later.
