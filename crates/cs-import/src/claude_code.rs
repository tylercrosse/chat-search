//! Claude Code transcripts — one JSONL file per thread, under `~/.claude/projects/`.
//!
//! The file is an append-only event log, not a document: the conversation, its title and
//! its shape are all *folds* over that log. Three consequences drive everything below.
//!
//! 1. **The file is the thread, the `sessionId` is the conversation.** A subagent runs in
//!    `<sessionId>/subagents/agent-*.jsonl` and carries the *parent's* `sessionId`, so the
//!    session id cannot identify a thread. `thread_key` is the logical path (ADR 4).
//! 2. **`parentUuid` is a real DAG edge**, already present in the source. It is used
//!    directly rather than synthesising a linear chain, because a re-run or an edit can put
//!    two children under one parent and a chain would silently pick one (ADR 4). The one
//!    adjustment is contraction: most events in the chain never become messages, and an
//!    edge to a node we did not materialise is not an edge (see `nearest_emitted`).
//! 3. **Titles are a fold** — `custom-title` (a rename, re-emitted on every save, so last
//!    wins) > `ai-title` > first user message (ADR 8).

use cs_core::{model_name, Conversation, Kind, Message, Role, Titles};
use serde_json::Value;
use std::collections::{HashMap, HashSet};

/// The watched location this importer parses. Permanent — it is part of every id (ADR 16).
const SOURCE: &str = "claude-code";

/// Titles are a preview, not content. Long enough to disambiguate, short enough to list.
const TITLE_MAX_CHARS: usize = 80;

/// Parse one archived transcript.
///
/// `logical_path` is the path the file had in the *source* tree, supplied by the archive
/// reader so physical layout never reaches here (ADR 18). It becomes `thread_key`.
///
/// Returns `None` when the file names no session or yields no messages — an empty or
/// header-only transcript is not a conversation.
pub fn import(logical_path: &str, bytes: &[u8]) -> Option<Conversation> {
    // Conversation-level metadata. Each is first-non-null: `cwd` changes mid-session in 26%
    // of sessions, so "the" cwd is a choice, and the first one is the one the conversation
    // started in. Merging across the several files of a conversation is the indexer's job
    // (COALESCE, ADR 7), not ours.
    //
    // `model` is deliberately not in this list. It changes mid-session too — 23 of 428
    // sessions here — but unlike `cwd` the source states it per message, so it is carried
    // per message and the single label is resolved once by the indexer (ADR 24).
    let mut session_id: Option<String> = None;
    let mut cwd: Option<String> = None;
    let mut git_branch: Option<String> = None;
    let mut surface: Option<String> = None;

    let mut custom_title: Option<String> = None;
    let mut generated_title: Option<String> = None;
    let mut first_user: Option<String> = None;

    let mut messages: Vec<Message> = Vec::new();
    let mut seq: i64 = 0;
    let mut event_ordinal: usize = 0;

    // Most events in a transcript are not messages, yet they all sit in the `parentUuid`
    // chain: `attachment`, `system`, `isMeta` injections, and — on this corpus — every
    // `thinking` turn, whose text is stored empty. Measured over 307 real transcripts, 46%
    // of edges point at one of those. Emitting those ids anyway would leave the DAG full of
    // references to messages that do not exist, so a dropped event's id is remembered
    // together with its own parent and the edge is contracted onto the nearest ancestor
    // that *did* become a message. The edge still comes from the source; only nodes we
    // chose not to materialise are elided.
    let mut emitted_ids: HashSet<String> = HashSet::new();
    let mut elided: HashMap<String, Option<String>> = HashMap::new();

    for line in lines(bytes) {
        // A truncated final line is expected on an in-flight session, and a stray
        // non-object line has been seen too. Neither is an error: skip and keep going.
        let Ok(event) = serde_json::from_str::<Value>(line) else { continue };
        if !event.is_object() {
            continue;
        }
        event_ordinal += 1;

        // Read from every event type, including the title events: those carry `sessionId`
        // but no `uuid`, and in a transcript whose message events were truncated away they
        // may be the only thing that names the session.
        if session_id.is_none() {
            session_id = text(&event, "sessionId").or_else(|| text(&event, "session_id"));
        }
        if cwd.is_none() {
            cwd = text(&event, "cwd");
        }
        if git_branch.is_none() {
            git_branch = text(&event, "gitBranch");
        }
        if surface.is_none() {
            // `entrypoint` is cli | claude-vscode | sdk-py. Derived only — it is a thread
            // attribute and files within one conversation disagree about it (ADR 16).
            surface = text(&event, "entrypoint");
        }

        let event_type = event.get("type").and_then(Value::as_str).unwrap_or("");
        match event_type {
            // Last wins: a rename is re-emitted on every save, up to 44x in one session.
            "custom-title" => {
                if let Some(t) = text(&event, "customTitle") {
                    custom_title = Some(t);
                }
                continue;
            }
            "ai-title" => {
                if let Some(t) = text(&event, "aiTitle") {
                    generated_title = Some(t);
                }
                continue;
            }
            // `user`, `assistant`, and the rest — `system`, `summary`, `attachment`, … —
            // carry no title but are all still nodes in the parent chain.
            _ => {}
        }

        let uuid = text(&event, "uuid");
        let parent_uuid = text(&event, "parentUuid");
        let mut emitted = 0usize;

        // Everything that can decline to produce a message lives in here, so that the one
        // exit below sees whether this event became part of the DAG or has to be elided.
        'event: {
            if !matches!(event_type, "user" | "assistant") {
                break 'event;
            }
            // Synthetic system-injected turns. They are phrased as user prose and would
            // otherwise become titles.
            if event.get("isMeta").and_then(Value::as_bool) == Some(true) {
                break 'event;
            }
            let Some(message) = event.get("message").filter(|m| m.is_object()) else {
                break 'event;
            };

            // What produced *this* event, not the conversation. `<synthetic>` is rejected by
            // `model_name`, which is where every source's placeholders now live.
            let event_model = if event_type == "assistant" {
                text(message, "model")
                    .as_deref()
                    .and_then(model_name)
                    .map(str::to_string)
            } else {
                None
            };

            let ts = event.get("timestamp").and_then(Value::as_str).and_then(epoch_millis);
            let is_sidechain = event.get("isSidechain").and_then(Value::as_bool) == Some(true);
            // A missing `uuid` has not been observed, but an id must exist and must be
            // unique within the file, so fall back to the event's position rather than
            // dropping data.
            let id_base = uuid
                .clone()
                .unwrap_or_else(|| format!("evt-{event_ordinal:06}"));

            // `content` is either a bare string or an array of typed blocks.
            let wrapped;
            let blocks: &[Value] = match message.get("content") {
                Some(Value::Array(a)) => a.as_slice(),
                Some(Value::Null) | None => break 'event,
                Some(scalar) => {
                    wrapped = [serde_json::json!({"type": "text", "text": scalar})];
                    &wrapped
                }
            };

            // One event can yield several messages. They are suffixed `uuid`, `uuid#1`, …
            // and chained to each other, so the DAG keeps the order the blocks were emitted
            // in; only the first block hangs off the event's own `parentUuid`.
            let mut parent = nearest_emitted(parent_uuid.clone(), &emitted_ids, &elided);

            for block in blocks {
                let block_type = block.get("type").and_then(Value::as_str).unwrap_or("");
                // Authoritative, and present only on results the harness judged failed:
                // sampled over a real transcript, 7 of 18 `tool_result` blocks carried the
                // key at all and one was true. Absent means "not reported", which is not the
                // same as "succeeded" — but false is the only safe reading of silence.
                let is_error = block.get("is_error").and_then(Value::as_bool).unwrap_or(false);
                let (role, kind, text_body) = match block_type {
                    "text" => (role_of(event_type), Kind::Prose, flatten(field(block, "text"))),
                    // Reasoning is stored with its text blanked on this corpus — every
                    // observed block is `{"thinking": "", "signature": "…"}` — so these
                    // almost always fall out at the empty-text check below.
                    "thinking" => {
                        (Role::Assistant, Kind::Reasoning, flatten(field(block, "thinking")))
                    }
                    "tool_use" => {
                        let input = field(block, "input");
                        let args =
                            if input.is_null() { String::new() } else { input.to_string() };
                        let name = block.get("name").and_then(Value::as_str).unwrap_or("");
                        (Role::Assistant, Kind::ToolCall, format!("{name}\n{args}"))
                    }
                    // A tool result is authored by neither side, which is what `Role::Tool`
                    // is for; the `user` event it rides on is a transport detail.
                    "tool_result" => {
                        (Role::Tool, Kind::ToolResult, flatten(field(block, "content")))
                    }
                    // `image`, `document`, and whatever ships next: no searchable text.
                    _ => continue,
                };

                let text_body = text_body.trim().to_string();
                if text_body.is_empty() {
                    continue;
                }

                if first_user.is_none() && kind == Kind::Prose && role == Role::User {
                    // May yield nothing if the turn was pure slash-command markup, in which
                    // case the next user message gets its turn.
                    first_user = title_candidate(&text_body);
                }

                let native_id =
                    if emitted == 0 { id_base.clone() } else { format!("{id_base}#{emitted}") };
                // `replace` hands back the previous parent and installs this block as the
                // next one, which is the whole intra-event chain in one line.
                let parent_native_id = parent.replace(native_id.clone());
                emitted_ids.insert(native_id.clone());
                messages.push(Message {
                    native_id,
                    parent_native_id,
                    thread_key: logical_path.to_string(),
                    is_sidechain,
                    is_error: is_error && kind == Kind::ToolResult,
                    seq,
                    role,
                    kind,
                    ts,
                    // Only what the model authored. A `tool_result` block rides on a `user`
                    // event and is authored by neither side, so it stays null.
                    model: event_model.clone().filter(|_| role == Role::Assistant),
                    text: text_body,
                });
                seq += 1;
                emitted += 1;
            }
        }

        // Nothing was materialised, so this event's children must inherit its parent.
        if emitted == 0 {
            if let Some(u) = uuid {
                elided.insert(u, parent_uuid);
            }
        }
    }

    let session_id = session_id?;
    if messages.is_empty() {
        return None;
    }

    Some(Conversation {
        source: SOURCE.to_string(),
        titles: Titles {
            custom: custom_title.as_deref().map(clamp_title),
            generated: generated_title.as_deref().map(clamp_title),
            first_user,
        },
        cwd,
        git_branch,
        // Claude Code never names a model anywhere but on an assistant turn, so there is
        // nothing to fall back to and nothing is lost by saying so.
        declared_model: None,
        surface,
        // Claude Code has no fork mechanism, and no head marker: the transcript is
        // append-only, so the last message of each thread is its leaf (ADR 4).
        forked_from_native_id: None,
        head_native_id: None,
        native_id: session_id,
        messages,
    })
}

/// Walk up through elided events to the nearest ancestor that became a message.
///
/// Parents always precede their children in an append-only log, so one forward pass is
/// enough: by the time an edge is resolved, every ancestor has already been classified.
/// `None` means the chain ran off the top of this file — a thread root.
fn nearest_emitted(
    start: Option<String>,
    emitted: &HashSet<String>,
    elided: &HashMap<String, Option<String>>,
) -> Option<String> {
    let mut cursor = start;
    // A well-formed transcript cannot loop, but a corrupt one must not hang the importer.
    for _ in 0..10_000 {
        let id = cursor?;
        if emitted.contains(&id) {
            return Some(id);
        }
        cursor = elided.get(&id)?.clone();
    }
    None
}

fn role_of(event_type: &str) -> Role {
    match event_type {
        "user" => Role::User,
        _ => Role::Assistant,
    }
}

/// Iterate JSONL lines, skipping blank ones and any that are not valid UTF-8.
pub(crate) fn lines(buf: &[u8]) -> impl Iterator<Item = &str> {
    buf.split(|&b| b == b'\n')
        .map(|l| l.strip_suffix(b"\r").unwrap_or(l))
        .filter(|l| !l.is_empty())
        .filter_map(|l| std::str::from_utf8(l).ok())
}

/// A field's value, with an explicit `null` reported as absent.
///
/// `Value::get` returns `Some(Value::Null)` for an explicit null, so the obvious
/// `get("a").or_else(|| get("b"))` is *not* JavaScript's `??` — it stops at the null and
/// never tries the fallback.
pub(crate) fn field<'a>(v: &'a Value, key: &str) -> &'a Value {
    v.get(key).unwrap_or(&Value::Null)
}

/// A field as a non-empty string. Missing, null, empty and non-string all read as absent,
/// so `"gitBranch": ""` outside a repo does not become a branch name.
pub(crate) fn text(v: &Value, key: &str) -> Option<String> {
    match v.get(key) {
        Some(Value::String(s)) if !s.is_empty() => Some(s.clone()),
        _ => None,
    }
}

/// Collapse arbitrary content (string | block | block[]) into searchable text.
pub(crate) fn flatten(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        Value::Array(a) => {
            a.iter().map(flatten).filter(|s| !s.is_empty()).collect::<Vec<_>>().join("\n")
        }
        Value::Object(o) => {
            for k in ["text", "content", "output", "message", "thinking"] {
                match o.get(k) {
                    // An explicit null must not shadow a later key that has content.
                    Some(Value::Null) | None => continue,
                    Some(inner) => return flatten(inner),
                }
            }
            String::new()
        }
        other => other.to_string(),
    }
}

// ------------------------------------------------------------------- titles

/// Whitespace-collapsed and clamped. Titles sit in a list; newlines and runs of spaces in
/// one make every row below it move.
pub(crate) fn clamp_title(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(TITLE_MAX_CHARS).collect()
}

/// Tags Claude Code injects into user turns whose *contents* are machinery, not prose.
/// Dropped whole, tag and body, so a turn that is nothing but a slash-command expansion
/// leaves no title behind and the next user message gets the job.
///
/// The `ide_*` three come from the editor extension rather than the slash-command
/// machinery, but they arrive the same way: their own user text block, closed, with nothing
/// after the closing tag in any of the 353 `ide_opened_file` and 66 `ide_selection` blocks
/// on the reference corpus. Left in, they titled 16 conversations with a sentence about
/// which file happened to be focused. `ide_diagnostics` has only been seen on `attachment`
/// events, which never become messages, but it is the same injection and costs a line.
const DROPPED_TAGS: [&str; 11] = [
    "command-message",
    "command-name",
    "command-contents",
    "local-command-stdout",
    "local-command-stderr",
    "local-command-caveat",
    "system-reminder",
    "system_reminder",
    "ide_opened_file",
    "ide_selection",
    "ide_diagnostics",
];

/// `<command-args>` is the one exception: its body is literally what the user typed after
/// the slash command (non-empty in 112 of 155 observed cases), so the body is kept and only
/// the tag is removed. `/research how do X` should title as `how do X`, not as nothing.
const UNWRAPPED_TAGS: [&str; 1] = ["command-args"];

/// …except after a built-in that takes a *setting* rather than a prompt. `/model opus` and
/// `/fast off` argue with the harness, not about the work, and the setting is the only
/// thing that survives stripping: all 9 conversations that titled from `<command-args>` on
/// the reference corpus came out as a bare model name — `opus`, `fable`,
/// `claude-fable-5[1m]`. These two also account for every one of the 110 non-empty
/// `<command-args>` bodies there, so the exception above survives only for the custom
/// commands it was written for, where the args really are a prompt.
const SETTING_COMMANDS: [&str; 2] = ["/model", "/fast"];

/// Bracketed notices the harness writes into the transcript as though the user had typed
/// them. They are the entire turn when they appear, so a session the user interrupted
/// before saying anything titles itself with the interruption — 3 conversations on the
/// reference corpus. Literal strings rather than a pattern: these two spellings are the
/// only ones in 119 occurrences.
const HARNESS_NOTICES: [&str; 2] =
    ["[Request interrupted by user for tool use]", "[Request interrupted by user]"];

/// A user message reduced to a title, or `None` if it was all markup.
pub(crate) fn title_candidate(raw: &str) -> Option<String> {
    // Whether `<command-args>` is a prompt or a setting is decided by the sibling
    // `<command-name>`, which is why this is read from the whole turn rather than left to
    // the tag-at-a-time scan below.
    let args_are_prose = !SETTING_COMMANDS
        .iter()
        .any(|name| raw.contains(&format!("<command-name>{name}</command-name>")));

    let mut stripped = strip_injected_markup(raw, args_are_prose);
    for notice in HARNESS_NOTICES {
        stripped = stripped.replace(notice, " ");
    }

    let title = clamp_title(&stripped);
    if title.is_empty() {
        None
    } else {
        Some(title)
    }
}

/// Remove Claude Code's injected pseudo-XML. Deliberately literal — these are fixed tags
/// the tool emits, not a general XML parser, and unbalanced markup must degrade rather than
/// swallow the rest of the message.
///
/// `args_are_prose` false drops `<command-args>` like any other machinery tag; see
/// `SETTING_COMMANDS`.
fn strip_injected_markup(s: &str, args_are_prose: bool) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;

    'scan: while let Some(lt) = rest.find('<') {
        out.push_str(&rest[..lt]);
        let at_tag = &rest[lt..];

        for tag in DROPPED_TAGS.iter().chain(UNWRAPPED_TAGS.iter()) {
            let Some(body) = at_tag.strip_prefix(format!("<{tag}>").as_str()) else { continue };
            match body.find(format!("</{tag}>").as_str()) {
                Some(end) => {
                    if args_are_prose && UNWRAPPED_TAGS.contains(tag) {
                        out.push(' ');
                        out.push_str(&body[..end]);
                        out.push(' ');
                    }
                    rest = &body[end + tag.len() + 3..];
                }
                // Unterminated: a truncated tail, or prose that merely looks like a tag.
                // Drop the marker only and keep reading.
                None => rest = body,
            }
            continue 'scan;
        }

        out.push('<');
        rest = &at_tag[1..];
    }

    out.push_str(rest);
    out
}

// --------------------------------------------------------------- timestamps

/// RFC 3339 to epoch milliseconds, without a date-time dependency.
///
/// The corpus is uniformly `YYYY-MM-DDTHH:MM:SS.mmmZ`, but offsets and other fraction
/// widths are legal and cost a few lines to accept. Unparseable input yields `None`: a
/// message with no timestamp is still a message.
pub(crate) fn epoch_millis(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() < 19 || b[4] != b'-' || b[7] != b'-' || b[13] != b':' || b[16] != b':' {
        return None;
    }
    if !matches!(b[10], b'T' | b't' | b' ') {
        return None;
    }
    let year: i64 = digits(s.get(0..4)?)?;
    let month: i64 = digits(s.get(5..7)?)?;
    let day: i64 = digits(s.get(8..10)?)?;
    let hour: i64 = digits(s.get(11..13)?)?;
    let minute: i64 = digits(s.get(14..16)?)?;
    let second: i64 = digits(s.get(17..19)?)?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    // 60 is a leap second; accept it rather than dropping the timestamp.
    if hour > 23 || minute > 59 || second > 60 {
        return None;
    }

    let mut rest = &s[19..];
    let mut millis = 0i64;
    if let Some(frac) = rest.strip_prefix('.').or_else(|| rest.strip_prefix(',')) {
        let frac_digits: String = frac.chars().take_while(char::is_ascii_digit).collect();
        if frac_digits.is_empty() {
            return None;
        }
        // Widen or narrow whatever precision was written to exactly milliseconds.
        let mut ms = frac_digits.chars().take(3).collect::<String>();
        while ms.len() < 3 {
            ms.push('0');
        }
        millis = digits(&ms)?;
        rest = &frac[frac_digits.len()..];
    }

    let offset_secs = match rest.as_bytes().first() {
        // A naive timestamp is read as UTC — every observed one is `Z`.
        None => 0,
        Some(b'Z' | b'z') if rest.len() == 1 => 0,
        Some(sign @ (b'+' | b'-')) => {
            let sign = if *sign == b'-' { -1 } else { 1 };
            let t = rest.get(1..)?;
            let (oh, om) = match t.as_bytes() {
                [_, _, b':', _, _] => (digits(&t[0..2])?, digits(&t[3..5])?),
                [_, _, _, _] => (digits(&t[0..2])?, digits(&t[2..4])?),
                [_, _] => (digits(&t[0..2])?, 0),
                _ => return None,
            };
            if oh > 23 || om > 59 {
                return None;
            }
            sign * (oh * 3600 + om * 60)
        }
        _ => return None,
    };

    let days = days_from_civil(year, month, day);
    Some((days * 86_400 + hour * 3_600 + minute * 60 + second - offset_secs) * 1_000 + millis)
}

/// Parse an all-ASCII-digit run. Rejects `+7`, ` 7` and `7e2`, all of which `i64::from_str`
/// would otherwise take or mangle.
fn digits(s: &str) -> Option<i64> {
    if s.is_empty() || !s.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    s.parse().ok()
}

/// Days since 1970-01-01 from a proleptic Gregorian date (Howard Hinnant's `days_from_civil`).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every fixture here is written by hand. Real transcripts contain secrets and must
    /// never become test data.
    const MAIN: &str = "project/session-1.jsonl";

    /// One event per line. Fixtures are written across several source lines for
    /// readability, so newlines *inside* an event are folded to spaces first — JSONL means
    /// one object per line, and the importer splits on `\n`.
    fn jsonl(events: &[&str]) -> Vec<u8> {
        events.iter().map(|e| e.replace('\n', " ")).collect::<Vec<_>>().join("\n").into_bytes()
    }

    fn import_main(events: &[&str]) -> Conversation {
        import(MAIN, &jsonl(events)).expect("fixture should import")
    }

    /// A minimal user event; `$` placeholders keep the fixtures readable.
    fn user(uuid: &str, parent: &str, text: &str) -> String {
        let parent = if parent.is_empty() {
            "null".to_string()
        } else {
            format!("\"{parent}\"")
        };
        format!(
            r#"{{"type":"user","sessionId":"s-1","uuid":"{uuid}","parentUuid":{parent},
                 "timestamp":"2026-07-28T19:09:51.319Z",
                 "message":{{"role":"user","content":[{{"type":"text","text":"{text}"}}]}}}}"#
        )
    }

    #[test]
    fn blocks_map_to_the_right_kind_and_role() {
        let c = import_main(&[
            &user("u1", "", "hello there"),
            r#"{"type":"assistant","sessionId":"s-1","uuid":"a1","parentUuid":"u1",
                "message":{"model":"claude-opus-4","content":[
                  {"type":"thinking","thinking":"weighing options"},
                  {"type":"text","text":"here is the answer"},
                  {"type":"tool_use","name":"Read","input":{"file_path":"/x.rs"}}]}}"#,
            r#"{"type":"user","sessionId":"s-1","uuid":"u2","parentUuid":"a1",
                "message":{"content":[{"type":"tool_result","content":[
                  {"type":"text","text":"fn main() {}"}]}]}}"#,
        ]);

        let got: Vec<_> =
            c.messages.iter().map(|m| (m.kind, m.role, m.text.as_str())).collect();
        assert_eq!(
            got,
            vec![
                (Kind::Prose, Role::User, "hello there"),
                (Kind::Reasoning, Role::Assistant, "weighing options"),
                (Kind::Prose, Role::Assistant, "here is the answer"),
                (Kind::ToolCall, Role::Assistant, "Read\n{\"file_path\":\"/x.rs\"}"),
                (Kind::ToolResult, Role::Tool, "fn main() {}"),
            ]
        );
        // The model lands on the three blocks the assistant event produced, and on nothing
        // else: the user turn was typed and the tool result came back from a tool.
        let opus = Some("claude-opus-4");
        assert_eq!(
            c.messages.iter().map(|m| m.model.as_deref()).collect::<Vec<_>>(),
            vec![None, opus, opus, opus, None]
        );
        assert_eq!(c.messages.iter().map(|m| m.seq).collect::<Vec<_>>(), vec![0, 1, 2, 3, 4]);
    }

    #[test]
    fn parent_uuid_is_carried_through_as_the_dag_edge() {
        // Two children under one parent: an edit-branch. A synthesised linear chain would
        // lose one of them, which is the whole reason the edge comes from the source.
        let c = import_main(&[
            &user("u1", "", "first"),
            &user("u2a", "u1", "second"),
            &user("u2b", "u1", "second, rewritten"),
        ]);

        let edges: Vec<_> = c
            .messages
            .iter()
            .map(|m| (m.native_id.as_str(), m.parent_native_id.as_deref()))
            .collect();
        assert_eq!(
            edges,
            vec![("u1", None), ("u2a", Some("u1")), ("u2b", Some("u1"))]
        );
        assert!(c.messages.iter().all(|m| m.thread_key == MAIN));
        assert_eq!(c.head_native_id, None);
    }

    #[test]
    fn multi_block_events_get_suffixed_ids_chained_within_the_event() {
        let c = import_main(&[
            &user("u1", "", "go"),
            r#"{"type":"assistant","sessionId":"s-1","uuid":"a1","parentUuid":"u1",
                "message":{"content":[
                  {"type":"text","text":"one"},
                  {"type":"text","text":"two"},
                  {"type":"text","text":"three"}]}}"#,
        ]);

        let edges: Vec<_> = c.messages[1..]
            .iter()
            .map(|m| (m.native_id.as_str(), m.parent_native_id.as_deref()))
            .collect();
        assert_eq!(
            edges,
            vec![("a1", Some("u1")), ("a1#1", Some("a1")), ("a1#2", Some("a1#1"))]
        );
    }

    #[test]
    fn a_skipped_block_does_not_leave_a_gap_in_the_suffixes() {
        // An empty block and an image block yield nothing; ids stay dense so the first
        // *emitted* block always owns the bare uuid.
        let c = import_main(&[
            r#"{"type":"assistant","sessionId":"s-1","uuid":"a1","message":{"content":[
                 {"type":"text","text":"   "},
                 {"type":"image","source":{"data":"..."}},
                 {"type":"text","text":"real"},
                 {"type":"text","text":"also real"}]}}"#,
        ]);
        let ids: Vec<_> = c.messages.iter().map(|m| m.native_id.as_str()).collect();
        assert_eq!(ids, vec!["a1", "a1#1"]);
        assert_eq!(c.messages[0].text, "real");
    }

    #[test]
    fn meta_events_are_excluded_from_messages_and_titles() {
        let c = import_main(&[
            r#"{"type":"user","sessionId":"s-1","uuid":"m1","isMeta":true,
                "message":{"content":[{"type":"text","text":"Caveat: this is injected"}]}}"#,
            &user("u1", "m1", "the real question"),
        ]);

        assert_eq!(c.messages.len(), 1);
        assert_eq!(c.messages[0].native_id, "u1");
        assert_eq!(c.titles.first_user.as_deref(), Some("the real question"));
        // The meta event was the only ancestor, so contracting past it leaves a root
        // rather than an edge to a message that does not exist.
        assert_eq!(c.messages[0].parent_native_id, None);
    }

    #[test]
    fn edges_are_contracted_across_events_that_yield_no_message() {
        // The real chain on this corpus: user -> attachment -> thinking (text blanked)
        // -> assistant text. Only the two ends become messages.
        let c = import_main(&[
            &user("u1", "", "question"),
            r#"{"type":"attachment","sessionId":"s-1","uuid":"at1","parentUuid":"u1",
                "attachment":{"kind":"file"}}"#,
            r#"{"type":"assistant","sessionId":"s-1","uuid":"th1","parentUuid":"at1",
                "message":{"content":[{"type":"thinking","thinking":"","signature":"sig"}]}}"#,
            r#"{"type":"system","sessionId":"s-1","uuid":"sy1","parentUuid":"th1",
                "subtype":"hook"}"#,
            r#"{"type":"assistant","sessionId":"s-1","uuid":"a1","parentUuid":"sy1",
                "message":{"content":[{"type":"text","text":"answer"}]}}"#,
        ]);

        let edges: Vec<_> = c
            .messages
            .iter()
            .map(|m| (m.native_id.as_str(), m.parent_native_id.as_deref()))
            .collect();
        assert_eq!(edges, vec![("u1", None), ("a1", Some("u1"))]);
    }

    #[test]
    fn every_parent_resolves_to_a_message_in_the_conversation() {
        let c = import_main(&[
            &user("u1", "", "one"),
            r#"{"type":"assistant","sessionId":"s-1","uuid":"th1","parentUuid":"u1",
                "message":{"content":[{"type":"thinking","thinking":"","signature":"s"}]}}"#,
            r#"{"type":"assistant","sessionId":"s-1","uuid":"a1","parentUuid":"th1",
                "message":{"content":[{"type":"text","text":"two"},
                                      {"type":"tool_use","name":"Read","input":{"p":"/x"}}]}}"#,
            r#"{"type":"user","sessionId":"s-1","uuid":"u2","parentUuid":"a1",
                "message":{"content":[{"type":"tool_result","content":"ok"}]}}"#,
        ]);

        let ids: std::collections::HashSet<&str> =
            c.messages.iter().map(|m| m.native_id.as_str()).collect();
        for m in &c.messages {
            if let Some(p) = &m.parent_native_id {
                assert!(ids.contains(p.as_str()), "dangling parent {p} on {}", m.native_id);
            }
        }
        // A child names the *event*, and the event's bare uuid is its first block, so the
        // tool_result attaches to `a1` rather than to the `a1#1` tool_use that produced it.
        // The suffixed ids are an intra-event detail no other event can address.
        assert_eq!(c.messages.last().unwrap().parent_native_id.as_deref(), Some("a1"));
    }

    #[test]
    fn a_blank_thinking_block_produces_nothing() {
        // Claude Code persists the signature but blanks the reasoning text, so there is
        // nothing to index. Every one of 9,123 blocks on the reference corpus is like this.
        let c = import_main(&[
            &user("u1", "", "go"),
            r#"{"type":"assistant","sessionId":"s-1","uuid":"th1","parentUuid":"u1",
                "message":{"content":[{"type":"thinking","thinking":"","signature":"abc"}]}}"#,
        ]);
        assert_eq!(c.messages.len(), 1);
        assert!(c.messages.iter().all(|m| m.kind != Kind::Reasoning));
    }

    #[test]
    fn custom_title_outranks_everything_and_the_last_one_wins() {
        let c = import_main(&[
            &user("u1", "", "how do I parse jsonl"),
            r#"{"type":"ai-title","sessionId":"s-1","aiTitle":"Parsing JSONL in Rust"}"#,
            r#"{"type":"custom-title","sessionId":"s-1","customTitle":"first name"}"#,
            r#"{"type":"custom-title","sessionId":"s-1","customTitle":"second name"}"#,
            r#"{"type":"custom-title","sessionId":"s-1","customTitle":"final name"}"#,
        ]);

        assert_eq!(c.titles.custom.as_deref(), Some("final name"));
        assert_eq!(c.titles.generated.as_deref(), Some("Parsing JSONL in Rust"));
        assert_eq!(c.titles.first_user.as_deref(), Some("how do I parse jsonl"));
        assert_eq!(c.titles.resolve(), Some("final name"));
    }

    #[test]
    fn ai_title_outranks_the_first_user_message() {
        let c = import_main(&[
            &user("u1", "", "how do I parse jsonl"),
            r#"{"type":"ai-title","sessionId":"s-1","aiTitle":"Parsing JSONL in Rust"}"#,
        ]);
        assert_eq!(c.titles.custom, None);
        assert_eq!(c.titles.resolve(), Some("Parsing JSONL in Rust"));
    }

    #[test]
    fn titles_are_whitespace_collapsed_and_clamped_to_80_chars() {
        let long = "x".repeat(200);
        let c = import_main(&[
            &user("u1", "", "line one   \\n   line two"),
            &format!(r#"{{"type":"custom-title","sessionId":"s-1","customTitle":"{long}"}}"#),
        ]);
        assert_eq!(c.titles.custom.as_deref().unwrap().chars().count(), 80);
        assert_eq!(c.titles.first_user.as_deref(), Some("line one line two"));
    }

    #[test]
    fn slash_command_markup_is_stripped_and_falls_through_to_the_next_message() {
        let c = import_main(&[
            r#"{"type":"user","sessionId":"s-1","uuid":"u1","message":{"content":
                "<command-message>find-skills</command-message>
                 <command-name>/find-skills</command-name>"}}"#,
            &user("u2", "u1", "now the actual question"),
        ]);

        // The markup turn is still a message — only the *title* rejects it.
        assert_eq!(c.messages.len(), 2);
        assert!(c.messages[0].text.contains("<command-name>"));
        assert_eq!(c.titles.first_user.as_deref(), Some("now the actual question"));
    }

    #[test]
    fn command_args_survive_stripping_because_the_user_typed_them() {
        let c = import_main(&[
            r#"{"type":"user","sessionId":"s-1","uuid":"u1","message":{"content":
                "<command-name>/research</command-name><command-args>how does fts5 rank</command-args>"}}"#,
        ]);
        assert_eq!(c.titles.first_user.as_deref(), Some("how does fts5 rank"));
    }

    #[test]
    fn system_reminders_and_local_command_output_do_not_reach_titles() {
        let c = import_main(&[
            r#"{"type":"user","sessionId":"s-1","uuid":"u1","message":{"content":
                "<system-reminder>do not mention this</system-reminder>keep this<local-command-stdout>noise</local-command-stdout>"}}"#,
        ]);
        assert_eq!(c.titles.first_user.as_deref(), Some("keep this"));
    }

    #[test]
    fn an_unterminated_tag_does_not_swallow_the_rest_of_the_message() {
        assert_eq!(strip_injected_markup("<system-reminder>tail", true), "tail");
        assert_eq!(strip_injected_markup("a < b and c <d> e", true), "a < b and c <d> e");
        // An unclosed `ide_` tag is the same story: drop the marker, keep reading.
        assert_eq!(strip_injected_markup("<ide_selection>the rest", true), "the rest");
    }

    #[test]
    fn ide_context_injections_do_not_reach_titles() {
        // The editor extension's blob is its own user text block, closed, with nothing
        // after it — so the turn yields no title and the next message gets the job.
        let c = import_main(&[
            r#"{"type":"user","sessionId":"s-1","uuid":"u1","message":{"content":
                "<ide_opened_file>The user opened the file /w/notes.md in the IDE.
                 This may or may not be related to the current task.</ide_opened_file>"}}"#,
            r#"{"type":"user","sessionId":"s-1","uuid":"u2","parentUuid":"u1","message":{"content":
                "<ide_selection>The user selected the lines 1 to 8 from /w/notes.md:
                 - a bullet</ide_selection>"}}"#,
            &user("u3", "u2", "why is the loader slow"),
        ]);

        // All three are still messages; only the title rejects the injected two.
        assert_eq!(c.messages.len(), 3);
        assert_eq!(c.titles.first_user.as_deref(), Some("why is the loader slow"));

        // Prose typed alongside the injection survives it.
        let mixed = import_main(&[
            r#"{"type":"user","sessionId":"s-1","uuid":"u1","message":{"content":
                "<ide_opened_file>The user opened the file /w/notes.md in the IDE.</ide_opened_file>what changed here?"}}"#,
        ]);
        assert_eq!(mixed.titles.first_user.as_deref(), Some("what changed here?"));
    }

    #[test]
    fn a_setting_command_argues_with_the_harness_and_never_becomes_the_title() {
        // `/model opus` would otherwise title the conversation `opus`, because the args are
        // all that survives stripping.
        let c = import_main(&[
            r#"{"type":"user","sessionId":"s-1","uuid":"u1","message":{"content":
                "<command-name>/model</command-name>
                 <command-message>model</command-message>
                 <command-args>opus</command-args>"}}"#,
            r#"{"type":"user","sessionId":"s-1","uuid":"u2","parentUuid":"u1","message":{"content":
                "<command-name>/fast</command-name>
                 <command-message>fast</command-message>
                 <command-args>off</command-args>"}}"#,
            &user("u3", "u2", "now the actual question"),
        ]);
        assert_eq!(c.titles.first_user.as_deref(), Some("now the actual question"));

        // The exception in `UNWRAPPED_TAGS` still holds for every other command: a setting
        // command in the same turn is what turns it off, not the mere presence of args.
        let custom = import_main(&[
            r#"{"type":"user","sessionId":"s-1","uuid":"u1","message":{"content":
                "<command-name>/research</command-name><command-args>how does fts5 rank</command-args>"}}"#,
        ]);
        assert_eq!(custom.titles.first_user.as_deref(), Some("how does fts5 rank"));
    }

    #[test]
    fn a_harness_interruption_notice_is_not_prose_and_never_becomes_the_title() {
        let c = import_main(&[
            &user("u1", "", "[Request interrupted by user]"),
            &user("u2", "u1", "[Request interrupted by user for tool use]"),
            &user("u3", "u2", "ok, do it this way instead"),
        ]);

        // Still messages — the notices are part of the transcript — but not titles.
        assert_eq!(c.messages.len(), 3);
        assert_eq!(c.titles.first_user.as_deref(), Some("ok, do it this way instead"));

        // Prose around a notice survives it.
        let mixed = import_main(&[&user(
            "u1",
            "",
            "stop [Request interrupted by user] and read the file first",
        )]);
        assert_eq!(
            mixed.titles.first_user.as_deref(),
            Some("stop and read the file first")
        );
    }

    #[test]
    fn a_subagent_transcript_is_a_sidechain_on_its_own_thread_key() {
        // Note the *parent's* sessionId: the file, not the session, identifies the thread.
        let sub = "project/session-1/subagents/agent-explore.jsonl";
        let bytes = jsonl(&[
            r#"{"type":"user","sessionId":"s-1","uuid":"su1","isSidechain":true,
                "entrypoint":"cli","cwd":"/repo",
                "message":{"content":[{"type":"text","text":"explore the crate"}]}}"#,
            r#"{"type":"assistant","sessionId":"s-1","uuid":"sa1","parentUuid":"su1",
                "isSidechain":true,
                "message":{"content":[{"type":"text","text":"found it"}]}}"#,
        ]);

        let c = import(sub, &bytes).unwrap();
        assert_eq!(c.native_id, "s-1", "a subagent carries the parent's session id");
        assert!(c.messages.iter().all(|m| m.is_sidechain));
        assert!(c.messages.iter().all(|m| m.thread_key == sub));
        // A subagent reopens by resuming its parent session, which follows from carrying the
        // parent's id rather than from anything this importer formats (chat-search-me9.3).
        assert!(!cs_core::destinations(&c.source, &c.native_id).is_empty());

        // Same conversation id as the main transcript, different thread key.
        let main = import_main(&[&user("u1", "", "hello")]);
        assert_eq!(main.id(), c.id());
        assert_ne!(main.messages[0].thread_key, c.messages[0].thread_key);
        assert!(!main.messages[0].is_sidechain);
    }

    #[test]
    fn bare_string_content_is_user_prose() {
        let c = import_main(&[
            r#"{"type":"user","sessionId":"s-1","uuid":"u1","message":{"content":"just a string"}}"#,
        ]);
        assert_eq!(c.messages.len(), 1);
        assert_eq!(c.messages[0].kind, Kind::Prose);
        assert_eq!(c.messages[0].role, Role::User);
        assert_eq!(c.messages[0].text, "just a string");
    }

    #[test]
    fn metadata_is_first_non_null_even_when_cwd_moves_mid_session() {
        let c = import_main(&[
            r#"{"type":"user","sessionId":"s-1","uuid":"u1","cwd":null,"gitBranch":"",
                "entrypoint":"claude-vscode",
                "message":{"content":[{"type":"text","text":"one"}]}}"#,
            r#"{"type":"user","sessionId":"s-1","uuid":"u2","cwd":"/repo/a","gitBranch":"main",
                "entrypoint":"cli",
                "message":{"content":[{"type":"text","text":"two"}]}}"#,
            r#"{"type":"user","sessionId":"s-1","uuid":"u3","cwd":"/repo/b","gitBranch":"wip",
                "message":{"content":[{"type":"text","text":"three"}]}}"#,
        ]);

        // An explicit null and an empty string are both "absent", so the first *value* wins.
        assert_eq!(c.cwd.as_deref(), Some("/repo/a"));
        assert_eq!(c.git_branch.as_deref(), Some("main"));
        assert_eq!(c.surface.as_deref(), Some("claude-vscode"));
    }

    #[test]
    fn a_synthetic_assistant_turn_is_not_a_model() {
        let c = import_main(&[
            r#"{"type":"assistant","sessionId":"s-1","uuid":"a1",
                "message":{"model":"<synthetic>","content":[
                  {"type":"text","text":"API Error: overloaded"}]}}"#,
            r#"{"type":"assistant","sessionId":"s-1","uuid":"a2",
                "message":{"model":"claude-opus-4","content":[
                  {"type":"text","text":"ok"}]}}"#,
        ]);
        // The error notice keeps its message and loses only the fake model name.
        assert_eq!(c.messages[0].model, None);
        assert_eq!(c.messages[1].model.as_deref(), Some("claude-opus-4"));
    }

    #[test]
    fn timestamps_become_epoch_millis() {
        let c = import_main(&[&user("u1", "", "hi")]);
        assert_eq!(c.messages[0].ts, Some(1_785_265_791_319));

        assert_eq!(epoch_millis("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(epoch_millis("2026-07-28T19:09:52Z"), Some(1_785_265_792_000));
        assert_eq!(epoch_millis("2026-07-28T21:09:51.319+02:00"), Some(1_785_265_791_319));
        assert_eq!(epoch_millis("2026-07-28T17:09:51.319-0200"), Some(1_785_265_791_319));
        // fraction narrower and wider than milliseconds
        assert_eq!(epoch_millis("2026-07-28T19:09:51.3Z"), Some(1_785_265_791_300));
        assert_eq!(epoch_millis("2026-07-28T19:09:51.319999Z"), Some(1_785_265_791_319));

        assert_eq!(epoch_millis(""), None);
        assert_eq!(epoch_millis("2026-07-28"), None);
        assert_eq!(epoch_millis("not a timestamp!!!!"), None);
        assert_eq!(epoch_millis("2026-13-28T19:09:51Z"), None);
        assert_eq!(epoch_millis("2026-07-28T19:09:51.Z"), None);
    }

    #[test]
    fn a_message_with_an_unparseable_timestamp_still_imports() {
        let c = import_main(&[
            r#"{"type":"user","sessionId":"s-1","uuid":"u1","timestamp":"whenever",
                "message":{"content":"hi"}}"#,
        ]);
        assert_eq!(c.messages[0].ts, None);
    }

    #[test]
    fn a_truncated_tail_line_is_skipped_not_fatal() {
        let mut bytes = jsonl(&[&user("u1", "", "complete line")]);
        bytes.extend_from_slice(b"\n{\"type\":\"assistant\",\"uuid\":\"a1\",\"mess");

        let c = import(MAIN, &bytes).unwrap();
        assert_eq!(c.messages.len(), 1);
        assert_eq!(c.messages[0].text, "complete line");
    }

    #[test]
    fn garbage_and_empty_transcripts_return_none() {
        assert!(import(MAIN, b"").is_none());
        assert!(import(MAIN, b"not json at all\n\x00\x01\x02\n{").is_none());
        assert!(import(MAIN, b"[1,2,3]\n\"a string\"\n42").is_none());

        // Messages but no session id.
        let no_session = br#"{"type":"user","uuid":"u1","message":{"content":"hi"}}"#;
        assert!(import(MAIN, no_session).is_none());

        // Session id but nothing that yields a message.
        let no_messages = jsonl(&[
            r#"{"type":"ai-title","sessionId":"s-1","aiTitle":"A title"}"#,
            r#"{"type":"user","sessionId":"s-1","uuid":"u1","message":{"content":"   "}}"#,
            r#"{"type":"file-history-snapshot","sessionId":"s-1","snapshot":{}}"#,
        ]);
        assert!(import(MAIN, &no_messages).is_none());
    }

    #[test]
    fn ids_stay_unique_when_an_event_has_no_uuid() {
        let c = import_main(&[
            r#"{"type":"user","sessionId":"s-1","message":{"content":[
                 {"type":"text","text":"a"},{"type":"text","text":"b"}]}}"#,
            r#"{"type":"user","sessionId":"s-1","message":{"content":"c"}}"#,
        ]);
        let ids: Vec<_> = c.messages.iter().map(|m| m.native_id.clone()).collect();
        let unique: std::collections::HashSet<_> = ids.iter().collect();
        assert_eq!(ids.len(), 3);
        assert_eq!(ids.len(), unique.len(), "{ids:?}");
    }

    #[test]
    fn importing_the_same_bytes_twice_gives_the_same_records() {
        let events = [
            user("u1", "", "hello"),
            r#"{"type":"assistant","sessionId":"s-1","uuid":"a1","parentUuid":"u1",
                "message":{"content":[{"type":"thinking","thinking":"hm"},
                                      {"type":"text","text":"hi"}]}}"#
                .to_string(),
        ];
        let refs: Vec<&str> = events.iter().map(String::as_str).collect();
        let a = import_main(&refs);
        let b = import_main(&refs);
        assert_eq!(
            serde_json::to_string(&a).unwrap(),
            serde_json::to_string(&b).unwrap()
        );
    }
}
