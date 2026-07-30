//! Golden-file coverage for the importers (bead chat-search-4ar.3).
//!
//! The unit tests inside each importer pin one behaviour at a time and say so in an
//! assertion. These pin the *whole* `Conversation` — every field, every message, every edge
//! — against a committed file, so a change nobody wrote an assertion for still shows up as a
//! diff. That is the half of the coverage a hand-written assertion cannot give: the fields
//! it did not think to mention.
//!
//! # Layout
//!
//! ```text
//! tests/fixtures/<source>/<case>/<logical path…>   inputs, mirroring the source tree
//! tests/fixtures/<source>/<case>.golden.json       the expected import of that case
//! ```
//!
//! The input tree mirrors the shape the files have in the *source* (`2026/06/02/rollout-….jsonl`,
//! `projects/<slug>/<session>.jsonl`) because the path is an input, not packaging: Codex
//! derives `thread_key` from the file stem and Claude Code from the whole logical path
//! (ADR 2, ADR 4). Renaming a fixture file therefore changes the expected output, which is
//! why the goldens carry `logical_path` beside each import.
//!
//! `<source>` is both the directory name and the importer that runs on it, so a fixture
//! cannot quietly be fed to the wrong parser — an unknown source directory is a panic, not a
//! skip.
//!
//! The four cases here are the shapes chat-search-4ar.3 called for, not the whole matrix:
//! anything a parser bug needs later (`gemini-cli`, a second ChatGPT branch shape) is a new
//! directory, an entry in `CASES` and a `check` — no harness changes. Everything under a case
//! directory is input, so a case is extended by dropping another file into it; the goldens
//! sit *beside* the directory rather than inside it for exactly that reason.
//!
//! # Regenerating
//!
//! ```sh
//! CS_UPDATE_GOLDEN=1 cargo test -p cs-import --test golden
//! ```
//!
//! Rewrites every golden from the current importers and passes. **Read the diff before
//! committing it**: this test only proves the output did not change by accident, so an
//! unreviewed regeneration launders a regression into the expected file. A golden nobody can
//! regenerate gets deleted the first time it fails, which is why the command lives here
//! rather than in a doc nobody opens.
//!
//! # Determinism
//!
//! The file list is sorted by the *string* form of the logical path before anything is
//! imported (ADR 7a): `read_dir` order is not defined, and `Path: Ord` compares components,
//! so `<id>/subagents/x.jsonl` sorts before `<id>.jsonl` under one and after under the other.
//! Serialisation goes through `serde_json`, whose maps are `BTreeMap`s here (nothing in the
//! workspace turns on `preserve_order`), so key order is alphabetical and stable across
//! builds; struct fields keep declaration order either way.

use serde_json::{json, Value};
use std::path::{Path, PathBuf};

use cs_import::{chatgpt_export, claude_code, codex};

/// Set to anything to rewrite the goldens instead of comparing against them.
const UPDATE_ENV: &str = "CS_UPDATE_GOLDEN";

/// A Codex subagent: two rollout files, one `session_id`, and the file name as the only
/// thing that tells the threads apart (ADR 2).
const SUBAGENT: Case = Case { source: "codex", name: "subagent-thread" };
/// A rollout that declares `forked_from_id`, and replays its parent's history — including
/// the parent's own `session_meta` — after its own.
const FORK: Case = Case { source: "codex", name: "declared-fork" };
/// A session renamed by hand over a generated title, with the opening user turn underneath
/// as the last resort (ADR 8).
const RENAMED: Case = Case { source: "claude-code", name: "renamed-session" };
/// The DAG case: an edited message that appended a sibling rather than overwriting, with
/// `current_node` naming the branch the UI shows (ADR 4).
const EDIT_BRANCH: Case = Case { source: "chatgpt-export", name: "edit-branch" };

const CASES: [Case; 4] = [SUBAGENT, FORK, RENAMED, EDIT_BRANCH];

#[derive(Clone, Copy)]
struct Case {
    source: &'static str,
    name: &'static str,
}

#[test]
fn codex_subagent_thread_matches_its_golden() {
    check(SUBAGENT);
}

#[test]
fn codex_declared_fork_matches_its_golden() {
    check(FORK);
}

#[test]
fn claude_code_renamed_session_matches_its_golden() {
    check(RENAMED);
}

#[test]
fn chatgpt_edit_branch_matches_its_golden() {
    check(EDIT_BRANCH);
}

#[test]
fn every_fixture_case_has_a_test_and_a_golden() {
    // A fixture with no test is a file that looks like coverage and is not, and a golden
    // with no fixture is a file that can never fail. Both are caught by comparing what is on
    // disk against the constants above.
    let mut on_disk: Vec<String> = Vec::new();
    for source in sorted_dir(&fixtures()) {
        for case in sorted_dir(&fixtures().join(&source)) {
            if fixtures().join(&source).join(&case).is_dir() {
                on_disk.push(format!("{source}/{case}"));
            }
        }
    }
    on_disk.sort();

    let mut declared: Vec<String> =
        CASES.iter().map(|c| format!("{}/{}", c.source, c.name)).collect();
    declared.sort();
    assert_eq!(on_disk, declared, "fixture directories and the CASES table have drifted apart");

    for case in CASES {
        let golden = golden_path(case);
        assert!(golden.is_file(), "missing golden {}; run {UPDATE_ENV}=1", golden.display());
    }
}

#[test]
fn importing_a_case_twice_produces_the_same_document() {
    // Ids and `seq` must be a pure function of (path, bytes) or every re-import churns the
    // index. Hash iteration order is the usual way that breaks, and it only shows up when
    // the same input is walked twice in one process.
    for case in CASES {
        assert_eq!(document(&import_case(case)), document(&import_case(case)), "{}", case.name);
    }
}

#[test]
fn a_regression_in_the_output_fails_the_diff() {
    // The acceptance criterion the goldens exist for: a change in importer output must fail
    // here rather than pass silently. Standing in for the change is a perturbation of the
    // imported document — the subagent flag dropped, which is exactly what a naive rewrite
    // of `Meta::read` would do — checked against the committed golden through the same
    // comparison the real tests use.
    let mut perturbed = import_case(SUBAGENT);
    let flag = &mut perturbed[1]["conversations"][0]["messages"][0]["is_sidechain"];
    assert_eq!(*flag, json!(true), "the subagent fixture must start out as a sidechain");
    *flag = json!(false);

    let want = std::fs::read_to_string(golden_path(SUBAGENT)).expect("golden is committed");
    assert_ne!(want, document(&perturbed), "the golden diff would have missed a lost sidechain");
}

// --------------------------------------------------------- the four shapes, stated

// The goldens above pin everything; these four say out loud what each fixture exists to
// prove, in the terms the model uses rather than in JSON.

#[test]
fn a_codex_subagent_is_one_conversation_split_across_two_threads() {
    const PARENT: &str = "2026/06/02/rollout-2026-06-02T09-15-04-thread-parent.jsonl";
    const CHILD: &str = "2026/06/02/rollout-2026-06-02T09-17-31-thread-explore.jsonl";

    let parent =
        codex::import(PARENT, &input(SUBAGENT, PARENT)).expect("the parent rollout imports");
    let child =
        codex::import(CHILD, &input(SUBAGENT, CHILD)).expect("the subagent rollout imports");

    assert_eq!(parent.native_id, child.native_id, "a subagent reuses the parent's session id");
    assert_ne!(parent.messages[0].thread_key, child.messages[0].thread_key);
    assert!(!parent.messages.iter().any(|m| m.is_sidechain));
    // The subagent file replays its parent's `session_meta` on line 2. First meta wins, or
    // the thread reverts to `thread_source: user` and stops being a sidechain at all.
    assert!(child.messages.iter().all(|m| m.is_sidechain));
    // Ordinals restart per file, so the ids would collide if they were not thread-scoped.
    assert_eq!(parent.messages[0].seq, child.messages[0].seq);
    assert_ne!(parent.messages[0].native_id, child.messages[0].native_id);
    // Every message is on a sidechain, so there is no main-thread leaf to fall back to.
    assert_eq!(child.resolved_head(), None);
}

#[test]
fn a_declared_fork_records_the_conversation_it_came_from() {
    const ROLLOUT: &str = "2026/06/03/rollout-2026-06-03T11-02-19-thread-fork.jsonl";
    let forked =
        codex::import(ROLLOUT, &input(FORK, ROLLOUT)).expect("the forked rollout imports");

    assert_eq!(forked.native_id, "conv-e5f6", "the replayed parent meta must not win");
    assert_eq!(forked.forked_from_native_id.as_deref(), Some("conv-a1b2"));
    assert_eq!(forked.surface.as_deref(), Some("codex_desktop"));
}

#[test]
fn a_renamed_session_resolves_to_the_rename() {
    const TRANSCRIPT: &str = "projects/-home-dev-example-project/sess-7f80.jsonl";
    let renamed = claude_code::import(TRANSCRIPT, &input(RENAMED, TRANSCRIPT))
        .expect("the renamed transcript imports");

    // All three candidates are present, so the fold is being exercised rather than defaulted
    // through (ADR 8), and the last `custom-title` is the one that wins.
    assert_eq!(renamed.titles.custom.as_deref(), Some("Widget cache rename bug"));
    assert_eq!(renamed.titles.generated.as_deref(), Some("Cache misses after renaming a widget"));
    assert_eq!(
        renamed.titles.first_user.as_deref(),
        Some("why does the widget cache miss after a rename?")
    );
    assert_eq!(renamed.titles.resolve(), Some("Widget cache rename bug"));
}

#[test]
fn an_edited_chatgpt_turn_keeps_both_branches_and_heads_on_the_edit() {
    let convs = chatgpt_export::import_all(&input(EDIT_BRANCH, "conversations-000.json"));
    let c = &convs[0];

    let ids: Vec<&str> = c.messages.iter().map(|m| m.native_id.as_str()).collect();
    assert_eq!(ids, ["node-u1", "node-a1", "node-u2", "node-a2", "node-u2b", "node-a2b"]);

    let parent = |id: &str| {
        c.messages.iter().find(|m| m.native_id == id).unwrap().parent_native_id.clone()
    };
    // The fixture gives `node-u2b` a *disagreeing* `metadata.parent_id` — the client-side
    // breadcrumb that differs from the real edge 4,948 times in the reference export. The
    // edge has to come from the node's `parent`, or the two siblings stop being siblings.
    assert_eq!(parent("node-u2").as_deref(), Some("node-a1"));
    assert_eq!(parent("node-u2b").as_deref(), Some("node-a1"));

    // `head_native_id` and the parent edges are the two things `cs_core::index::on_head_path`
    // walks, so pinning them here is what keeps "this hit is on an abandoned branch" honest.
    assert_eq!(c.head_native_id.as_deref(), Some("node-a2b"));
    let abandoned = c.messages.iter().any(|m| m.text.contains("name-keyed"));
    assert!(abandoned, "the branch that was edited away stays imported and searchable");
}

// ------------------------------------------------------------------ the harness

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("fixtures")
}

fn golden_path(case: Case) -> PathBuf {
    fixtures().join(case.source).join(format!("{}.golden.json", case.name))
}

/// One fixture file, addressed the way the importer addresses it: by the logical path it has
/// *inside* the case, so a test names that path once and reads with the same string it
/// passes to the parser.
fn input(case: Case, logical: &str) -> Vec<u8> {
    let path = fixtures().join(case.source).join(case.name).join(logical);
    std::fs::read(&path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()))
}

/// Every file of a case, imported by the source's own importer, in logical-path order.
///
/// One entry per file rather than one per conversation: which file a conversation came from
/// is itself under test — two files of one Codex session must not merge here, and must not
/// collide (ADR 2).
fn import_case(case: Case) -> Value {
    let root = fixtures().join(case.source).join(case.name);
    let mut files: Vec<(String, PathBuf)> = Vec::new();
    collect(&root, &root, &mut files);
    // ADR 7a. The sort is the point: `read_dir` hands back whatever the filesystem feels
    // like, and the comparison must be on the string, not on `Path`.
    files.sort_by(|a, b| a.0.cmp(&b.0));
    assert!(!files.is_empty(), "fixture case {}/{} has no input files", case.source, case.name);

    let entries: Vec<Value> = files
        .iter()
        .map(|(logical, path)| {
            let bytes = std::fs::read(path)
                .unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
            let imported: Vec<Value> = match case.source {
                "codex" => codex::import(logical, &bytes).into_iter().map(value).collect(),
                "claude-code" => {
                    claude_code::import(logical, &bytes).into_iter().map(value).collect()
                }
                "chatgpt-export" => {
                    chatgpt_export::import_all(&bytes).into_iter().map(value).collect()
                }
                other => panic!("no importer is wired for fixture source `{other}`"),
            };
            json!({"logical_path": logical, "conversations": imported})
        })
        .collect();

    Value::Array(entries)
}

/// Serialise through the model's own `Serialize`, so a field added to `Conversation` or
/// `Message` lands in every golden on the next regeneration instead of going uncovered.
fn value(c: cs_core::Conversation) -> Value {
    serde_json::to_value(c).expect("the model derives Serialize")
}

fn document(doc: &Value) -> String {
    let mut s = serde_json::to_string_pretty(doc).expect("a Value always serialises");
    s.push('\n');
    s
}

fn collect(root: &Path, dir: &Path, out: &mut Vec<(String, PathBuf)>) {
    let entries =
        std::fs::read_dir(dir).unwrap_or_else(|e| panic!("reading {}: {e}", dir.display()));
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(root, &path, out);
        } else {
            // The logical path is what the importer sees, so it is relative to the case root
            // and always `/`-separated — the same normalisation the archive reader does.
            let logical = path.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/");
            out.push((logical, path));
        }
    }
}

fn check(case: Case) {
    let got = document(&import_case(case));
    let path = golden_path(case);

    if std::env::var_os(UPDATE_ENV).is_some() {
        std::fs::write(&path, &got).unwrap_or_else(|e| panic!("writing {}: {e}", path.display()));
        return;
    }

    let want = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "reading {}: {e}\nregenerate with \
             `{UPDATE_ENV}=1 cargo test -p cs-import --test golden`",
            path.display()
        )
    });
    if want == got {
        return;
    }
    panic!("{}", report(&path, &want, &got));
}

/// The first line that differs, which is the one worth reading. A whole-document dump of a
/// golden this size hides the change it is supposed to show.
fn report(path: &Path, want: &str, got: &str) -> String {
    let (mut want_lines, mut got_lines) = (want.lines(), got.lines());
    let mut line = 0;
    loop {
        line += 1;
        match (want_lines.next(), got_lines.next()) {
            (None, None) => break,
            (w, g) if w == g => continue,
            (w, g) => {
                return format!(
                    "{} differs at line {line}\n  expected: {}\n  actual:   {}\n\
                     regenerate with `{UPDATE_ENV}=1 cargo test -p cs-import --test golden`, \
                     then read the diff before committing it",
                    path.display(),
                    w.unwrap_or("<end of file>"),
                    g.unwrap_or("<end of file>"),
                );
            }
        }
    }
    // Every line matched, so the two differ in trailing bytes `lines()` does not yield —
    // a stripped final newline, most likely.
    format!(
        "{} differs from the import only in trailing whitespace ({} bytes vs {}); \
         regenerate with `{UPDATE_ENV}=1 cargo test -p cs-import --test golden`",
        path.display(),
        want.len(),
        got.len(),
    )
}

fn sorted_dir(dir: &Path) -> Vec<String> {
    let entries =
        std::fs::read_dir(dir).unwrap_or_else(|e| panic!("reading {}: {e}", dir.display()));
    let mut names: Vec<String> =
        entries.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    names.sort();
    names
}
