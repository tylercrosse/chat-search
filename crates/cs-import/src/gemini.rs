//! Gemini CLI chat files (`~/.gemini/tmp/<projectHash>/chats/session-<ts>-<shortid>.json`).
//!
//! One file is one conversation: a JSON object carrying a `sessionId` and a flat `messages`
//! array. There is no branching, no subagent, no fork and no rename, so the DAG this
//! produces is a chain and the only title candidate is the opening user turn.
//!
//! The source globs `**/*.json` under the Gemini state directory, so two other file shapes
//! reach this importer and have to be declined:
//!
//! * `<projectHash>/logs.json` — a thin prompt log, a JSON *array* of `{sessionId, messageId,
//!   type, message, timestamp}`. Largely redundant with the chats files and not imported.
//! * `<projectHash>/checkpoints/*.json` — tool-write snapshots keyed `clientHistory`/
//!   `toolCall`, taken before a `write_file` or `replace`. Not conversation prose.
//!
//! Both are rejected by *shape* rather than by path: neither carries a `sessionId` alongside
//! a `messages` array, so there is no need to teach this module the archive's directory
//! layout, and a file that moves still parses the same way.

use cs_core::model::{model_name, Conversation, Kind, Message, Role, Titles};
use serde_json::Value;
use std::collections::HashSet;

const SOURCE: &str = "gemini-cli";
const TITLE_MAX_CHARS: usize = 80;

/// One archived chat file -> one conversation.
///
/// Returns `None` for anything that is not a chat file, and for a chat file that contributes
/// no messages — a session opened and abandoned is normal, not an error.
pub fn import(logical_path: &str, bytes: &[u8]) -> Option<Conversation> {
    // A truncated or non-JSON file is not an error either: the archive sweeps whatever is on
    // disk, including a chat still being written.
    let root: Value = serde_json::from_slice(bytes).ok()?;
    let session_id = text_field(&root, "sessionId").filter(|s| !s.is_empty())?;
    let records = field(&root, "messages")?.as_array()?;

    let mut messages: Vec<Message> = Vec::new();
    let mut first_user: Option<String> = None;
    let mut prev_native_id: Option<String> = None;
    let mut used_ids: HashSet<String> = HashSet::new();
    let mut seq: i64 = 0;

    for record in records {
        let text = flatten(field(record, "content"));
        let text = text.trim();
        // An empty turn is not a message. Skipping here — *before* `prev_native_id` moves —
        // is what re-parents the next message onto the nearest surviving ancestor instead of
        // onto the node that was dropped (see `Message::parent_native_id`).
        if text.is_empty() {
            continue;
        }

        let role = match text_field(record, "type").as_deref() {
            Some("user") => Role::User,
            Some("gemini") => Role::Assistant,
            // Only `user` and `gemini` were expected, but the reference archive also holds
            // `info` — harness chatter such as "Code Assist login required". An unrecognized
            // type is kept rather than dropped: we do not know it is worthless, and losing a
            // turn is the more expensive mistake. `System` keeps it out of the title, which
            // only ever comes from a `user` turn.
            _ => Role::System,
        };

        // The record states which model answered, so it is carried on the message rather
        // than collapsed here; the single conversation label is the indexer's (ADR 24). Only
        // a `gemini` record names one, so a user turn is null either way.
        let model = text_field(record, "model")
            .as_deref()
            .and_then(model_name)
            .filter(|_| role == Role::Assistant)
            .map(str::to_string);

        if role == Role::User && first_user.is_none() {
            first_user = Some(clamp_title(text));
        }

        let native_id = native_id(record, seq, &mut used_ids);
        messages.push(Message {
            parent_native_id: prev_native_id.replace(native_id.clone()),
            native_id,
            // The logical path, carried in rather than parsed back out of anything (ADR 4).
            thread_key: logical_path.to_string(),
            // This format has no subagent concept.
            is_sidechain: false,
            // Chat files hold only prose, so there is no result here to have failed.
            is_error: false,
            seq,
            role,
            // Every record here is prose. Tool calls do not appear in a chat file at all;
            // they are what the sibling `checkpoints/` files hold.
            kind: Kind::Prose,
            ts: text_field(record, "timestamp").as_deref().and_then(epoch_millis),
            model,
            text: text.to_string(),
        });
        seq += 1;
    }

    if messages.is_empty() {
        return None;
    }

    Some(Conversation {
        source: SOURCE.to_string(),
        native_id: session_id,
        // No rename and no generated title in this format; the opening user turn is all
        // there is.
        titles: Titles { custom: None, generated: None, first_user },
        // `projectHash` is a hash *of* the project directory, not the directory, so there is
        // nothing here a reader could act on. A hash in this field would look like a path.
        cwd: None,
        git_branch: None,
        // Nothing outside a `gemini` record names a model, so there is no fallback to give.
        declared_model: None,
        surface: None,
        forked_from_native_id: None,
        // Deliberately absent rather than a `gemini --resume`-shaped guess: an invented
        // command that does not work is worse than none, and the field is being removed
        // outright by chat-search-me9.3.
        // Append-only, so the last message is the head.
        head_native_id: None,
        messages,
    })
}

/// A message's id: its own, when it has one.
///
/// `Message::native_id` asks for file-scoped ids only where the source's id is merely
/// file-unique. Gemini's is a UUID, and using it is what makes a re-archived session
/// idempotent. The reference archive holds three sessions twice over — once under the
/// project hash and once under a friendly project directory the CLI also writes to — and
/// with per-file ordinals the second copy would arrive under the same conversation with a
/// fresh set of ids and double it. With the records' own ids the index's `INSERT OR IGNORE`
/// folds the copies together, and a session re-saved after growing merges instead of
/// duplicating.
///
/// The ordinal is the fallback for a record with no id, and for a repeat of one already used
/// in this file — that would otherwise be swallowed by the same `INSERT OR IGNORE`, silently
/// losing a real message.
fn native_id(record: &Value, seq: i64, used: &mut HashSet<String>) -> String {
    let id = match text_field(record, "id").filter(|id| !id.is_empty()) {
        Some(id) if !used.contains(&id) => id,
        _ => format!("msg-{seq:06}"),
    };
    used.insert(id.clone());
    id
}

/// Whitespace-collapsed and clamped. Titles sit in a list; a newline in one would move every
/// row below it.
fn clamp_title(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(TITLE_MAX_CHARS).collect()
}

/// `Value::get` returns `Some(Value::Null)` for an explicit JSON null, which would make the
/// `filter`/`and_then` chains above treat a null as a present value. Every lookup goes
/// through here, collapsing "absent" and "null" into `None`.
fn field<'a>(v: &'a Value, key: &str) -> Option<&'a Value> {
    match v.get(key) {
        None | Some(Value::Null) => None,
        Some(found) => Some(found),
    }
}

fn text_field(v: &Value, key: &str) -> Option<String> {
    field(v, key).and_then(Value::as_str).map(str::to_string)
}

/// Collapse a `content` value into searchable text.
///
/// Almost always a plain string. The array-of-parts form — `[{"text":"exit"}]` — is what the
/// CLI writes when a turn arrives as content parts rather than a string.
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
            for k in ["text", "content"] {
                if let Some(inner) = o.get(k) {
                    return flatten(Some(inner));
                }
            }
            String::new()
        }
        Some(other) => other.to_string(),
    }
}

/// RFC3339 -> epoch milliseconds.
///
/// A local copy: the same parser exists in `codex.rs` and `claude_code.rs`, and lifting all
/// three into one shared module is tracked as chat-search-n58.8. Doing it here would mean
/// editing two files that already have uncommitted changes, so this waits for that issue.
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

    // Fixtures are synthetic on purpose. Real chats carry secrets, and a golden file cut from
    // one cannot be shared or reviewed (chat-search-4ar.3).

    const MAIN_PATH: &str = "f9b6b2ab/chats/session-2025-10-19T21-38-fb65e7fe.json";

    const MAIN: &str = r#"{
      "sessionId": "fb65e7fe-1954-4a96-b746-7d6c0a411e03",
      "projectHash": "f9b6b2ab50b62230b491ba486d76fd5347ab683768356255c91e4fa5b2a2750b",
      "startTime": "2025-10-19T21:38:01.823Z",
      "lastUpdated": "2025-10-19T21:39:15.520Z",
      "messages": [
        {"id":"11111111-1111-4111-8111-111111111111","timestamp":"2025-10-19T21:38:01.823Z",
         "type":"user","content":"  what is   the\n  difference  "},
        {"id":"22222222-2222-4222-8222-222222222222","timestamp":"2025-10-19T21:38:05.497Z",
         "type":"gemini","content":"One resolves, the other does not.","model":"gemini-2.5-pro",
         "thoughts":[{"subject":"Scoping","description":"reading the docs"}],"tokens":{"total":12}},
        {"id":"33333333-3333-4333-8333-333333333333","timestamp":"2025-10-19T21:39:15.520Z",
         "type":"user","content":"thanks"}
      ]
    }"#;

    fn conv(path: &str, json: &str) -> Conversation {
        import(path, json.as_bytes()).expect("fixture should import")
    }

    #[test]
    fn a_chat_file_maps_roles_keeps_order_and_chains_parents() {
        let c = conv(MAIN_PATH, MAIN);

        assert_eq!(c.source, "gemini-cli");
        assert_eq!(c.native_id, "fb65e7fe-1954-4a96-b746-7d6c0a411e03");
        assert_eq!(c.id(), "gemini-cli:fb65e7fe-1954-4a96-b746-7d6c0a411e03");
        // Only the `gemini` turn names one; nothing at the conversation level ever does.
        assert_eq!(
            c.messages.iter().map(|m| m.model.as_deref()).collect::<Vec<_>>(),
            vec![None, Some("gemini-2.5-pro"), None]
        );
        assert_eq!(c.declared_model, None);
        // `projectHash` is a hash, not a path, so it must not land in `cwd`.
        assert_eq!(c.cwd, None);
        assert_eq!(c.git_branch, None);
        assert_eq!(c.surface, None);
        assert_eq!(c.forked_from_native_id, None);
        assert_eq!(c.head_native_id, None);
        // Never a fabricated resume command (chat-search-me9.3). Gemini CLI writes no resumable
        // session, so the honest answer is nothing to offer — not a command that would fail.
        assert!(cs_core::destinations(&c.source, &c.native_id).is_empty());

        let shape: Vec<_> = c.messages.iter().map(|m| (m.role, m.kind)).collect();
        assert_eq!(
            shape,
            vec![
                (Role::User, Kind::Prose),
                (Role::Assistant, Kind::Prose),
                (Role::User, Kind::Prose),
            ]
        );

        // Ids are the records' own.
        let ids: Vec<_> = c.messages.iter().map(|m| m.native_id.as_str()).collect();
        assert_eq!(
            ids,
            vec![
                "11111111-1111-4111-8111-111111111111",
                "22222222-2222-4222-8222-222222222222",
                "33333333-3333-4333-8333-333333333333",
            ]
        );

        // Linear chain: the first message is a root, every later one points at its predecessor.
        assert_eq!(c.messages[0].parent_native_id, None);
        for pair in c.messages.windows(2) {
            assert_eq!(pair[1].parent_native_id.as_deref(), Some(pair[0].native_id.as_str()));
        }
        for (i, m) in c.messages.iter().enumerate() {
            assert_eq!(m.seq, i as i64);
            // ADR 4: the thread key is the logical path, verbatim.
            assert_eq!(m.thread_key, MAIN_PATH);
            assert!(!m.is_sidechain);
        }

        assert_eq!(c.messages[1].text, "One resolves, the other does not.");
        assert_eq!(c.messages[0].ts, Some(1_760_909_881_823));
        assert_eq!(c.resolved_head(), Some("33333333-3333-4333-8333-333333333333"));
    }

    #[test]
    fn the_title_is_the_first_user_turn_collapsed_and_truncated() {
        let c = conv(MAIN_PATH, MAIN);
        assert_eq!(c.titles.custom, None);
        assert_eq!(c.titles.generated, None);
        // Collapsed, and taken from the *first* user turn rather than the last.
        assert_eq!(c.titles.resolve(), Some("what is the difference"));

        // An assistant turn first must not supply the title.
        let assistant_first = r#"{"sessionId":"s","messages":[
          {"id":"a","type":"gemini","content":"hello there"},
          {"id":"b","type":"user","content":"the real question"}]}"#;
        assert_eq!(
            conv("p/chats/s.json", assistant_first).titles.resolve(),
            Some("the real question")
        );

        let long = format!(
            r#"{{"sessionId":"s","messages":[{{"id":"a","type":"user","content":"{}"}}]}}"#,
            "w ".repeat(100)
        );
        assert_eq!(
            conv("p/chats/long.json", &long).titles.resolve().unwrap().chars().count(),
            TITLE_MAX_CHARS
        );

        // Nothing but assistant turns leaves the conversation untitled.
        let no_user = r#"{"sessionId":"s","messages":[{"id":"a","type":"gemini","content":"hi"}]}"#;
        assert_eq!(conv("p/chats/nouser.json", no_user).titles.resolve(), None);
    }

    #[test]
    fn a_skipped_message_re_parents_its_child_onto_the_nearest_survivor() {
        // The middle turn is empty and never emitted, so the third must point at the first —
        // not at the dropped node, which would dangle and terminate `index::on_head_path`.
        let gappy = r#"{"sessionId":"s","messages":[
          {"id":"a","type":"user","content":"first"},
          {"id":"b","type":"gemini","content":"   \n  "},
          {"id":"c","type":"user","content":"third"},
          {"id":"d","type":"gemini","content":""},
          {"id":"e","type":"gemini","content":[]},
          {"id":"f","type":"gemini","content":"last"}]}"#;
        let c = conv("p/chats/gappy.json", gappy);

        let ids: Vec<_> = c.messages.iter().map(|m| m.native_id.as_str()).collect();
        assert_eq!(ids, vec!["a", "c", "f"]);
        assert_eq!(c.messages[0].parent_native_id, None);
        assert_eq!(c.messages[1].parent_native_id.as_deref(), Some("a"));
        assert_eq!(c.messages[2].parent_native_id.as_deref(), Some("c"));

        // Ordinals stay dense: only emitted messages consume one.
        assert_eq!(c.messages.iter().map(|m| m.seq).collect::<Vec<_>>(), vec![0, 1, 2]);

        // Every parent names a message that was actually emitted.
        let emitted: HashSet<&str> = ids.iter().copied().collect();
        for m in &c.messages {
            if let Some(p) = &m.parent_native_id {
                assert!(emitted.contains(p.as_str()), "dangling parent {p}");
            }
        }
    }

    #[test]
    fn an_unrecognized_type_is_kept_rather_than_dropped() {
        // `info` is real: the reference archive holds auth chatter under it.
        let with_info = r#"{"sessionId":"s","messages":[
          {"id":"a","type":"info","content":"Code Assist login required."},
          {"id":"b","type":"user","content":"now the actual question"},
          {"id":"c","type":"quinoa","content":"a type nobody has seen"}]}"#;
        let c = conv("p/chats/info.json", with_info);

        assert_eq!(c.messages.len(), 3);
        assert_eq!(c.messages[0].role, Role::System);
        assert_eq!(c.messages[2].role, Role::System);
        assert_eq!(c.messages[0].text, "Code Assist login required.");
        // Kept, but never the title — that comes only from a `user` turn.
        assert_eq!(c.titles.resolve(), Some("now the actual question"));
        // Still chained, so the unknown turns do not break the walk.
        assert_eq!(c.messages[1].parent_native_id.as_deref(), Some("a"));
        assert_eq!(c.messages[2].parent_native_id.as_deref(), Some("b"));
    }

    #[test]
    fn content_arrives_as_a_string_or_as_content_parts() {
        let parts = r#"{"sessionId":"s","messages":[
          {"id":"a","type":"user","content":[{"text":"exit"}]},
          {"id":"b","type":"gemini","content":[{"text":"line a"},{"text":"line b"}]},
          {"id":"c","type":"user","content":"plain string"}]}"#;
        let c = conv("p/chats/parts.json", parts);
        assert_eq!(c.messages[0].text, "exit");
        assert_eq!(c.messages[1].text, "line a\nline b");
        assert_eq!(c.messages[2].text, "plain string");
    }

    #[test]
    fn nothing_worth_indexing_yields_none() {
        // An empty `messages` array is a real, normal file: a session opened and abandoned.
        let empty = r#"{"sessionId":"s","projectHash":"h","messages":[]}"#;
        assert!(import("p/chats/empty.json", empty.as_bytes()).is_none());

        // As is one whose every turn is blank.
        let blank = r#"{"sessionId":"s","messages":[
          {"id":"a","type":"user","content":"   "},
          {"id":"b","type":"gemini","content":null}]}"#;
        assert!(import("p/chats/blank.json", blank.as_bytes()).is_none());

        // Malformed or truncated input must return None, never panic.
        assert!(import("p/chats/none.json", b"").is_none());
        assert!(import("p/chats/garbage.json", b"not json at all\n{{{").is_none());
        assert!(import("p/chats/binary.json", b"\x00\xff\xfe").is_none());
        let truncated = &MAIN.as_bytes()[..MAIN.len() / 2];
        assert!(import(MAIN_PATH, truncated).is_none());

        // A session id is what a conversation hangs off.
        let anonymous = r#"{"messages":[{"id":"a","type":"user","content":"hi"}]}"#;
        assert!(import("p/chats/anon.json", anonymous.as_bytes()).is_none());
        let blank_id = r#"{"sessionId":"","messages":[{"id":"a","type":"user","content":"hi"}]}"#;
        assert!(import("p/chats/blankid.json", blank_id.as_bytes()).is_none());
        let null_id = r#"{"sessionId":null,"messages":[{"id":"a","type":"user","content":"hi"}]}"#;
        assert!(import("p/chats/nullid.json", null_id.as_bytes()).is_none());
    }

    #[test]
    fn the_other_two_file_shapes_under_the_source_are_declined() {
        // The source globs `**/*.json`, so these arrive here and must be rejected by shape.

        // logs.json: a JSON array, and one whose records do carry a `sessionId`.
        let logs = r#"[{"sessionId":"74eb0b66-5c68-4b32-808b-4b1e275b7cfe","messageId":0,
          "type":"user","message":"/theme","timestamp":"2025-07-02T15:54:26.800Z"}]"#;
        assert!(import("p/logs.json", logs.as_bytes()).is_none());
        assert!(import("p/logs.json", b"[]").is_none());

        // checkpoints/*.json: an object, but keyed `clientHistory`/`toolCall`.
        let checkpoint = r#"{"clientHistory":[{"role":"user","parts":[{"text":"setting up"}]}],
          "commitHash":"abc123","messageId":4,"toolCall":{"name":"write_file","args":{}}}"#;
        assert!(import("p/checkpoints/x-write_file.json", checkpoint.as_bytes()).is_none());

        // A `messages` key that is not an array is not a chat file either.
        let odd = r#"{"sessionId":"s","messages":{"0":{"type":"user","content":"hi"}}}"#;
        assert!(import("p/chats/odd.json", odd.as_bytes()).is_none());
    }

    #[test]
    fn the_same_session_archived_twice_produces_identical_ids() {
        // The reference archive writes some projects under both a hash and a friendly name,
        // so one session shows up as two files. Message ids come from the records rather than
        // from per-file ordinals precisely so the index folds the copies instead of doubling
        // the conversation.
        let a = conv(MAIN_PATH, MAIN);
        let b = conv("personal-site/chats/session-2025-10-19T21-38-fb65e7fe.json", MAIN);

        assert_eq!(a.id(), b.id());
        let ids = |c: &Conversation| {
            c.messages.iter().map(|m| m.native_id.clone()).collect::<Vec<_>>()
        };
        assert_eq!(ids(&a), ids(&b));
        // Only the thread key, which is the logical path, differs.
        assert_ne!(a.messages[0].thread_key, b.messages[0].thread_key);

        // And the same bytes at the same path are byte-for-byte reproducible (ADR 7).
        assert_eq!(ids(&a), ids(&conv(MAIN_PATH, MAIN)));
        assert_eq!(a.titles.resolve(), conv(MAIN_PATH, MAIN).titles.resolve());
    }

    #[test]
    fn a_missing_or_repeated_record_id_falls_back_to_an_ordinal() {
        // A repeat would be swallowed by the index's `INSERT OR IGNORE`, losing a real turn.
        let messy = r#"{"sessionId":"s","messages":[
          {"id":"dup","type":"user","content":"one"},
          {"type":"gemini","content":"two"},
          {"id":"","type":"user","content":"three"},
          {"id":"dup","type":"gemini","content":"four"}]}"#;
        let c = conv("p/chats/messy.json", messy);

        let ids: Vec<_> = c.messages.iter().map(|m| m.native_id.as_str()).collect();
        assert_eq!(ids, vec!["dup", "msg-000001", "msg-000002", "msg-000003"]);
        let distinct: HashSet<&str> = ids.iter().copied().collect();
        assert_eq!(distinct.len(), ids.len());
        // The chain still holds across the substitutions.
        for pair in c.messages.windows(2) {
            assert_eq!(pair[1].parent_native_id.as_deref(), Some(pair[0].native_id.as_str()));
        }
    }

    #[test]
    fn each_turn_keeps_the_model_that_answered_it() {
        let switched = r#"{"sessionId":"s","messages":[
          {"id":"a","type":"user","content":"hi"},
          {"id":"b","type":"gemini","content":"hello","model":"gemini-2.5-pro"},
          {"id":"c","type":"gemini","content":"still here","model":"gemini-3-flash-preview"}]}"#;
        let c = conv("p/chats/model.json", switched);
        assert_eq!(
            c.messages.iter().map(|m| m.model.as_deref()).collect::<Vec<_>>(),
            vec![None, Some("gemini-2.5-pro"), Some("gemini-3-flash-preview")]
        );

        // User-only conversations name no model rather than an empty string.
        let none = r#"{"sessionId":"s","messages":[{"id":"a","type":"user","content":"hi"}]}"#;
        let c = conv("p/chats/nomodel.json", none);
        assert_eq!(c.declared_model, None);
        assert!(c.messages.iter().all(|m| m.model.is_none()));
    }

    #[test]
    fn a_message_without_a_usable_timestamp_still_imports() {
        let untimed = r#"{"sessionId":"s","messages":[
          {"id":"a","type":"user","content":"no clock here"},
          {"id":"b","timestamp":"garbage","type":"gemini","content":"nor here"},
          {"id":"c","timestamp":null,"type":"user","content":"nor here either"}]}"#;
        let c = conv("p/chats/untimed.json", untimed);
        assert_eq!(c.messages.len(), 3);
        assert!(c.messages.iter().all(|m| m.ts.is_none()));
    }

    #[test]
    fn timestamps_become_epoch_millis() {
        // The shape every record in the reference archive uses.
        assert_eq!(epoch_millis("2025-10-19T21:38:01.823Z"), Some(1_760_909_881_823));
        assert_eq!(epoch_millis("1970-01-01T00:00:00Z"), Some(0));
        // Leap day, and a century that is not a leap year.
        assert_eq!(epoch_millis("2024-02-29T00:00:00Z"), Some(1_709_164_800_000));
        assert_eq!(epoch_millis("1900-03-01T00:00:00Z"), Some(-2_203_891_200_000));
        // Sub-millisecond digits are read and discarded, not misread as an offset.
        assert_eq!(epoch_millis("2026-07-28T14:46:55.123456789Z"), Some(1_785_250_015_123));
        assert_eq!(epoch_millis("2026-07-28T14:46:55.1Z"), Some(1_785_250_015_100));
        // Offsets, both separator styles, and a missing zone read as UTC.
        assert_eq!(epoch_millis("2026-07-28T16:46:55+02:00"), Some(1_785_250_015_000));
        assert_eq!(epoch_millis("2026-07-28T16:46:55+0200"), Some(1_785_250_015_000));
        assert_eq!(epoch_millis("2026-07-28T14:46:55"), Some(1_785_250_015_000));

        for bad in [
            "", "nope", "2025-10-19", "2026-13-01T00:00:00Z", "2026-07-32T00:00:00Z",
            "2026-07-28T25:00:00Z", "2026-07-28T14:46:55.Z", "2026-07-28T14:46:55~",
        ] {
            assert_eq!(epoch_millis(bad), None, "{bad} should not parse");
        }
    }

    /// Sanity check against a real Gemini state directory. Ignored by default and driven by
    /// an env var, so no personal data reaches the repo or CI, and it prints counts only:
    ///
    /// ```sh
    /// CS_GEMINI_TMP=~/.gemini/tmp cargo test -p cs-import -- --ignored --nocapture
    /// ```
    #[test]
    #[ignore = "needs a real archive; set CS_GEMINI_TMP to a ~/.gemini/tmp directory"]
    fn real_corpus_stats() {
        use std::collections::HashMap;

        let Ok(dir) = std::env::var("CS_GEMINI_TMP") else { return };
        fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            let Ok(entries) = std::fs::read_dir(dir) else { return };
            for e in entries.flatten() {
                let p = e.path();
                if p.is_dir() {
                    walk(&p, out);
                } else if p.extension().is_some_and(|x| x == "json") {
                    out.push(p);
                }
            }
        }
        let mut files = Vec::new();
        walk(std::path::Path::new(&dir), &mut files);
        files.sort();

        let root = std::path::Path::new(&dir);
        let (mut imported, mut skipped, mut msgs) = (0, 0, 0);
        let mut roles: HashMap<&str, usize> = HashMap::new();
        let mut ids_by_conversation: HashMap<String, HashSet<String>> = HashMap::new();

        for f in &files {
            let logical = f.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/");
            let Some(c) = import(&logical, &std::fs::read(f).unwrap()) else {
                skipped += 1;
                continue;
            };
            imported += 1;
            msgs += c.messages.len();
            let seen = ids_by_conversation.entry(c.id()).or_default();
            let emitted: HashSet<&str> =
                c.messages.iter().map(|m| m.native_id.as_str()).collect();
            for (i, m) in c.messages.iter().enumerate() {
                *roles.entry(m.role.as_str()).or_default() += 1;
                assert_eq!(m.seq, i as i64, "seq must be dense and ordered in {logical}");
                assert_eq!(m.thread_key, logical, "thread key must be the logical path");
                if let Some(p) = &m.parent_native_id {
                    assert!(emitted.contains(p.as_str()), "dangling parent in {logical}");
                }
                seen.insert(m.native_id.clone());
            }
        }

        let mut roles: Vec<_> = roles.into_iter().collect();
        roles.sort_unstable();
        println!(
            "files={} imported={imported} skipped={skipped} conversations={} messages={msgs} \
             roles={roles:?}",
            files.len(),
            ids_by_conversation.len()
        );
        assert!(imported > 0 && msgs > 0);
    }
}
