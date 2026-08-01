# chat-search

Search your AI conversations across every tool you use, and open them where they live.

## The problem

Conversations are spread across ChatGPT, Claude, Claude Code, Codex, Gemini and OpenCode —
in native apps, CLIs, VS Code extensions and web apps — and finding an old one is
genuinely hard. Existing tools mostly start as Chrome extensions, which reaches the two web
surfaces and none of the rest.

Most of the corpus is already on disk in open formats. Roughly 80% of it can be read without
touching a browser at all, which is what makes a local-first tool the right shape.

Some of it is also actively disappearing: Claude Code prunes transcripts after 30 days by
default, and everything before 2026-06-19 on this machine was already gone by the time the
project started.

## Status

Early. One component works.

| | |
| --- | --- |
| **Archiver** | working, running every 5 minutes — 1,022 files / 2.2 GB captured |
| Importers | not built (a throwaway proof of concept exists under `poc/`) |
| Index and search | not built |
| Clients | not built |

See [docs/BACKLOG.md](docs/BACKLOG.md) for what is deferred and why.

## Quick start

```sh
cargo install --path crates/cs
cs init      # writes a config from the sources found on this machine
cs status    # show config, machine identity and source health
cs archive   # capture changed files; --dry-run to preview
```

`cs archive` is quiet: it prints only when it captured something, because it is meant to
run on a schedule and almost every run has nothing to say. Errors and source-drift
warnings still print; `--verbose` brings the table back.

The archive lands in `~/.chat-archive/`. On APFS it is stored with copy-on-write clones, so
capturing 2.2 GB cost about 2 MB of disk.

## Shape

```
live sources → [archiver] → raw archive → [importer] → [index] → [clients]
                dumb,        immutable      per-format   FTS5      CLI, GUI
              never parses
```

The split between archiver and importer is the load-bearing one. The archiver never parses
content, so it can be simple and reliable; the importer understands formats, so it can be
fixed and re-run over the whole archive retroactively.

```
crates/cs-archive   capture, manifest, change detection
crates/cs           the cs binary
poc/                throwaway TypeScript vs Rust benchmark, kept for its results
docs/               vocabulary, decisions, architecture, backlog
```

## Reading order

1. [docs/GLOSSARY.md](docs/GLOSSARY.md) — vocabulary. Four different things in these sources
   look like "a conversation branching" and they are not interchangeable.
2. [docs/DECISIONS.md](docs/DECISIONS.md) — what was decided and why, with status and a
   trigger for reopening each one.
3. [poc/RESULTS.md](poc/RESULTS.md) — measurements, and the three silent-corruption bugs that
   building the same thing twice caught.
4. [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) — how the pieces fit. Draft, and derived from
   the decision log rather than authoritative over it.
5. [docs/BACKLOG.md](docs/BACKLOG.md) — deferred work and the MVP definition.

## Three ideas worth knowing before reading the code

- **The raw archive is the source of truth; the index is disposable.** Any schema change is
  `rm index.db` and rebuild, never a migration. Needing a migration means something crept
  into the index that the archive cannot reproduce.
- **Prose is indexed separately from tool traffic.** Tool calls and their output are 91% of
  the text; searching them together buries the content you actually wanted.
- **A conversation is a DAG, not a list.** Editing a message creates a sibling rather than
  overwriting, so the hardest-looking mutation becomes an append.
