//! Google Takeout — four conversation formats from one export (chat-search-o1i).
//!
//! A Takeout archive is not a live directory the way `~/.codex/sessions` is; it is a
//! one-shot dump the user unpacks somewhere. `chatgpt-export` already works this way, and
//! this follows it: one permanent source id (ADR 16), pointed at wherever exports land.
//!
//! # The four formats
//!
//! | logical path | what it is |
//! |---|---|
//! | `*/My Activity/Gemini Apps/MyActivity.json` | gemini.google.com, 1,173 records |
//! | `*/My Activity/AI Mode/MyActivity.json` | Google Search AI Mode, 145 records |
//! | `*/Gemini in Workspace/Conversation History/conversation_*.txt` | Docs/Gmail sidebar, 7 files |
//! | `*/NotebookLM/<notebook>/Chat History/Chat Session - <uuid>.html` | NotebookLM, 2 sessions |
//!
//! They share an export mechanism and nothing else, so `import_all` dispatches on the shape
//! of the path and each format gets its own function.
//!
//! # Configuring it
//!
//! Like `chatgpt-export`, this source has no canonical home directory — it points at wherever
//! exports are unpacked — so it is hand-added to `config.toml` rather than auto-detected by
//! `candidate_sources`, which could only ever guess at a path.
//!
//! ```toml
//! [[sources]]
//! id = "google-takeout"
//! path = "~/dev/sandbox/chat-history/data"   # the directory exports unpack *into*
//! layout = "mirror"
//! include = [
//!   "**/My Activity/*/MyActivity.json",
//!   "**/Conversation History/conversation_*.txt",
//!   "**/Chat History/*.html",
//! ]
//! ```
//!
//! Two things about those globs are deliberate. They lead with `**/` rather than naming the
//! export directory, because Takeout unpacks to a bare `Takeout/` every time and the only way
//! to keep two exports apart is to rename them — so the pattern cannot assume what they are
//! called, and `Takeout*` would silently miss a directory renamed to lowercase. And they name
//! conversation files rather than sweeping the tree, because 311 MB of the 330 MB reference
//! export is NotebookLM source PDFs and `.wav` audio overviews. The globs above capture
//! 9.5 MB of the 330 and leave the rest on disk.
//!
//! # Why one source id and not four
//!
//! A source id is permanent and becomes part of every conversation id (ADR 16), so spending
//! four of them to describe one export is a cost that never comes back. What actually
//! distinguishes these is [`Conversation::surface`], which exists for fine-grained origin and
//! is explicitly never part of an id. `agent:google-takeout` finds all of it;
//! `surface` separates `gemini-apps` from `notebooklm` within it.
//!
//! # MyActivity has no conversation id
//!
//! This is the one thing worth knowing before reading the code. `MyActivity.json` is an
//! **activity log, not a transcript**: there is no conversation, thread, or session key at
//! any nesting level of any record — verified across all 1,318 records of the reference
//! export. A twenty-turn chat is twenty independent `(prompt, response, time)` records with
//! nothing linking them.
//!
//! So one record becomes one two-message conversation. The tempting alternative — regroup
//! records into conversations by clustering on time gaps — is what this deliberately does
//! not do, because a conversation id must be permanent and an id derived from a heuristic
//! changes the day the heuristic does. Every conversation in the corpus would get a new id
//! the first time someone tuned the gap threshold, and the archive would gain a duplicate
//! copy of all of it. Turning the log back into threads belongs at *read* time, where being
//! wrong is free, and is filed as chat-search-o1i.5.
//!
//! `native_id` is therefore the record's own `time`. It is unique — 1,173 and 145 distinct
//! within each file, and zero collisions across them, measured — it is a property of the
//! record rather than of the export, and it is not derived from anything we compute. That
//! last part is what makes re-exporting safe: the same logical record keeps its id, so a
//! second export collapses onto the first through `INSERT OR IGNORE` instead of doubling the
//! corpus. It is the same overlap-collapse property ADR 9 relies on for ChatGPT.

use crate::html_text::to_text;
use crate::timestamp::epoch_millis;
use cs_core::model::{Conversation, Kind, Message, Role, Titles};
use serde_json::Value;

const SOURCE: &str = "google-takeout";
const TITLE_MAX_CHARS: usize = 80;

/// One archived file -> zero or more conversations.
///
/// Returns empty for anything this module does not recognise. The include globs already
/// narrow what reaches here, but a glob describes where a file sits and this describes what
/// it is, so an export that moves a directory still parses (the rule `gemini.rs` follows).
pub fn import_all(logical_path: &str, bytes: &[u8]) -> Vec<Conversation> {
    let file = logical_path.rsplit('/').next().unwrap_or(logical_path);

    if file.eq_ignore_ascii_case("MyActivity.json") {
        return activity_log(logical_path, bytes);
    }
    if logical_path.contains("/Conversation History/") && file.starts_with("conversation_") {
        return workspace(logical_path, bytes).into_iter().collect();
    }
    if logical_path.contains("/Chat History/") && file.to_ascii_lowercase().ends_with(".html") {
        return notebook_lm(logical_path, bytes).into_iter().collect();
    }
    Vec::new()
}

// ---------------------------------------------------------------------------
// My Activity — Gemini Apps and AI Mode
// ---------------------------------------------------------------------------

/// A flat JSON array of activity records; each becomes one conversation.
fn activity_log(logical_path: &str, bytes: &[u8]) -> Vec<Conversation> {
    let Ok(Value::Array(records)) = serde_json::from_slice::<Value>(bytes) else {
        return Vec::new();
    };
    records.iter().filter_map(|r| activity_record(logical_path, r)).collect()
}

/// One `(prompt, response, time)` record -> one two-message conversation.
///
/// Returns `None` when the record carries no text at all. 15 records in the reference export
/// are a bare `"Used Gemini Apps"` with no prompt and no response — a session that touched
/// the product without saying anything. There is nothing there to find.
fn activity_record(logical_path: &str, record: &Value) -> Option<Conversation> {
    let time = text_field(record, "time").filter(|t| !t.is_empty())?;
    let ts = epoch_millis(&time);

    // `header` is "Gemini Apps" or "AI Mode" — read from the record rather than from the
    // directory it sits in, so the two products stay distinguishable even if a future export
    // packs them into one file.
    let surface = text_field(record, "header").map(|h| slug(&h));

    let title = text_field(record, "title").unwrap_or_default();
    let prompt = spoken_text(&title).unwrap_or_default();

    let mut user_text = prompt.clone();
    // Attachment filenames are the only trace left of a file the user dropped into the chat,
    // and they are frequently what makes it recognisable later — "the one where I uploaded
    // the Xen paper". `imageFile` is always one of `attachedFiles` (60 of 60 in the reference
    // export), so it contributes nothing on its own and is not read.
    let attached = string_list(record, "attachedFiles");
    if !attached.is_empty() {
        user_text = join_block(&user_text, &format!("[attached: {}]", attached.join(", ")));
    }

    // `safeHtmlItem` is the response, in 1 to 4 HTML chunks.
    let response_html: Vec<String> = field(record, "safeHtmlItem")
        .and_then(Value::as_array)
        .map(|items| items.iter().filter_map(|i| text_field(i, "html")).collect())
        .unwrap_or_default();
    let mut assistant_text = to_text(&response_html.join("\n"));

    // `subtitles` is two things wearing one name: real web citations, and display names for
    // the local attachments above. Only the former is worth keeping — the latter is already
    // covered by `[attached: …]` and would double every filename in the index.
    let sources = cited_urls(record);
    if !sources.is_empty() {
        assistant_text = join_block(&assistant_text, &format!("[sources: {}]", sources.join(", ")));
    }

    let mut messages = Vec::new();
    if !user_text.is_empty() {
        messages.push(message("prompt", None, logical_path, 0, Role::User, ts, user_text));
    }
    if !assistant_text.is_empty() {
        // Parent is the prompt only when one was emitted. A response whose prompt was empty
        // has to be a root, or `index::on_head_path` walks into a message that does not
        // exist and quietly marks this conversation as off-head-path.
        let parent = messages.first().map(|m: &Message| m.native_id.clone());
        messages.push(message(
            "response",
            parent,
            logical_path,
            1,
            Role::Assistant,
            ts,
            assistant_text,
        ));
    }
    if messages.is_empty() {
        return None;
    }

    Some(Conversation {
        source: SOURCE.to_string(),
        native_id: time,
        // The prompt is the only title candidate. Google generates no title for an activity
        // record and there is nothing to rename.
        titles: Titles { custom: None, generated: None, first_user: non_empty(clamp(&prompt)) },
        cwd: None,
        git_branch: None,
        // No model is recorded anywhere in the export — not per record, not per file. Left
        // absent rather than filled with "gemini", which would assert a precision the data
        // does not have.
        model: None,
        surface,
        forked_from_native_id: None,
        head_native_id: None,
        messages,
    })
}

/// What the user actually said, or `None` when the title is only bookkeeping.
///
/// A record's `title` doubles as its content and as its activity label, so the verb decides
/// which. Measured over the reference export's 1,318 records:
///
/// ```text
/// Prompted <prompt>                    1081 + 138 (AI Mode "Searched for")  the real thing
/// Used Gemini Apps / Used AI Mode        22   0 with a response  bare product-touch
/// Created … Canvas titled <name>         33   0 with a response  24 of them empty or a URL
/// Gave feedback: Bad response (…)         1   0 with a response  a rating, not a turn
/// ```
///
/// The dropped forms are boilerplate a person cannot search for and would only ever appear
/// as noise in results. What is *kept* among them is the named canvas — "Created Gemini
/// Canvas titled Anthropic Application Draft" is genuinely findable, and the name is the
/// only record that the canvas ever existed.
fn spoken_text(title: &str) -> Option<String> {
    let title = title.trim();
    if let Some(prompt) = title.strip_prefix("Prompted ") {
        return non_empty(prompt.trim().to_string());
    }
    // "Used <product>" carries no object at all; the product is already the `header`.
    if title.starts_with("Used ") || title.starts_with("Gave feedback") {
        return None;
    }
    // Split on the word, not on `" titled "` with a trailing space: the 9 nameless canvases
    // arrive as `"Created Gemini Canvas titled "`, whose space this function already trimmed.
    if let Some((_, name)) = title.split_once(" titled") {
        let name = name.trim();
        // 24 of 33 canvases are titled with nothing, or with a placeholder
        // `googleusercontent.com` URL that names no document.
        if name.is_empty() || name.starts_with("http://") || name.starts_with("https://") {
            return None;
        }
    }
    non_empty(title.to_string())
}

/// Cited web sources, excluding the entries that are really attachment display names.
///
/// 122 of 185 `subtitles` entries in the reference export have a `url` that is a local
/// filename rather than a link, which is why this filters on scheme instead of taking them
/// all. Deduplicated: a response citing one document six times lists it once.
fn cited_urls(record: &Value) -> Vec<String> {
    let mut seen = Vec::new();
    let Some(items) = field(record, "subtitles").and_then(Value::as_array) else {
        return seen;
    };
    for item in items {
        let Some(url) = text_field(item, "url") else { continue };
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            continue;
        }
        if !seen.contains(&url) {
            seen.push(url);
        }
    }
    seen
}

// ---------------------------------------------------------------------------
// Gemini in Workspace
// ---------------------------------------------------------------------------

/// `conversation_<id>.txt` — JSON despite the extension, one file per conversation.
///
/// ```text
/// { "title": …, "creation_time": …, "last_modification_time": …,
///   "conversation_turns": [ {"user_turn":   {"prompt": …, "turn_index": …, "turn_last_modified": …}}
///                         | {"system_turn": {"text": [{"data": …, "preamble": …}], …}} ] }
/// ```
///
/// Only those two turn kinds exist across all 32 turns of the reference export.
fn workspace(logical_path: &str, bytes: &[u8]) -> Option<Conversation> {
    let root: Value = serde_json::from_slice(bytes).ok()?;
    let turns = field(&root, "conversation_turns")?.as_array()?;

    // The file carries no id of its own, so the filename stem is the key. It is stable
    // across re-exports — the same conversation lands in the same filename — which is what
    // an id has to be. `conversation_1775576272.txt` -> `1775576272`.
    let file = logical_path.rsplit('/').next().unwrap_or(logical_path);
    let native_id = file
        .strip_suffix(".txt")
        .unwrap_or(file)
        .strip_prefix("conversation_")
        .unwrap_or(file)
        .to_string();
    if native_id.is_empty() {
        return None;
    }

    let mut messages = Vec::new();
    let mut first_user = None;
    let mut prev: Option<String> = None;
    let mut seq = 0i64;

    for turn in turns {
        let (role, text, stamp) = if let Some(u) = field(turn, "user_turn") {
            (Role::User, text_field(u, "prompt").unwrap_or_default(), turn_time(u))
        } else if let Some(s) = field(turn, "system_turn") {
            (Role::Assistant, system_text(s), turn_time(s))
        } else {
            // An unrecognised turn kind is skipped without moving `prev`, so the next real
            // turn re-parents onto the last surviving one rather than onto a message that
            // was never emitted.
            continue;
        };

        let text = text.trim();
        if text.is_empty() {
            continue;
        }
        if role == Role::User && first_user.is_none() {
            first_user = non_empty(clamp(text));
        }

        let id = format!("turn-{seq:04}");
        messages.push(message(
            &id,
            prev.replace(id.clone()),
            logical_path,
            seq,
            role,
            stamp,
            text.to_string(),
        ));
        seq += 1;
    }

    if messages.is_empty() {
        return None;
    }

    Some(Conversation {
        source: SOURCE.to_string(),
        native_id,
        // Google writes this title; the user never typed it, so it is `generated` rather
        // than `custom` and an opening prompt does not outrank it (ADR 8).
        titles: Titles {
            custom: None,
            generated: text_field(&root, "title").and_then(|t| non_empty(clamp(&t))),
            first_user,
        },
        cwd: None,
        git_branch: None,
        model: None,
        surface: Some("gemini-workspace".to_string()),
        forked_from_native_id: None,
        head_native_id: None,
        messages,
    })
}

/// A system turn's `text` is an array of `{data, preamble}`. The preamble is the sentence
/// the sidebar showed above the result ("The text now matches your document's writing
/// style."); it is part of what was said and is kept ahead of the data it introduces.
fn system_text(turn: &Value) -> String {
    let Some(items) = field(turn, "text").and_then(Value::as_array) else {
        return String::new();
    };
    let mut parts = Vec::new();
    for item in items {
        for key in ["preamble", "data"] {
            if let Some(v) = text_field(item, key) {
                let v = v.trim().to_string();
                if !v.is_empty() {
                    parts.push(v);
                }
            }
        }
    }
    parts.join("\n")
}

fn turn_time(turn: &Value) -> Option<i64> {
    text_field(turn, "turn_last_modified").as_deref().and_then(epoch_millis)
}

// ---------------------------------------------------------------------------
// NotebookLM
// ---------------------------------------------------------------------------

/// `<notebook>/Chat History/Chat Session - <uuid>.html`.
///
/// Not really an HTML document — a sequence of blank-line-separated blocks, each prefixed
/// `USER: ` or `MODEL: `, whose body is an HTML fragment.
///
/// The notebook's own `metadata.json` sits beside this file and holds a nicer title, but an
/// importer is a pure function over `(logical_path, bytes)` with no filesystem access
/// (ADR 1), so it cannot be read from here. The containing directory name is the same string
/// in practice — Takeout names the folder after the notebook — and a title may be derived
/// from a path even though an id may not.
fn notebook_lm(logical_path: &str, bytes: &[u8]) -> Option<Conversation> {
    let text = std::str::from_utf8(bytes).ok()?;
    let file = logical_path.rsplit('/').next().unwrap_or(logical_path);

    // `Chat Session - <uuid>.html` -> `<uuid>`, a real stable id rather than a coined one.
    let stem = file.strip_suffix(".html").or_else(|| file.strip_suffix(".HTML")).unwrap_or(file);
    let native_id = stem.rsplit(" - ").next().unwrap_or(stem).trim().to_string();
    if native_id.is_empty() {
        return None;
    }

    let notebook = logical_path
        .split('/')
        .rev()
        .nth(2)
        .filter(|s| !s.is_empty())
        .map(str::to_string);

    let mut messages = Vec::new();
    let mut first_user = None;
    let mut prev: Option<String> = None;
    let mut seq = 0i64;

    for (role, body) in blocks(text) {
        let body = to_text(&body);
        let body = body.trim();
        if body.is_empty() {
            continue;
        }
        if role == Role::User && first_user.is_none() {
            first_user = non_empty(clamp(body));
        }
        let id = format!("turn-{seq:04}");
        messages.push(message(
            &id,
            prev.replace(id.clone()),
            logical_path,
            seq,
            role,
            // No per-turn timestamp exists in this format, and the notebook's `lastViewed`
            // is in a file this importer cannot open. An absent timestamp is honest; a
            // synthesised one would sort these into a week they did not happen in.
            None,
            body.to_string(),
        ));
        seq += 1;
    }

    if messages.is_empty() {
        return None;
    }

    Some(Conversation {
        source: SOURCE.to_string(),
        native_id,
        titles: Titles {
            custom: None,
            generated: notebook.and_then(|n| non_empty(clamp(&n))),
            first_user,
        },
        cwd: None,
        git_branch: None,
        model: None,
        surface: Some("notebooklm".to_string()),
        forked_from_native_id: None,
        head_native_id: None,
        messages,
    })
}

/// Split a chat session into `(role, html)` blocks on the `USER: ` / `MODEL: ` prefixes.
///
/// Scanning for the markers at line starts rather than splitting on blank lines, because a
/// response contains blank lines of its own between its paragraphs.
fn blocks(text: &str) -> Vec<(Role, String)> {
    let mut out: Vec<(Role, String)> = Vec::new();
    for line in text.lines() {
        let opened = line
            .strip_prefix("USER:")
            .map(|rest| (Role::User, rest))
            .or_else(|| line.strip_prefix("MODEL:").map(|rest| (Role::Assistant, rest)));

        match opened {
            Some((role, rest)) => out.push((role, rest.trim_start().to_string())),
            None => {
                // Continuation of the block above. Text before the first marker is preamble
                // that belongs to no turn, and is dropped.
                if let Some((_, body)) = out.last_mut() {
                    body.push('\n');
                    body.push_str(line);
                }
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// shared
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn message(
    native_id: &str,
    parent_native_id: Option<String>,
    thread_key: &str,
    seq: i64,
    role: Role,
    ts: Option<i64>,
    text: String,
) -> Message {
    Message {
        native_id: native_id.to_string(),
        parent_native_id,
        // The logical path, carried in rather than parsed back out of anything (ADR 4).
        thread_key: thread_key.to_string(),
        // None of these formats has a subagent concept, and none records tool traffic:
        // an activity log stores what was said, not how it was produced.
        is_sidechain: false,
        is_error: false,
        seq,
        role,
        kind: Kind::Prose,
        ts,
        text,
    }
}

/// Append a bracketed trailer, keeping it on its own line and out of the prose above it.
fn join_block(body: &str, trailer: &str) -> String {
    if body.is_empty() {
        return trailer.to_string();
    }
    format!("{body}\n\n{trailer}")
}

/// `"Gemini Apps"` -> `"gemini-apps"`.
fn slug(s: &str) -> String {
    s.split_whitespace().map(str::to_lowercase).collect::<Vec<_>>().join("-")
}

/// Whitespace-collapsed and clamped. Titles sit in a list; a newline in one would move every
/// row below it.
fn clamp(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(TITLE_MAX_CHARS).collect()
}

fn non_empty(s: String) -> Option<String> {
    (!s.is_empty()).then_some(s)
}

fn string_list(v: &Value, key: &str) -> Vec<String> {
    field(v, key)
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
        .unwrap_or_default()
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

#[cfg(test)]
mod tests {
    use super::*;

    // Fixtures are synthetic, shaped after the real export. Real chats carry secrets and a
    // golden file cut from one cannot be shared or reviewed (chat-search-4ar.3).

    const ACTIVITY_PATH: &str = "Takeout-gemini/My Activity/Gemini Apps/MyActivity.json";

    fn activity(records: &str) -> Vec<Conversation> {
        import_all(ACTIVITY_PATH, records.as_bytes())
    }

    #[test]
    fn an_activity_record_becomes_a_two_message_conversation_keyed_on_its_own_timestamp() {
        let c = activity(
            r#"[{
              "header": "Gemini Apps",
              "title": "Prompted Should I CC the interviewers?",
              "time": "2026-08-01T17:48:13.908Z",
              "products": ["Gemini Apps"],
              "safeHtmlItem": [{"html": "<p>Yes, <strong>absolutely</strong>.</p>"}]
            }]"#,
        );
        assert_eq!(c.len(), 1);
        let c = &c[0];
        // The id is the record's own time: not the file it came from, not its position in
        // that file, and not anything we computed. That is what survives a re-export.
        assert_eq!(c.native_id, "2026-08-01T17:48:13.908Z");
        assert_eq!(c.id(), "google-takeout:2026-08-01T17:48:13.908Z");
        assert_eq!(c.surface.as_deref(), Some("gemini-apps"));
        assert_eq!(c.titles.resolve(), Some("Should I CC the interviewers?"));

        assert_eq!(c.messages.len(), 2);
        assert_eq!(c.messages[0].role, Role::User);
        assert_eq!(c.messages[0].text, "Should I CC the interviewers?");
        assert_eq!(c.messages[1].role, Role::Assistant);
        assert_eq!(c.messages[1].text, "Yes, absolutely.");
        // Both turns happened at the one instant the record records.
        assert_eq!(c.messages[0].ts, c.messages[1].ts);
        assert_eq!(c.messages[1].parent_native_id.as_deref(), Some("prompt"));
    }

    #[test]
    fn the_same_record_gets_the_same_id_from_a_different_export_directory() {
        // The point of keying on the record rather than the path: a second Takeout unpacks
        // somewhere new, and every conversation it shares with the first has to land on the
        // same id or the corpus doubles.
        let record = r#"[{"header":"Gemini Apps","title":"Prompted hello",
                          "time":"2026-08-01T17:48:13.908Z",
                          "safeHtmlItem":[{"html":"<p>hi</p>"}]}]"#;
        let first = import_all(ACTIVITY_PATH, record.as_bytes());
        let second = import_all(
            "takeout-20261115T090000Z/My Activity/Gemini Apps/MyActivity.json",
            record.as_bytes(),
        );
        assert_eq!(first[0].id(), second[0].id());
    }

    #[test]
    fn ai_mode_is_the_same_format_under_a_different_surface() {
        let c = import_all(
            "Takeout-gemini/My Activity/AI Mode/MyActivity.json",
            br#"[{"header":"AI Mode","title":"Searched for do you tip in oregon",
                 "time":"2026-08-02T01:37:21.314Z",
                 "safeHtmlItem":[{"html":"<p>No.</p>"}]}]"#,
        );
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].surface.as_deref(), Some("ai-mode"));
        // "Searched for" is not a prompt verb, so the title is kept whole rather than
        // having a prefix guessed off it.
        assert_eq!(c[0].messages[0].text, "Searched for do you tip in oregon");
    }

    #[test]
    fn a_record_with_no_prompt_and_no_response_contributes_nothing() {
        // 15 records in the reference export are a bare "Used Gemini Apps".
        let c = activity(
            r#"[{"header":"Gemini Apps","title":"Used Gemini Apps","time":"2026-07-13T00:41:52.627Z"},
                {"header":"Gemini Apps","title":"Prompted real one","time":"2026-07-13T00:42:00.000Z"}]"#,
        );
        // The second survives on its prompt alone even though it never got a response.
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].messages.len(), 1);
        assert_eq!(c[0].messages[0].role, Role::User);
    }

    #[test]
    fn bookkeeping_titles_are_told_apart_from_titles_that_carry_content() {
        // Kept: a prompt, a search, and a canvas someone can actually name.
        assert_eq!(spoken_text("Prompted why is the sky blue"), Some("why is the sky blue".into()));
        assert_eq!(spoken_text("Searched for mont ventoux"), Some("Searched for mont ventoux".into()));
        assert_eq!(
            spoken_text("Created Gemini Canvas titled Anthropic Application Draft"),
            Some("Created Gemini Canvas titled Anthropic Application Draft".into())
        );
        // Dropped: nothing here is searchable, and all of it arrives without a response.
        assert_eq!(spoken_text("Used Gemini Apps"), None);
        assert_eq!(spoken_text("Used AI Mode"), None);
        assert_eq!(spoken_text("Gave feedback: Bad response (Unspecified)"), None);
        assert_eq!(spoken_text("Created Gemini Canvas titled "), None);
        assert_eq!(
            spoken_text("Created Gemini Canvas titled http://googleusercontent.com/map_content/0"),
            None
        );
        // A prompt that merely starts with a dropped word is still a prompt.
        assert_eq!(
            spoken_text("Prompted Used to work at Google, what changed?"),
            Some("Used to work at Google, what changed?".into())
        );
    }

    #[test]
    fn a_response_without_a_prompt_is_rooted_rather_than_pointing_at_a_message_that_was_skipped() {
        let c = activity(
            r#"[{"header":"Gemini Apps","title":"Used Gemini Apps","time":"2026-07-13T00:41:52.627Z",
                 "safeHtmlItem":[{"html":"<p>orphan answer</p>"}]}]"#,
        );
        assert_eq!(c[0].messages.len(), 1);
        // A dangling parent silently terminates `index::on_head_path`'s walk.
        assert_eq!(c[0].messages[0].parent_native_id, None);
    }

    #[test]
    fn attachments_are_recorded_on_the_prompt_and_cited_urls_on_the_response() {
        let c = activity(
            r#"[{
              "header": "Gemini Apps",
              "title": "Prompted summarise this",
              "time": "2026-07-26T17:09:35.000Z",
              "attachedFiles": ["xen-paper.pdf", "diagram.png"],
              "imageFile": "diagram.png",
              "subtitles": [
                {"name":"https://docs.google.com/document/d/abc","url":"https://docs.google.com/document/d/abc"},
                {"name":"https://docs.google.com/document/d/abc","url":"https://docs.google.com/document/d/abc"},
                {"name":"-  2026-07-26_17-09-35.jpg","url":"2026-07-26_17-09-35-22fe64d5ab710e64.jpg"}
              ],
              "safeHtmlItem": [{"html": "<p>It virtualises.</p>"}]
            }]"#,
        );
        let c = &c[0];
        assert_eq!(c.messages[0].text, "summarise this\n\n[attached: xen-paper.pdf, diagram.png]");
        // The local-filename subtitle is dropped as an attachment display name, and the
        // repeated citation appears once.
        assert_eq!(
            c.messages[1].text,
            "It virtualises.\n\n[sources: https://docs.google.com/document/d/abc]"
        );
    }

    #[test]
    fn a_multi_chunk_response_is_joined_rather_than_truncated_to_its_first_part() {
        let c = activity(
            r#"[{"header":"Gemini Apps","title":"Prompted q","time":"2026-07-13T00:41:52.627Z",
                 "safeHtmlItem":[{"html":"<p>one</p>"},{"html":"<p>two</p>"},{"html":"<p>three</p>"}]}]"#,
        );
        assert_eq!(c[0].messages[1].text, "one\ntwo\nthree");
    }

    #[test]
    fn a_file_that_is_not_a_json_array_yields_nothing_instead_of_panicking() {
        assert!(activity("not json at all").is_empty());
        assert!(activity(r#"{"records": []}"#).is_empty());
        assert!(activity("[]").is_empty());
    }

    // -- Gemini in Workspace --------------------------------------------------

    const WORKSPACE_PATH: &str =
        "Takeout-html/Gemini in Workspace/Conversation History/conversation_1775576272.txt";

    const WORKSPACE: &str = r#"{
      "title": "Match writing style",
      "creation_time": "2026-04-07T15:37:52.177503+00:00",
      "last_modification_time": "2026-04-07T16:04:00.170847+00:00",
      "conversation_turns": [
        {"user_turn": {"prompt": "Match writing style", "turn_index": 0,
                       "turn_last_modified": "2026-04-07T15:37:52.177503+00:00"}},
        {"system_turn": {"text": [{"data": "Writing style: Technical, structured tone.",
                                   "preamble": "The text now matches your document's writing style."}],
                         "turn_index": 1,
                         "turn_last_modified": "2026-04-07T15:37:52.177503+00:00"}}
      ]
    }"#;

    #[test]
    fn a_workspace_conversation_is_keyed_on_its_filename_stem_and_chains_its_turns() {
        let c = import_all(WORKSPACE_PATH, WORKSPACE.as_bytes());
        assert_eq!(c.len(), 1);
        let c = &c[0];
        assert_eq!(c.native_id, "1775576272");
        assert_eq!(c.surface.as_deref(), Some("gemini-workspace"));
        assert_eq!(c.messages.len(), 2);
        assert_eq!(c.messages[0].parent_native_id, None);
        assert_eq!(c.messages[1].parent_native_id.as_deref(), Some("turn-0000"));
        // The preamble is what the sidebar actually showed, so it leads the data.
        assert_eq!(
            c.messages[1].text,
            "The text now matches your document's writing style.\nWriting style: Technical, structured tone."
        );
        assert_eq!(c.messages[0].ts, epoch_millis("2026-04-07T15:37:52.177503+00:00"));
    }

    #[test]
    fn googles_workspace_title_is_generated_and_so_outranks_the_opening_prompt() {
        let c = import_all(WORKSPACE_PATH, WORKSPACE.as_bytes());
        assert_eq!(c[0].titles.generated.as_deref(), Some("Match writing style"));
        assert!(c[0].titles.custom.is_none(), "the user never typed this title");
    }

    #[test]
    fn an_unknown_turn_kind_is_skipped_without_orphaning_the_turn_after_it() {
        let c = import_all(
            WORKSPACE_PATH,
            br#"{"conversation_turns":[
                 {"user_turn":{"prompt":"first"}},
                 {"mystery_turn":{"prompt":"???"}},
                 {"user_turn":{"prompt":"second"}}]}"#,
        );
        assert_eq!(c[0].messages.len(), 2);
        // Re-parented onto the surviving turn, not onto the one that was dropped.
        assert_eq!(c[0].messages[1].parent_native_id.as_deref(), Some("turn-0000"));
    }

    // -- NotebookLM -----------------------------------------------------------

    const NOTEBOOK_PATH: &str =
        "Takeout-html/NotebookLM/Graduate Algorithms NP/Chat History/Chat Session - 2ddf3f0b-d173-40ad-8bed-0e66402d9117.html";

    #[test]
    fn a_notebooklm_session_takes_its_id_from_the_uuid_and_its_title_from_the_notebook() {
        let c = import_all(
            NOTEBOOK_PATH,
            b"MODEL: <p>Boolean choices assign truth values [1].</p>\n\nUSER: <p>Discuss SAT.</p>\n\nMODEL: <p>Independent Set is <strong>pairwise non-conflict</strong>.</p>\n",
        );
        assert_eq!(c.len(), 1);
        let c = &c[0];
        assert_eq!(c.native_id, "2ddf3f0b-d173-40ad-8bed-0e66402d9117");
        assert_eq!(c.surface.as_deref(), Some("notebooklm"));
        assert_eq!(c.titles.resolve(), Some("Graduate Algorithms NP"));
        assert_eq!(c.messages.len(), 3);
        assert_eq!(c.messages[0].role, Role::Assistant);
        assert_eq!(c.messages[1].text, "Discuss SAT.");
        assert_eq!(c.messages[2].text, "Independent Set is pairwise non-conflict.");
        // This format records no time anywhere, and inventing one would misdate it.
        assert!(c.messages.iter().all(|m| m.ts.is_none()));
    }

    #[test]
    fn a_response_spanning_blank_lines_stays_one_message() {
        // Splitting on blank lines would shatter every multi-paragraph answer.
        let c = import_all(
            NOTEBOOK_PATH,
            b"USER: <p>Explain.</p>\n\nMODEL: <p>First para.</p>\n\n<p>Second para.</p>\n\nUSER: <p>Thanks.</p>\n",
        );
        assert_eq!(c[0].messages.len(), 3);
        assert_eq!(c[0].messages[1].text, "First para.\nSecond para.");
    }

    // -- dispatch -------------------------------------------------------------

    #[test]
    fn a_file_the_globs_let_through_but_this_module_does_not_know_is_declined() {
        // The include globs also reach notebook metadata and source documents.
        assert!(import_all("Takeout-html/NotebookLM/ML A2/ML A2 metadata.json", b"{}").is_empty());
        assert!(import_all("Takeout-html/Gemini/gemini_gems_data.html", b"<div></div>").is_empty());
        assert!(import_all("Takeout-html/NotebookLM/ML A2/Sources/paper.md.html", b"<p>x</p>").is_empty());
    }
}
