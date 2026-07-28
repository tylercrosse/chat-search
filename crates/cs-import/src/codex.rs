//! Codex rollout files (`~/.codex/sessions/<y>/<m>/<d>/rollout-<ts>-<id>.jsonl`).
//!
//! One rollout file is one *thread*, not one conversation. A subagent runs in its own
//! rollout file while reusing the parent's `session_id`, so several files fold into one
//! conversation and per-file ordinals are not unique across it. Message ids are therefore
//! scoped by the rollout file stem (ADR 2).
//!
//! Codex threads are linear — there is no per-event parent pointer and no edit-branching —
//! so the DAG this produces is a chain: each message's parent is the previous message *in
//! the same file*.

use cs_core::model::{Conversation, Kind, Message, Role, Titles};
use serde_json::Value;

const SOURCE: &str = "codex";
const TITLE_MAX_CHARS: usize = 80;

/// One archived rollout file -> one conversation-shaped fragment.
///
/// `logical_path` is the path the file had in the source; only its file name is used, and
/// only to derive the thread key. Returns `None` when the file declares no session id or
/// contributes no messages — an empty or header-only rollout is normal, not an error.
pub fn import(logical_path: &str, bytes: &[u8]) -> Option<Conversation> {
    let thread_key = thread_key(logical_path);

    let mut meta: Option<Meta> = None;
    let mut model: Option<String> = None;
    let mut messages: Vec<Message> = Vec::new();
    let mut first_user: Option<String> = None;
    let mut prev_native_id: Option<String> = None;
    let mut seq: i64 = 0;

    for line in lines(bytes) {
        // A half-written final line is expected while a session is still in flight, and a
        // rollout also carries lines this importer has no business understanding. Neither
        // is an error: skip and keep going.
        let Ok(record) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let Some(payload) = field(&record, "payload").filter(|p| p.is_object()) else {
            continue;
        };
        let record_type = text_field(&record, "type").unwrap_or_default();
        let payload_type = text_field(payload, "type").unwrap_or_default();

        // Only the *first* session_meta describes this file. A forked or subagent rollout
        // replays its parent's history, and that history includes the parent's own
        // session_meta — taking the last one instead assigns the parent's session id to the
        // child (24 of 673 files on the reference corpus).
        if record_type == "session_meta" {
            if meta.is_none() {
                meta = Some(Meta::read(payload));
            }
            continue;
        }
        // `session_meta.model_provider` is the vendor ("openai"), not the model. The model
        // name lives on turn_context and can change mid-conversation; the last one wins, so
        // the conversation is labelled with what it ended on.
        if record_type == "turn_context" {
            if let Some(m) = text_field(payload, "model") {
                model = Some(m);
            }
            continue;
        }

        // Codex's analogue of Claude Code's `isMeta`: a synthetic turn the harness injects,
        // carrying an <environment_context><cwd>…</cwd></environment_context> blob rather
        // than anything the user typed. It is the first user message in ~11% of files, so
        // keeping it would make one conversation in nine title itself with a path dump.
        if record_type == "event_msg"
            && payload_type == "user_message"
            && text_field(payload, "kind").as_deref() == Some("environment_context")
        {
            continue;
        }

        let (role, kind, text) = match (record_type.as_str(), payload_type.as_str()) {
            ("event_msg", "user_message") => {
                (Role::User, Kind::Prose, flatten_field(payload, "message"))
            }
            ("event_msg", "agent_message") => {
                (Role::Assistant, Kind::Prose, flatten_field(payload, "message"))
            }
            ("response_item", "reasoning") => (
                Role::Assistant,
                Kind::Reasoning,
                flatten(first_present(payload, &["summary", "content"])),
            ),
            ("response_item", "function_call") | ("response_item", "custom_tool_call") => (
                Role::Assistant,
                Kind::ToolCall,
                format!(
                    "{}\n{}",
                    text_field(payload, "name").unwrap_or_default(),
                    // `function_call` spells it `arguments`, `custom_tool_call` spells it
                    // `input`; neither carries the other's key.
                    flatten(first_present(payload, &["arguments", "input"])),
                ),
            ),
            ("response_item", "function_call_output")
            | ("response_item", "custom_tool_call_output") => {
                (Role::Tool, Kind::ToolResult, flatten_field(payload, "output"))
            }
            // `response_item`/`message` duplicates the `event_msg` prose, and its
            // `developer` variant is a ~17 KB system prompt. Everything else — token
            // counts, patch results, task lifecycle — is not conversation content.
            _ => continue,
        };

        let text = text.trim();
        if text.is_empty() {
            continue;
        }

        if kind == Kind::Prose && role == Role::User && first_user.is_none() {
            first_user = Some(title_from(text));
        }

        let native_id = format!("{thread_key}:{seq:06}");
        messages.push(Message {
            parent_native_id: prev_native_id.replace(native_id.clone()),
            native_id,
            thread_key: thread_key.clone(),
            // Stamped after the loop: it is a property of the file, not of the message.
            is_sidechain: false,
            seq,
            role,
            kind,
            ts: text_field(&record, "timestamp").as_deref().and_then(epoch_millis),
            text: text.to_string(),
        });
        seq += 1;
    }

    let meta = meta?;
    let session_id = meta.session_id?;
    if messages.is_empty() {
        return None;
    }
    for m in &mut messages {
        m.is_sidechain = meta.is_sidechain;
    }

    Some(Conversation {
        source: SOURCE.to_string(),
        native_id: session_id.clone(),
        // Codex has no rename and no auto-title; the opening user message is all there is.
        titles: Titles { custom: None, generated: None, first_user },
        cwd: meta.cwd,
        git_branch: meta.git_branch,
        model,
        surface: meta.surface,
        forked_from_native_id: meta.forked_from,
        resume_cmd: Some(format!("codex resume {session_id}")),
        // Codex only ever appends, so the last main-thread message is the head.
        head_native_id: None,
        messages,
    })
}

/// The parts of `session_meta` that survive into the normalized model.
struct Meta {
    session_id: Option<String>,
    cwd: Option<String>,
    git_branch: Option<String>,
    surface: Option<String>,
    forked_from: Option<String>,
    is_sidechain: bool,
}

impl Meta {
    fn read(payload: &Value) -> Self {
        Meta {
            // A subagent's `session_id` is the conversation it belongs to while `id` is its
            // own thread; older rollouts omit `session_id` and only carry `id`, which is
            // then both.
            session_id: text_field(payload, "session_id").or_else(|| text_field(payload, "id")),
            cwd: text_field(payload, "cwd"),
            git_branch: field(payload, "git").and_then(|g| text_field(g, "branch")),
            surface: text_field(payload, "originator"),
            forked_from: text_field(payload, "forked_from_id"),
            // Two spellings in the wild: newer rollouts set `thread_source`, older ones only
            // widen `source` from a string to an object keyed by how the thread was spawned.
            is_sidechain: text_field(payload, "thread_source").as_deref() == Some("subagent")
                || field(payload, "source").and_then(|s| field(s, "subagent")).is_some(),
        }
    }
}

/// The rollout file stem, which is what identifies a thread.
///
/// Codex subagents share their parent's `session_id` and are told apart only by which file
/// they live in, so the file name — not the session id — is the thread key (ADR 2).
fn thread_key(logical_path: &str) -> String {
    let name = logical_path.rsplit('/').next().unwrap_or(logical_path);
    name.strip_suffix(".jsonl").unwrap_or(name).to_string()
}

/// Non-empty JSONL lines as UTF-8. Trailing `\r` and undecodable lines are dropped.
fn lines(buf: &[u8]) -> impl Iterator<Item = &str> {
    buf.split(|&b| b == b'\n')
        .filter_map(|l| std::str::from_utf8(l).ok())
        .map(str::trim)
        .filter(|l| !l.is_empty())
}

/// `Value::get` returns `Some(Value::Null)` for an explicit JSON null, so the obvious
/// `get("a").or_else(|| get("b"))` never falls through on one. Every lookup goes through
/// here, which collapses "absent" and "null" into `None` and makes the fallbacks work.
fn field<'a>(v: &'a Value, key: &str) -> Option<&'a Value> {
    match v.get(key) {
        None | Some(Value::Null) => None,
        Some(found) => Some(found),
    }
}

fn text_field(v: &Value, key: &str) -> Option<String> {
    field(v, key).and_then(Value::as_str).map(str::to_string)
}

/// The first of `keys` that is present and not null.
fn first_present<'a>(v: &'a Value, keys: &[&str]) -> Option<&'a Value> {
    keys.iter().find_map(|k| field(v, k))
}

fn flatten_field(v: &Value, key: &str) -> String {
    flatten(field(v, key))
}

/// Collapse arbitrary content — string, block, or array of blocks — into searchable text.
fn flatten(v: Option<&Value>) -> String {
    match v {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(a)) => a
            .iter()
            .map(|item| flatten(Some(item)))
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("\n"),
        Some(Value::Object(o)) => {
            for k in ["text", "content", "output", "message", "thinking"] {
                if let Some(inner) = o.get(k) {
                    return flatten(Some(inner));
                }
            }
            String::new()
        }
        Some(other) => other.to_string(),
    }
}

fn title_from(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(TITLE_MAX_CHARS).collect()
}

/// RFC3339 -> epoch milliseconds.
///
/// Hand-rolled rather than pulling a date crate into an importer: this is the only date
/// this crate reads, the shape is fixed (`2026-07-28T14:46:55.123Z` across every observed
/// record), and a dependency here would be paid for by every source.
///
/// Accepts an optional fractional part of any length and either `Z` or a `±HH:MM` offset.
fn epoch_millis(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() < 19 || !matches!(b[10], b'T' | b't' | b' ') {
        return None;
    }
    if b[4] != b'-' || b[7] != b'-' || b[13] != b':' || b[16] != b':' {
        return None;
    }
    let year = digits(b, 0, 4)?;
    let month = digits(b, 5, 2)?;
    let day = digits(b, 8, 2)?;
    let hour = digits(b, 11, 2)?;
    let minute = digits(b, 14, 2)?;
    let second = digits(b, 17, 2)?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    if hour > 23 || minute > 59 || second > 60 {
        return None;
    }

    let mut i = 19;
    let mut millis_frac = 0i64;
    if b.get(i) == Some(&b'.') {
        i += 1;
        let start = i;
        while i < b.len() && b[i].is_ascii_digit() {
            // Digits past the third are real but below our resolution, so they are read and
            // discarded rather than left to be misparsed as an offset.
            if i - start < 3 {
                millis_frac = millis_frac * 10 + i64::from(b[i] - b'0');
            }
            i += 1;
        }
        if i == start {
            return None;
        }
        for _ in (i - start)..3 {
            millis_frac *= 10;
        }
    }

    // Absent zone means local time, which we cannot resolve, so treat it as UTC.
    let offset_seconds = match b.get(i) {
        None | Some(b'Z') | Some(b'z') => 0,
        Some(sign @ (b'+' | b'-')) => {
            let sign = if *sign == b'-' { -1 } else { 1 };
            let oh = digits(b, i + 1, 2)?;
            // `+HH:MM` and `+HHMM` are both legal; only the separator moves.
            let mm_at = if b.get(i + 3) == Some(&b':') { i + 4 } else { i + 3 };
            let om = digits(b, mm_at, 2)?;
            sign * (oh * 3600 + om * 60)
        }
        Some(_) => return None,
    };

    let days = days_from_civil(year, month, day);
    let seconds = days * 86_400 + hour * 3_600 + minute * 60 + second - offset_seconds;
    Some(seconds * 1_000 + millis_frac)
}

/// Days since 1970-01-01 for a proleptic Gregorian date (Howard Hinnant's `days_from_civil`).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn digits(b: &[u8], start: usize, len: usize) -> Option<i64> {
    let slice = b.get(start..start + len)?;
    let mut n = 0i64;
    for &c in slice {
        if !c.is_ascii_digit() {
            return None;
        }
        n = n * 10 + i64::from(c - b'0');
    }
    Some(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Fixtures are synthetic on purpose. Real rollouts carry secrets, and a golden file cut
    // from one cannot be shared or reviewed.

    const MAIN_PATH: &str = "2026/07/28/rollout-2026-07-28T14-46-55-019f-main.jsonl";

    const MAIN: &str = r#"
{"timestamp":"2026-07-28T14:46:55.123Z","type":"session_meta","payload":{"id":"019f-main","originator":"codex_vscode","thread_source":"user","source":"cli","cwd":"/w/proj","git":{"branch":"feature/x","commit_hash":"deadbeef"}}}
{"timestamp":"2026-07-28T14:46:56.000Z","type":"turn_context","payload":{"model":"gpt-5-codex","effort":"medium"}}
{"timestamp":"2026-07-28T14:46:57.000Z","type":"event_msg","payload":{"type":"user_message","message":"  summarize   the\n  changelog  "}}
{"timestamp":"2026-07-28T14:46:58.000Z","type":"response_item","payload":{"type":"reasoning","summary":[{"type":"summary_text","text":"read the file first"}],"content":null}}
{"timestamp":"2026-07-28T14:46:59.000Z","type":"response_item","payload":{"type":"function_call","name":"shell","arguments":"{\"cmd\":\"cat CHANGELOG.md\"}","call_id":"c1"}}
{"timestamp":"2026-07-28T14:47:00.000Z","type":"response_item","payload":{"type":"function_call_output","call_id":"c1","output":"1.2.0\nfaster startup"}}
{"timestamp":"2026-07-28T14:47:01.000Z","type":"event_msg","payload":{"type":"token_count","info":{"total":1234}}}
{"timestamp":"2026-07-28T14:47:02.000Z","type":"response_item","payload":{"type":"message","role":"developer","content":[{"type":"input_text","text":"you are a coding agent ..."}]}}
{"timestamp":"2026-07-28T14:47:03.000Z","type":"response_item","payload":{"type":"reasoning","summary":[],"content":null,"encrypted_content":"gAAAA"}}
{"timestamp":"2026-07-28T14:47:04.000Z","type":"event_msg","payload":{"type":"agent_message","message":"1.2.0 made startup faster."}}
"#;

    fn conv(path: &str, jsonl: &str) -> Conversation {
        import(path, jsonl.as_bytes()).expect("fixture should import")
    }

    #[test]
    fn main_thread_maps_kinds_keeps_order_and_chains_parents() {
        let c = conv(MAIN_PATH, MAIN);

        assert_eq!(c.source, "codex");
        assert_eq!(c.native_id, "019f-main");
        assert_eq!(c.resume_cmd.as_deref(), Some("codex resume 019f-main"));
        assert_eq!(c.surface.as_deref(), Some("codex_vscode"));
        assert_eq!(c.cwd.as_deref(), Some("/w/proj"));
        assert_eq!(c.git_branch.as_deref(), Some("feature/x"));
        assert_eq!(c.model.as_deref(), Some("gpt-5-codex"));
        assert_eq!(c.forked_from_native_id, None);
        assert_eq!(c.head_native_id, None);

        // token_count, the developer-role `message`, and the summary-less reasoning are all
        // dropped; what is left keeps source order.
        let shape: Vec<_> = c.messages.iter().map(|m| (m.role, m.kind)).collect();
        assert_eq!(
            shape,
            vec![
                (Role::User, Kind::Prose),
                (Role::Assistant, Kind::Reasoning),
                (Role::Assistant, Kind::ToolCall),
                (Role::Tool, Kind::ToolResult),
                (Role::Assistant, Kind::Prose),
            ]
        );

        let stem = "rollout-2026-07-28T14-46-55-019f-main";
        let ids: Vec<_> = c.messages.iter().map(|m| m.native_id.as_str()).collect();
        assert_eq!(
            ids,
            vec![
                format!("{stem}:000000"),
                format!("{stem}:000001"),
                format!("{stem}:000002"),
                format!("{stem}:000003"),
                format!("{stem}:000004"),
            ]
        );

        // Linear chain: first message is a root, every later one points at its predecessor.
        assert_eq!(c.messages[0].parent_native_id, None);
        for pair in c.messages.windows(2) {
            assert_eq!(pair[1].parent_native_id.as_deref(), Some(pair[0].native_id.as_str()));
        }
        for (i, m) in c.messages.iter().enumerate() {
            assert_eq!(m.seq, i as i64);
            assert_eq!(m.thread_key, stem);
            assert!(!m.is_sidechain);
        }

        assert_eq!(c.messages[2].text, "shell\n{\"cmd\":\"cat CHANGELOG.md\"}");
        assert_eq!(c.messages[1].text, "read the file first");
        assert_eq!(c.messages[0].ts, Some(1_785_250_017_000));
        assert_eq!(c.resolved_head(), Some(format!("{stem}:000004").as_str()));
    }

    #[test]
    fn title_is_the_first_user_message_collapsed_and_truncated() {
        let c = conv(MAIN_PATH, MAIN);
        assert_eq!(c.titles.custom, None);
        assert_eq!(c.titles.generated, None);
        assert_eq!(c.titles.resolve(), Some("summarize the changelog"));

        let long = format!(
            r#"{{"type":"session_meta","payload":{{"id":"s"}}}}
{{"type":"event_msg","payload":{{"type":"user_message","message":"{}"}}}}"#,
            "w ".repeat(100)
        );
        let c = conv("rollout-long.jsonl", &long);
        assert_eq!(c.titles.resolve().unwrap().chars().count(), TITLE_MAX_CHARS);
    }

    #[test]
    fn a_subagent_file_is_a_sidechain_keyed_by_its_own_file_name() {
        // Both spellings, and the shared `session_id` that makes the file the only thing
        // distinguishing this thread from its parent.
        let by_thread_source = r#"
{"timestamp":"2026-07-28T14:46:20.000Z","type":"session_meta","payload":{"session_id":"019f-root","id":"019f-sub","thread_source":"subagent","originator":"codex_vscode","parent_thread_id":"019f-root","source":{"subagent":{"thread_spawn":{"parent_thread_id":"019f-root"}}}}}
{"timestamp":"2026-07-28T14:46:21.000Z","type":"event_msg","payload":{"type":"user_message","message":"find the config loader"}}
{"timestamp":"2026-07-28T14:46:22.000Z","type":"event_msg","payload":{"type":"agent_message","message":"it is in config.rs"}}
"#;
        let c = conv("2026/07/28/rollout-2026-07-28T14-46-20-019f-sub.jsonl", by_thread_source);

        // The conversation is the parent's; the thread is this file.
        assert_eq!(c.native_id, "019f-root");
        assert!(c.messages.iter().all(|m| m.is_sidechain));
        assert!(c.messages.iter().all(|m| m.thread_key == "rollout-2026-07-28T14-46-20-019f-sub"));
        // With every message on a sidechain there is no main-thread leaf to fall back to.
        assert_eq!(c.resolved_head(), None);

        // `source` as an object is the only signal on older subagent rollouts.
        let by_source_object = r#"
{"type":"session_meta","payload":{"id":"019f-sub2","source":{"subagent":{"thread_spawn":{}}}}}
{"type":"event_msg","payload":{"type":"user_message","message":"go"}}
"#;
        assert!(conv("rollout-sub2.jsonl", by_source_object).messages[0].is_sidechain);

        // An explicit null `source` must not be mistaken for an object, and `thread_source`
        // of "user" is the ordinary case.
        let plain = r#"
{"type":"session_meta","payload":{"id":"019f-plain","thread_source":"user","source":null}}
{"type":"event_msg","payload":{"type":"user_message","message":"go"}}
"#;
        assert!(!conv("rollout-plain.jsonl", plain).messages[0].is_sidechain);
    }

    #[test]
    fn ordinals_from_two_files_of_one_session_do_not_collide() {
        // The exact shape that collided before ids were scoped by file: a parent and its
        // subagent, same session_id, both numbering from zero.
        let parent = r#"
{"type":"session_meta","payload":{"session_id":"019f-shared","id":"019f-shared"}}
{"type":"event_msg","payload":{"type":"user_message","message":"parent turn"}}
"#;
        let child = r#"
{"type":"session_meta","payload":{"session_id":"019f-shared","id":"019f-kid","thread_source":"subagent"}}
{"type":"event_msg","payload":{"type":"user_message","message":"child turn"}}
"#;
        let a = conv("2026/07/28/rollout-A-019f-shared.jsonl", parent);
        let b = conv("2026/07/28/rollout-B-019f-kid.jsonl", child);

        // Same conversation, so the ids share a namespace and must not overlap.
        assert_eq!(a.native_id, b.native_id);
        assert_eq!(a.messages[0].seq, b.messages[0].seq);
        assert_ne!(a.messages[0].native_id, b.messages[0].native_id);
        assert_eq!(a.messages[0].native_id, "rollout-A-019f-shared:000000");
        assert_eq!(b.messages[0].native_id, "rollout-B-019f-kid:000000");
        assert_ne!(a.messages[0].thread_key, b.messages[0].thread_key);
    }

    #[test]
    fn a_fork_records_the_conversation_it_came_from() {
        // A forked rollout replays the parent's history, so the parent's session_meta shows
        // up *after* the fork's own. First meta wins, or the fork is swallowed by its parent.
        let forked = r#"
{"timestamp":"2026-06-13T21:13:32.000Z","type":"session_meta","payload":{"id":"019ec267-fork","forked_from_id":"019ebd61-parent","originator":"Codex Desktop","cwd":"/w/fork"}}
{"timestamp":"2026-06-12T21:48:44.000Z","type":"session_meta","payload":{"id":"019ebd61-parent","originator":"codex_vscode","cwd":"/w/parent"}}
{"timestamp":"2026-06-13T21:13:40.000Z","type":"event_msg","payload":{"type":"user_message","message":"carry on from here"}}
"#;
        let c = conv("2026/06/13/rollout-2026-06-13T21-13-32-019ec267-fork.jsonl", forked);

        assert_eq!(c.native_id, "019ec267-fork");
        assert_eq!(c.forked_from_native_id.as_deref(), Some("019ebd61-parent"));
        assert_eq!(c.surface.as_deref(), Some("Codex Desktop"));
        assert_eq!(c.cwd.as_deref(), Some("/w/fork"));
    }

    #[test]
    fn a_truncated_trailing_line_does_not_fail_the_import() {
        let mut jsonl = MAIN.to_string();
        jsonl.push_str(
            r#"{"timestamp":"2026-07-28T14:47:05.000Z","type":"event_msg","payload":{"type":"agent_"#,
        );
        let c = conv(MAIN_PATH, &jsonl);
        assert_eq!(c.messages.len(), 5);

        // Garbage in the middle is survivable too: the messages around it still land, and
        // ordinals stay dense because only emitted messages consume one.
        let interrupted = r#"
{"type":"session_meta","payload":{"id":"s"}}
{"type":"event_msg","payload":{"type":"user_message","message":"one"}}
not json at all
{"type":"event_msg","payload":
{"type":"event_msg","payload":{"type":"agent_message","message":"two"}}
"#;
        let c = conv("rollout-interrupted.jsonl", interrupted);
        assert_eq!(c.messages.len(), 2);
        assert_eq!(c.messages[1].native_id, "rollout-interrupted:000001");
        assert_eq!(c.messages[1].parent_native_id.as_deref(), Some("rollout-interrupted:000000"));
    }

    #[test]
    fn nothing_worth_indexing_yields_none() {
        assert!(import("rollout-empty.jsonl", b"").is_none());
        assert!(import("rollout-garbage.jsonl", b"not json\n{{{\n\x00\xff\n").is_none());

        // A header with no messages is a real, normal file — a session opened and abandoned.
        let header_only = r#"{"type":"session_meta","payload":{"id":"019f-empty","cwd":"/w"}}"#;
        assert!(import("rollout-header.jsonl", header_only.as_bytes()).is_none());

        // Messages with no session id have nothing to hang off.
        let no_meta = r#"{"type":"event_msg","payload":{"type":"user_message","message":"hi"}}"#;
        assert!(import("rollout-orphan.jsonl", no_meta.as_bytes()).is_none());

        // Neither `session_id` nor `id`.
        let anonymous = r#"
{"type":"session_meta","payload":{"cwd":"/w","originator":"codex_vscode"}}
{"type":"event_msg","payload":{"type":"user_message","message":"hi"}}
"#;
        assert!(import("rollout-anon.jsonl", anonymous.as_bytes()).is_none());

        // Whitespace-only text is not a message.
        let blank = r#"
{"type":"session_meta","payload":{"id":"s"}}
{"type":"event_msg","payload":{"type":"user_message","message":"   \n  "}}
{"type":"response_item","payload":{"type":"function_call","name":"","arguments":""}}
"#;
        assert!(import("rollout-blank.jsonl", blank.as_bytes()).is_none());
    }

    #[test]
    fn explicit_nulls_fall_through_to_the_alternate_key() {
        // `Value::get` hands back `Some(Null)` for an explicit null, which would make a
        // naive `or_else` chain stop at the null instead of trying the next key.
        let nulls = r#"
{"type":"session_meta","payload":{"session_id":null,"id":"019f-fallback","git":null,"forked_from_id":null,"originator":null}}
{"type":"response_item","payload":{"type":"custom_tool_call","name":"apply_patch","arguments":null,"input":"*** Begin Patch"}}
{"type":"response_item","payload":{"type":"custom_tool_call_output","output":"Done"}}
"#;
        let c = conv("rollout-nulls.jsonl", nulls);

        assert_eq!(c.native_id, "019f-fallback");
        assert_eq!(c.git_branch, None);
        assert_eq!(c.surface, None);
        assert_eq!(c.forked_from_native_id, None);
        assert_eq!(c.messages[0].text, "apply_patch\n*** Begin Patch");
        assert_eq!(c.messages[0].kind, Kind::ToolCall);
        assert_eq!(c.messages[1].kind, Kind::ToolResult);
        assert_eq!(c.messages[1].role, Role::Tool);
    }

    #[test]
    fn structured_payloads_flatten_into_searchable_text() {
        let nested = r#"
{"type":"session_meta","payload":{"id":"s"}}
{"type":"response_item","payload":{"type":"reasoning","summary":[{"type":"summary_text","text":"first"},{"type":"summary_text","text":"second"}]}}
{"type":"response_item","payload":{"type":"function_call_output","output":[{"type":"output_text","text":"line a"},{"type":"output_text","text":"line b"}]}}
{"type":"event_msg","payload":{"type":"user_message","message":"plain string"}}
"#;
        let c = conv("rollout-nested.jsonl", nested);
        assert_eq!(c.messages[0].text, "first\nsecond");
        assert_eq!(c.messages[1].text, "line a\nline b");
        assert_eq!(c.messages[2].text, "plain string");
    }

    #[test]
    fn the_last_turn_context_names_the_model() {
        let switched = r#"
{"type":"session_meta","payload":{"id":"s","model_provider":"openai"}}
{"type":"turn_context","payload":{"model":"gpt-5-codex"}}
{"type":"event_msg","payload":{"type":"user_message","message":"hi"}}
{"type":"turn_context","payload":{"model":"gpt-5.4"}}
{"type":"event_msg","payload":{"type":"agent_message","message":"hello"}}
"#;
        // Never the provider, which is what `session_meta.model_provider` holds.
        assert_eq!(conv("rollout-model.jsonl", switched).model.as_deref(), Some("gpt-5.4"));

        let none = r#"
{"type":"session_meta","payload":{"id":"s","model_provider":"openai"}}
{"type":"event_msg","payload":{"type":"user_message","message":"hi"}}
"#;
        assert_eq!(conv("rollout-nomodel.jsonl", none).model, None);
    }

    #[test]
    fn thread_key_is_the_file_stem_whatever_the_path_shape() {
        assert_eq!(thread_key("2026/07/28/rollout-x.jsonl"), "rollout-x");
        assert_eq!(thread_key("rollout-x.jsonl"), "rollout-x");
        assert_eq!(thread_key("a/b/rollout-x"), "rollout-x");
        assert_eq!(thread_key(""), "");
        // Only the final `.jsonl` is a suffix; dots inside the stem stay.
        assert_eq!(
            thread_key("rollout-2026-07-28T14-46-55-019f.a.b.jsonl"),
            "rollout-2026-07-28T14-46-55-019f.a.b"
        );
    }

    #[test]
    fn timestamps_become_epoch_millis() {
        assert_eq!(epoch_millis("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(epoch_millis("2026-07-28T14:46:55.123Z"), Some(1_785_250_015_123));
        assert_eq!(epoch_millis("2025-11-04T17:01:35.000Z"), Some(1_762_275_695_000));
        // Leap day, and a century that is not a leap year: both fall out of the civil-days
        // formula rather than a naive /4.
        assert_eq!(epoch_millis("2024-02-29T00:00:00Z"), Some(1_709_164_800_000));
        assert_eq!(epoch_millis("1900-03-01T00:00:00Z"), Some(-2_203_891_200_000));

        // Sub-millisecond digits are read and discarded, not left to look like an offset.
        assert_eq!(epoch_millis("2026-07-28T14:46:55.123456789Z"), Some(1_785_250_015_123));
        assert_eq!(epoch_millis("2026-07-28T14:46:55.1Z"), Some(1_785_250_015_100));
        assert_eq!(epoch_millis("2026-07-28T14:46:55.12Z"), Some(1_785_250_015_120));

        // Offsets, both separator styles, and a missing zone read as UTC.
        assert_eq!(epoch_millis("2026-07-28T16:46:55+02:00"), Some(1_785_250_015_000));
        assert_eq!(epoch_millis("2026-07-28T16:46:55+0200"), Some(1_785_250_015_000));
        assert_eq!(epoch_millis("2026-07-28T09:46:55-05:00"), Some(1_785_250_015_000));
        assert_eq!(epoch_millis("2026-07-28T14:46:55"), Some(1_785_250_015_000));

        for bad in [
            "", "nope", "2026-07-28", "2026-07-28X14:46:55Z", "20260728T144655Z",
            "2026-13-01T00:00:00Z", "2026-00-01T00:00:00Z", "2026-07-32T00:00:00Z",
            "2026-07-28T25:00:00Z", "2026-07-28T14:46:55.Z", "2026-07-28T14:46:55~",
        ] {
            assert_eq!(epoch_millis(bad), None, "{bad} should not parse");
        }
    }

    #[test]
    fn a_message_without_a_timestamp_still_imports() {
        let untimed = r#"
{"type":"session_meta","payload":{"id":"s"}}
{"type":"event_msg","payload":{"type":"user_message","message":"no clock here"}}
{"timestamp":"garbage","type":"event_msg","payload":{"type":"agent_message","message":"nor here"}}
"#;
        let c = conv("rollout-untimed.jsonl", untimed);
        assert_eq!(c.messages.len(), 2);
        assert!(c.messages.iter().all(|m| m.ts.is_none()));
    }

    /// Sanity check against a real session directory. Ignored by default and driven by an
    /// env var, so no personal data reaches the repo or CI, and it prints counts only:
    ///
    /// ```sh
    /// CS_CODEX_SESSIONS=~/.codex/sessions cargo test -p cs-import -- --ignored --nocapture
    /// ```
    #[test]
    #[ignore = "needs real rollouts; set CS_CODEX_SESSIONS to a ~/.codex/sessions directory"]
    fn real_corpus_stats() {
        use std::collections::{HashMap, HashSet};

        let Ok(dir) = std::env::var("CS_CODEX_SESSIONS") else { return };
        fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            let Ok(entries) = std::fs::read_dir(dir) else { return };
            for e in entries.flatten() {
                let p = e.path();
                if p.is_dir() {
                    walk(&p, out);
                } else if p.extension().is_some_and(|x| x == "jsonl") {
                    out.push(p);
                }
            }
        }
        let mut files = Vec::new();
        walk(std::path::Path::new(&dir), &mut files);
        files.sort();

        let root = std::path::Path::new(&dir);
        let (mut imported, mut skipped, mut msgs, mut sidechains, mut forks) = (0, 0, 0, 0, 0);
        let mut kinds: HashMap<&str, usize> = HashMap::new();
        // The collision this scoping exists to prevent: two threads of one conversation
        // both numbering from zero.
        let mut ids_by_conversation: HashMap<String, HashSet<String>> = HashMap::new();
        let mut collisions = 0usize;

        for f in &files {
            let logical = f.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/");
            let Some(c) = import(&logical, &std::fs::read(f).unwrap()) else {
                skipped += 1;
                continue;
            };
            imported += 1;
            msgs += c.messages.len();
            forks += usize::from(c.forked_from_native_id.is_some());
            sidechains += usize::from(c.messages.iter().any(|m| m.is_sidechain));
            let seen = ids_by_conversation.entry(c.id()).or_default();
            for (i, m) in c.messages.iter().enumerate() {
                *kinds.entry(m.kind.as_str()).or_default() += 1;
                assert_eq!(m.seq, i as i64, "seq must be dense and ordered in {logical}");
                assert!(m.native_id.starts_with(&m.thread_key), "id must be thread-scoped");
                if !seen.insert(m.native_id.clone()) {
                    collisions += 1;
                }
            }
        }

        let mut kinds: Vec<_> = kinds.into_iter().collect();
        kinds.sort_unstable();
        println!(
            "files={} imported={imported} skipped={skipped} conversations={} messages={msgs} \
             sidechain_files={sidechains} forks={forks} kinds={kinds:?}",
            files.len(),
            ids_by_conversation.len()
        );
        assert!(imported > 0 && msgs > 0);
        assert_eq!(collisions, 0, "message ids collided inside a conversation");
    }

    #[test]
    fn importing_the_same_bytes_twice_gives_the_same_ids() {
        // Ids must be a pure function of (path, bytes) or re-import churns the index.
        let a = conv(MAIN_PATH, MAIN);
        let b = conv(MAIN_PATH, MAIN);
        let ids = |c: &Conversation| {
            c.messages.iter().map(|m| m.native_id.clone()).collect::<Vec<_>>()
        };
        assert_eq!(ids(&a), ids(&b));
        assert_eq!(a.id(), b.id());
    }

    #[test]
    fn a_synthetic_environment_context_turn_is_not_prose_and_never_becomes_the_title() {
        let raw = r#"{"timestamp":"2026-07-28T14:46:20.000Z","type":"session_meta","payload":{"session_id":"s1","id":"s1"}}
{"timestamp":"2026-07-28T14:46:21.000Z","type":"event_msg","payload":{"type":"user_message","kind":"environment_context","message":"<environment_context><cwd>/Users/x/dev</cwd></environment_context>"}}
{"timestamp":"2026-07-28T14:46:22.000Z","type":"event_msg","payload":{"type":"user_message","message":"why is the loader slow"}}"#;
        let c = import("rollout-2026-07-28T14-46-20-s1.jsonl", raw.as_bytes()).unwrap();
        assert_eq!(c.messages.len(), 1, "the injected turn must not be imported");
        assert_eq!(c.messages[0].text, "why is the loader slow");
        assert_eq!(c.titles.first_user.as_deref(), Some("why is the loader slow"));
    }

}
