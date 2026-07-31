//! Claude Desktop's Cowork and Chat tabs
//! (`~/Library/Application Support/Claude/local-agent-mode-sessions/`).
//!
//! These are the conversations Claude Desktop runs locally. The cloud-synced claude.ai
//! conversations are *not* here — the app's IndexedDB is a React Query cache holding no
//! message bodies, which is why an earlier survey concluded there was nothing local to read
//! (`chat-search-a7k.9`). Those need the fetch route in ADR 21; this module is the half that
//! was already sitting in the clear on disk.
//!
//! **One session is two files**, nested under an account uuid then an org uuid:
//!
//! * `local_<uuid>.json` — session state. Carries the title and the model, and no messages.
//! * `local_<uuid>/audit.jsonl` — the transcript. Carries the messages, and no title.
//!
//! Neither is sufficient alone, and an importer is a pure function over one file's bytes
//! (ADR 1), so it cannot read its sibling. It does not have to: both files resolve to the
//! same `native_id`, so they arrive as two `Conversation` objects with the same id and the
//! index's `COALESCE` upsert folds them — the same mechanism that already assembles a Codex
//! conversation from several thread files (ADR 7). The pairing is exact in the reference
//! archive: 14 state files, 14 transcripts, no orphan either way.
//!
//! The transcript is **Claude Agent SDK stream-json, not Claude Code's JSONL**:
//! `session_id`/`parent_tool_use_id` here against `sessionId`/`parentUuid` there. What the two
//! share is Anthropic's content-block schema, so the block mapping and the injected-markup
//! stripping are borrowed from [`crate::claude_code`] rather than written twice.
//!
//! They are close enough that pointing the `claude-code` glob at this directory looks like a
//! free way to cover it, and the reason not to is *not* that the files would fail to parse —
//! `claude_code` already accepts both spellings of the session key, so they parse. They land
//! under the wrong source id, carrying a `claude --resume` command that cannot reopen a
//! desktop session, and without titles, which live in a `.json` that glob never matches. A
//! test pins this, because the failure looks like success.
//!
//! The archive's glob reaches plugin manifests, caches and settings files as well — 165 JSON
//! files for 14 sessions. Those are declined by *shape*, not by path, so a file that moves
//! still parses the same way.

use crate::claude_code::{clamp_title, epoch_millis, field, flatten, lines, text, title_candidate};
use cs_core::model::{Conversation, Kind, Message, Role, Titles};
use serde_json::Value;
use std::collections::HashSet;

const SOURCE: &str = "claude-desktop";

/// One archived file -> the part of a conversation it knows about.
///
/// Returns `None` for the many files under this root that are not session state and not a
/// transcript, and for a transcript that yields no messages.
pub fn import(logical_path: &str, bytes: &[u8]) -> Option<Conversation> {
    let native_id = session_id(logical_path)?;
    if logical_path.rsplit('/').next() == Some("audit.jsonl") {
        transcript(logical_path, &native_id, bytes)
    } else if logical_path.ends_with(".json") {
        state(&native_id, bytes)
    } else {
        None
    }
}

/// The session uuid, read out of the `local_<uuid>` path segment.
///
/// This is the one key the two files share, and it is how the app itself pairs them — the
/// state file is `local_<uuid>.json` and the transcript is `local_<uuid>/audit.jsonl`. The
/// records' own `session_id` is deliberately *not* used: a transcript carries two of them,
/// one for the desktop session and one for the CLI session running underneath it, and on
/// this corpus the second holds 405 of 424 records. Keying on either would split one
/// conversation in two.
///
/// The first matching segment wins, which is what makes a nested `local_*` — a plugin
/// directory inside a session working directory — resolve to the session that contains it.
fn session_id(logical_path: &str) -> Option<String> {
    logical_path
        .split('/')
        .find_map(|segment| {
            segment.strip_suffix(".json").unwrap_or(segment).strip_prefix("local_")
        })
        .filter(|id| !id.is_empty())
        .map(str::to_string)
}

/// `local_<uuid>.json` -> the attributes the transcript does not carry.
///
/// Contributes no messages, so it can never bring a conversation into existence on its own —
/// only decorate one the transcript created.
fn state(native_id: &str, bytes: &[u8]) -> Option<Conversation> {
    let root: Value = serde_json::from_slice(bytes).ok()?;

    // The shape check, and a strict one: a session state file names itself, and the name it
    // uses is the `local_`-prefixed form of the id the path already gave us. Every other JSON
    // file under this root — plugin manifests, `spaces.json`, MCP caches — fails it.
    if text(&root, "sessionId")?.strip_prefix("local_") != Some(native_id) {
        return None;
    }

    Some(Conversation {
        source: SOURCE.to_string(),
        native_id: native_id.to_string(),
        titles: Titles {
            // Whether the user renamed this or the app generated it is not recorded, and
            // `generated` is the safer reading of the ambiguity: ADR 8 ranks `custom` above
            // it, so guessing `custom` would let an auto-title outrank a real rename
            // arriving from anywhere else.
            custom: None,
            generated: text(&root, "title").map(|t| clamp_title(&t)).filter(|t| !t.is_empty()),
            first_user: None,
        },
        // Deliberately dropped. This field holds either a VM sandbox path
        // (`/sessions/loving-adoring-lovelace`) or the session's own `outputs/` directory
        // inside Application Support — never a directory the user worked in. Both would
        // render as a project path and mean nothing, which is the same reason `gemini.rs`
        // declines to put a project *hash* here.
        cwd: None,
        git_branch: None,
        model: text(&root, "model").filter(|m| !m.is_empty()),
        surface: None,
        forked_from_native_id: None,
        // No documented URL or command reopens one of these by id, and an invented one that
        // fails is worse than none.
        resume_cmd: None,
        head_native_id: None,
        messages: Vec::new(),
    })
}

/// `local_<uuid>/audit.jsonl` -> the messages.
fn transcript(logical_path: &str, native_id: &str, bytes: &[u8]) -> Option<Conversation> {
    let mut messages: Vec<Message> = Vec::new();
    let mut first_user: Option<String> = None;
    let mut model: Option<String> = None;
    let mut prev_native_id: Option<String> = None;
    let mut used_ids: HashSet<String> = HashSet::new();
    let mut seq: i64 = 0;
    let mut saw_record = false;

    for line in lines(bytes) {
        // A trailing partial line is expected, not exceptional: the archiver copies whatever
        // is on disk, including a session still being written.
        let Ok(record) = serde_json::from_str::<Value>(line) else { continue };
        let record_type = text(&record, "type").unwrap_or_default();
        saw_record = true;

        // `system`/`subtype=init` is where the model is stated before any turn happens; the
        // last one wins, so a conversation is labelled with what it ended on. Everything in
        // this arm is harness bookkeeping — `result` totals, `rate_limit_event`,
        // `tool_use_summary` — and none of it is a message.
        if record_type != "user" && record_type != "assistant" {
            if let Some(m) = text(&record, "model").filter(|m| !m.is_empty()) {
                model = Some(m);
            }
            continue;
        }

        // A replay is the same turn re-emitted when a session resumes, and it is the reason
        // uuids repeat within a file at all: on this corpus 19 of 22 repeated uuids in the
        // worst file are replays. Dropping them here means the ordinal fallback below stays
        // reserved for genuine collisions rather than manufacturing a duplicate of every
        // resumed turn.
        if field(&record, "isReplay").as_bool().unwrap_or(false) {
            continue;
        }
        // Harness injection wearing a user turn — skill preambles opening "Base directory
        // for this skill:" and similar. `claude_code` strips the same family of markers.
        if field(&record, "isSynthetic").as_bool().unwrap_or(false) {
            continue;
        }

        let message = field(&record, "message");
        if let Some(m) = text(message, "model").filter(|m| !m.is_empty()) {
            model = Some(m);
        }

        // Set on a subagent's turns to name the tool call that spawned them. Never populated
        // on this corpus, which is why it is read defensively rather than trusted as the
        // subagent signal — but when it does appear it is exactly that.
        let is_sidechain = !field(&record, "parent_tool_use_id").is_null();
        let ts = text(&record, "_audit_timestamp")
            .or_else(|| text(&record, "timestamp"))
            .as_deref()
            .and_then(epoch_millis);

        let id_base = text(&record, "uuid").filter(|u| !u.is_empty());
        let mut emitted_here = 0usize;

        for block in blocks(field(message, "content")) {
            let block_type = block.get("type").and_then(Value::as_str).unwrap_or("");
            // Present only on results the harness judged failed; absent means "not
            // reported", which is not the same as "succeeded" — but false is the only safe
            // reading of silence.
            let is_error = block.get("is_error").and_then(Value::as_bool).unwrap_or(false);

            let (role, kind, body) = match block_type {
                "text" => (role_of(&record_type), Kind::Prose, flatten(field(&block, "text"))),
                "thinking" => {
                    (Role::Assistant, Kind::Reasoning, flatten(field(&block, "thinking")))
                }
                "tool_use" => {
                    let input = field(&block, "input");
                    let args = if input.is_null() { String::new() } else { input.to_string() };
                    let name = block.get("name").and_then(Value::as_str).unwrap_or("");
                    (Role::Assistant, Kind::ToolCall, format!("{name}\n{args}"))
                }
                // Authored by neither side, which is what `Role::Tool` is for; the `user`
                // record it rides on is a transport detail.
                //
                // The sibling top-level `tool_use_result` is *not* read. It agrees with this
                // block only 117 times in 465 on this corpus — it is the richer structured
                // form, not a copy — but indexing both would double-count tool output that
                // ADR 5 already keeps out of default ranking.
                "tool_result" => {
                    (Role::Tool, Kind::ToolResult, flatten(field(&block, "content")))
                }
                // `image`, `document`, and whatever ships next: no searchable text.
                _ => continue,
            };

            let body = body.trim().to_string();
            // Skipping here, before `prev_native_id` moves, re-parents the next message onto
            // the nearest surviving ancestor rather than onto a node never emitted — which
            // `index::on_head_path` would read as a broken chain.
            if body.is_empty() {
                continue;
            }

            if first_user.is_none() && kind == Kind::Prose && role == Role::User {
                first_user = title_candidate(&body);
            }

            let native = message_id(id_base.as_deref(), emitted_here, seq, &mut used_ids);
            messages.push(Message {
                parent_native_id: prev_native_id.replace(native.clone()),
                native_id: native,
                // The logical path, carried in rather than parsed back out (ADR 4).
                thread_key: logical_path.to_string(),
                is_sidechain,
                is_error: is_error && kind == Kind::ToolResult,
                seq,
                role,
                kind,
                ts,
                text: body,
            });
            seq += 1;
            emitted_here += 1;
        }
    }

    // A session opened and abandoned is normal, not an error — but a file that parsed as no
    // records at all is a different thing, and neither should reach the index.
    if messages.is_empty() || !saw_record {
        return None;
    }

    Some(Conversation {
        source: SOURCE.to_string(),
        native_id: native_id.to_string(),
        // The state file supplies the real title and merges over this one; `first_user` is
        // what survives if that file is ever missing.
        titles: Titles { custom: None, generated: None, first_user },
        cwd: None,
        git_branch: None,
        model,
        surface: None,
        forked_from_native_id: None,
        resume_cmd: None,
        // Append-only, so the last message is the head.
        head_native_id: None,
        messages,
    })
}

fn role_of(record_type: &str) -> Role {
    match record_type {
        "user" => Role::User,
        _ => Role::Assistant,
    }
}

/// `message.content` as a list of blocks.
///
/// A plain user turn is a bare string rather than an array — 105 of them on this corpus — so
/// it is lifted into a one-element `text` block instead of being special-cased at every use.
fn blocks(content: &Value) -> Vec<Value> {
    match content {
        Value::String(s) => vec![serde_json::json!({"type": "text", "text": s})],
        Value::Array(a) => a.clone(),
        _ => Vec::new(),
    }
}

/// A message's id: the record's uuid, suffixed when one record yields several blocks.
///
/// One record can produce prose, reasoning and a tool call, so they are `uuid`, `uuid#1`,
/// `uuid#2` — the convention `claude_code` already uses. The ordinal fallback covers a record
/// with no uuid and a uuid already used in this file; without it the index's
/// `INSERT OR IGNORE` would swallow the second one and silently lose a real message.
fn message_id(
    id_base: Option<&str>,
    block_index: usize,
    seq: i64,
    used: &mut HashSet<String>,
) -> String {
    let candidate = match id_base {
        Some(base) if block_index == 0 => base.to_string(),
        Some(base) => format!("{base}#{block_index}"),
        None => format!("msg-{seq:06}"),
    };
    let id = if used.contains(&candidate) { format!("msg-{seq:06}") } else { candidate };
    used.insert(id.clone());
    id
}

#[cfg(test)]
mod tests {
    use super::*;

    const STATE: &str = r#"{"sessionId":"local_abc","title":"Madrid trip planning",
        "model":"claude-opus-4-8","cwd":"/sessions/loving-adoring-lovelace"}"#;

    /// One record per line, which is what a real transcript is.
    ///
    /// Fixtures below are indented across several lines to stay readable, so the wrapping
    /// this file added is taken back out — a literal newline mid-record would split it into
    /// two unparseable halves and the test would pass or fail for the wrong reason.
    fn jsonl(records: &[&str]) -> Vec<u8> {
        records
            .iter()
            .map(|r| r.lines().map(str::trim_start).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
            .into_bytes()
    }

    #[test]
    fn the_state_file_and_the_transcript_resolve_to_one_conversation_id() {
        let state = import("acct/org/local_abc.json", &jsonl(&[STATE])).unwrap();
        let audit = import(
            "acct/org/local_abc/audit.jsonl",
            &jsonl(&[r#"{"type":"user","uuid":"u1","message":{"role":"user","content":"where to eat"}}"#]),
        )
        .unwrap();

        assert_eq!(state.id(), audit.id(), "or the index cannot fold them together");
        assert_eq!(state.id(), "claude-desktop:abc");
    }

    #[test]
    fn the_state_file_carries_the_title_and_the_transcript_carries_the_messages() {
        let state = import("acct/org/local_abc.json", &jsonl(&[STATE])).unwrap();
        assert_eq!(state.titles.resolve(), Some("Madrid trip planning"));
        assert!(state.messages.is_empty(), "a state file must not invent a conversation");
        assert_eq!(state.model.as_deref(), Some("claude-opus-4-8"));
    }

    #[test]
    fn a_sandbox_cwd_is_dropped_rather_than_shown_as_a_project_directory() {
        let state = import("acct/org/local_abc.json", &jsonl(&[STATE])).unwrap();
        assert_eq!(state.cwd, None, "/sessions/... is a VM path, not somewhere the user was");
    }

    #[test]
    fn the_session_id_comes_from_the_path_not_from_the_records() {
        // A transcript carries two session ids — the desktop one and the CLI one underneath
        // it — and the second holds most of the records. Keying on either splits the
        // conversation in two.
        let c = import(
            "acct/org/local_abc/audit.jsonl",
            &jsonl(&[r#"{"type":"user","uuid":"u1","session_id":"other-entirely",
                    "message":{"role":"user","content":"hello"}}"#]),
        )
        .unwrap();
        assert_eq!(c.native_id, "abc");
    }

    #[test]
    fn blocks_map_to_the_right_kind_and_role() {
        let audit = jsonl(&[
            r#"{"type":"assistant","uuid":"a1","message":{"model":"claude-opus-4-8","content":[
                {"type":"thinking","thinking":"weighing options"},
                {"type":"text","text":"Try Sobrino de Botin."},
                {"type":"tool_use","name":"Read","input":{"file_path":"/x.rs"}}]}}"#,
            r#"{"type":"user","uuid":"u2","message":{"role":"user","content":[
                {"type":"tool_result","tool_use_id":"t1","content":"fn main() {}","is_error":true}]}}"#,
        ]);

        let c = import("acct/org/local_abc/audit.jsonl", &audit).unwrap();
        let got: Vec<_> = c
            .messages
            .iter()
            .map(|m| (m.kind, m.role, m.text.as_str(), m.native_id.as_str()))
            .collect();
        assert_eq!(
            got,
            vec![
                (Kind::Reasoning, Role::Assistant, "weighing options", "a1"),
                (Kind::Prose, Role::Assistant, "Try Sobrino de Botin.", "a1#1"),
                (Kind::ToolCall, Role::Assistant, "Read\n{\"file_path\":\"/x.rs\"}", "a1#2"),
                (Kind::ToolResult, Role::Tool, "fn main() {}", "u2"),
            ]
        );
        assert!(c.messages[3].is_error, "the harness said this result failed");
    }

    #[test]
    fn a_bare_string_user_turn_becomes_prose() {
        let c = import(
            "acct/org/local_abc/audit.jsonl",
            &jsonl(&[r#"{"type":"user","uuid":"u1","message":{"role":"user","content":"plan my trip"}}"#]),
        )
        .unwrap();
        assert_eq!(c.messages.len(), 1);
        assert_eq!(c.messages[0].kind, Kind::Prose);
        assert_eq!(c.titles.resolve(), Some("plan my trip"));
    }

    #[test]
    fn a_replayed_turn_is_dropped_rather_than_duplicated() {
        let audit = jsonl(&[
            r#"{"type":"user","uuid":"u1","message":{"role":"user","content":"first ask"}}"#,
            r#"{"type":"user","uuid":"u1","isReplay":true,"message":{"role":"user","content":"first ask"}}"#,
        ]);

        let c = import("acct/org/local_abc/audit.jsonl", &audit).unwrap();
        assert_eq!(c.messages.len(), 1, "a resume replays turns already in the file");
    }

    #[test]
    fn a_genuine_uuid_collision_keeps_both_messages() {
        // Distinct from a replay: same id, different content, and dropping either loses a
        // real turn. The index keys on `native_id`, so they must not collide there.
        let audit = jsonl(&[
            r#"{"type":"user","uuid":"dup","message":{"role":"user","content":"first ask"}}"#,
            r#"{"type":"user","uuid":"dup","message":{"role":"user","content":"second ask"}}"#,
        ]);

        let c = import("acct/org/local_abc/audit.jsonl", &audit).unwrap();
        assert_eq!(c.messages.len(), 2);
        assert_ne!(c.messages[0].native_id, c.messages[1].native_id);
    }

    #[test]
    fn synthetic_harness_turns_do_not_become_messages() {
        let audit = jsonl(&[
            r#"{"type":"user","uuid":"s1","isSynthetic":true,"message":{"role":"user","content":[
                {"type":"text","text":"Base directory for this skill: /mnt/.skills/docx"}]}}"#,
            r#"{"type":"user","uuid":"u1","message":{"role":"user","content":"real question"}}"#,
        ]);

        let c = import("acct/org/local_abc/audit.jsonl", &audit).unwrap();
        assert_eq!(c.messages.len(), 1);
        assert_eq!(c.titles.resolve(), Some("real question"));
    }

    #[test]
    fn a_dropped_block_re_parents_its_successor_onto_the_nearest_emitted_ancestor() {
        // An image block yields no text, so the tool call after it must hang off the prose
        // before it — a parent naming a message that was never emitted would make
        // `index::on_head_path` mark everything above it as off-head-path.
        let audit = jsonl(&[r#"{"type":"assistant","uuid":"a1","message":{"content":[
            {"type":"text","text":"here goes"},
            {"type":"image","source":{"data":"..."}},
            {"type":"tool_use","name":"Read","input":{}}]}}"#]);

        let c = import("acct/org/local_abc/audit.jsonl", &audit).unwrap();
        assert_eq!(c.messages.len(), 2);
        assert_eq!(c.messages[1].parent_native_id.as_deref(), Some(c.messages[0].native_id.as_str()));
    }

    #[test]
    fn the_model_is_read_from_the_init_record_when_no_turn_states_one() {
        let audit = jsonl(&[
            r#"{"type":"system","subtype":"init","model":"claude-sonnet-4-6","cwd":"/sessions/x"}"#,
            r#"{"type":"user","uuid":"u1","message":{"role":"user","content":"hi"}}"#,
        ]);

        let c = import("acct/org/local_abc/audit.jsonl", &audit).unwrap();
        assert_eq!(c.model.as_deref(), Some("claude-sonnet-4-6"));
    }

    #[test]
    fn harness_bookkeeping_records_are_not_messages() {
        let audit = jsonl(&[
            r#"{"type":"system","subtype":"init","model":"claude-opus-4-8"}"#,
            r#"{"type":"rate_limit_event","rate_limit_info":{}}"#,
            r#"{"type":"tool_use_summary","summary":"read three files"}"#,
            r#"{"type":"result","subtype":"success","result":"done","is_error":false}"#,
        ]);

        assert!(
            import("acct/org/local_abc/audit.jsonl", &audit).is_none(),
            "no turns means no conversation, however much bookkeeping surrounds it"
        );
    }

    #[test]
    fn files_that_are_not_session_state_are_declined_by_shape() {
        // The archive's glob reaches plugin manifests and caches nested inside a session's
        // own working directory, so the path alone cannot rule them out.
        for (path, body) in [
            ("acct/org/local_abc/cowork_plugins/local_x.json", r#"{"name":"a-plugin"}"#),
            ("acct/org/local_abc/spaces.json", r#"{"spaces":[]}"#),
            ("acct/org/local_abc.json", r#"{"sessionId":"local_somethingelse"}"#),
        ] {
            assert!(import(path, body.as_bytes()).is_none(), "{path} must be declined");
        }
    }

    #[test]
    fn widening_the_claude_code_glob_would_mislabel_these_rather_than_skip_them() {
        // Worth pinning because the obvious guess is wrong in the reassuring direction. The
        // two formats look different enough — `session_id` here, `sessionId` there — that
        // pointing `claude-code`'s `**/*.jsonl` glob at this directory seems like it would
        // simply import nothing. It does not: `claude_code` already accepts both spellings,
        // so it parses these fine and files them under the wrong source, with a
        // `claude --resume` command that does not reopen a desktop session, and without the
        // titles, which live in a `.json` its glob never matches.
        //
        // So the reason to keep two importers is not that one cannot read the other's files.
        // It is that the one that can produces confidently wrong rows.
        let desktop = jsonl(&[
            r#"{"type":"user","uuid":"u1","session_id":"s","message":{"role":"user","content":"hello"}}"#,
        ]);

        let stolen = crate::claude_code::import("acct/org/local_abc/audit.jsonl", &desktop)
            .expect("claude_code parses this, which is the hazard");
        assert_eq!(stolen.source, "claude-code", "filed under a source it does not belong to");
        assert!(
            stolen.resume_cmd.is_some_and(|cmd| cmd.starts_with("claude --resume")),
            "and offered a resume command that cannot reopen a desktop session"
        );

        let ours = import("acct/org/local_abc/audit.jsonl", &desktop).unwrap();
        assert_eq!(ours.source, SOURCE);
        assert_eq!(ours.resume_cmd, None, "no command reopens one of these, so none is offered");
    }

    #[test]
    fn a_file_outside_a_session_directory_is_not_a_conversation() {
        assert!(import("acct/org/settings.json", r#"{"sessionId":"local_abc"}"#.as_bytes()).is_none());
    }

    #[test]
    fn a_truncated_trailing_line_does_not_lose_the_records_before_it() {
        let audit = jsonl(&[
            r#"{"type":"user","uuid":"u1","message":{"role":"user","content":"complete turn"}}"#,
            r#"{"type":"assistant","uuid":"a1","message":{"content":[{"type":"text","tex"#,
        ]);
        let c = import("acct/org/local_abc/audit.jsonl", &audit).unwrap();
        assert_eq!(c.messages.len(), 1);
    }

    #[test]
    fn timestamps_come_from_the_audit_clock() {
        let c = import(
            "acct/org/local_abc/audit.jsonl",
            &jsonl(&[r#"{"type":"user","uuid":"u1","_audit_timestamp":"2026-04-13T03:17:36.939Z",
                    "message":{"role":"user","content":"hi"}}"#]),
        )
        .unwrap();
        assert_eq!(c.messages[0].ts, Some(1_776_050_256_939));
    }
}
