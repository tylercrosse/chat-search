//! ChatGPT export importer — the `conversations-NNN.json` files in an OpenAI data export.
//!
//! This is the only way to reach ChatGPT history: the desktop app's local cache is
//! encrypted, so the periodic "export your data" mail is the whole source. It is also the
//! largest corpus here (2,011 conversations, 2023-01 → 2026-07) and the one that forces the
//! DAG (ADR 4) — editing a message appends a *sibling* under the same parent rather than
//! overwriting, and `current_node` says which sibling is currently displayed.
//!
//! # Format
//!
//! One file is a JSON **array of conversations** (an export is sharded into ~100
//! conversations per file). Each conversation:
//!
//! ```text
//! {
//!   "id":                 "0096a257-…",   // == conversation_id; the <id> in chatgpt.com/c/<id>
//!   "conversation_id":    "0096a257-…",
//!   "title":              "Looping PostgreSQL queries.",
//!   "create_time":        1680295988.196831,   // float epoch SECONDS, not millis
//!   "update_time":        1680296329.0,
//!   "current_node":       "74087b17-…",        // the displayed leaf — our head
//!   "default_model_slug": "gpt-4o" | null,
//!   "mapping":            { "<node id>": <node>, … },
//!   … is_archived, is_starred, moderation_results, gizmo_type, voice, …
//! }
//! ```
//!
//! `mapping` is the DAG, keyed by node id. Every node is:
//!
//! ```text
//! { "id": "<node id>", "parent": "<node id>" | null, "children": ["<node id>", …],
//!   "message": <message> | null }
//! ```
//!
//! Observed invariants (checked over all 17,512 nodes of the real export):
//!
//! - exactly **one root per conversation**, and it is the one node with `message: null` —
//!   pure scaffolding, so it is skipped and its children become message roots;
//! - `parent`/`children` are mutually consistent; no dangling edges, no orphans;
//! - `message.id` always equals the mapping key, so node ids are already unique within the
//!   conversation and we never invent one (ADR 2);
//! - `current_node` is always present and always in the mapping;
//! - `message.metadata.parent_id` **disagrees with the node's `parent` 4,948 times** — it is
//!   a client-side breadcrumb, not the edge. Always use the node's `parent`.
//!
//! A message node:
//!
//! ```text
//! { "id": …, "author": {"role": "user"|"assistant"|"tool"|"system", "name": …},
//!   "create_time": 1680296329.203718, "update_time": null, "status": …, "weight": 1.0,
//!   "recipient": "all",            // a tool/plugin name means the assistant is calling it
//!   "content": { "content_type": …, … },
//!   "metadata": { "model_slug": …, "is_visually_hidden_from_conversation": …, … } }
//! ```
//!
//! Content shapes seen in the corpus, and what each maps to:
//!
//! | `content_type`     | payload                              | kind          |
//! | ------------------ | ------------------------------------ | ------------- |
//! | `text`             | `parts: [string]`                    | `Prose`       |
//! | `multimodal_text`  | `parts: [string \| {content_type,…}]` | `Prose`       |
//! | `thoughts`         | `thoughts: [{summary, content, …}]`  | `Reasoning`   |
//! | `reasoning_recap`  | `content: string`                    | `Reasoning`   |
//! | `code`             | `text: string`                       | `ToolCall`    |
//! | `execution_output` | `text: string`                       | `ToolResult`  |
//! | `tether_*`         | `text` / `result`                    | `ToolResult`  |
//!
//! The last three do not occur in this export — 2,011 conversations are pure user/assistant
//! prose plus reasoning — but they are cheap to carry and appear in exports that used Code
//! Interpreter or plugins. Anything unrecognised falls back to whatever string-ish field it
//! has and, failing that, is skipped: never a panic.
//!
//! # Branch shapes
//!
//! 287 nodes have more than one child, across 234 of the 2,011 conversations. Walking
//! `current_node` up to the root leaves **869 of the 15,501 message nodes off the head
//! path** (the figure in ADR 4) — every one of them a message a flat model would have to
//! drop or overwrite. Sibling order in `children` is *not* always chronological (23 branch
//! points are out of order), so `seq` follows the export's own `children` order rather than
//! time.

use cs_core::model::{model_name, Conversation, Kind, Message, Role, Titles};
use serde_json::Value;
use std::collections::{HashMap, HashSet};

pub const SOURCE: &str = "chatgpt-export";

/// ChatGPT has no subagents: one conversation is one thread (ADR 4).
const THREAD_KEY: &str = "main";

/// Characters of the opening user message kept as the last-resort title (ADR 8).
const FIRST_USER_TITLE_CHARS: usize = 80;

/// Content types that are the *output* of a tool rather than a call to one.
const TOOL_RESULT_CONTENT: &[&str] = &[
    "execution_output",
    "system_error",
    "tether_quote",
    "tether_browsing_display",
    "tether_browsing_code",
];

/// Every conversation in one export file.
///
/// Total function: a file that is not JSON, or not shaped like an export, yields an empty
/// `Vec` rather than an error. An importer that panics on one bad shard takes the whole
/// rebuild with it, and the archive keeps the bytes either way (ADR 1).
pub fn import_all(bytes: &[u8]) -> Vec<Conversation> {
    let Ok(root) = serde_json::from_slice::<Value>(bytes) else {
        return Vec::new();
    };
    conversations_in(&root).into_iter().filter_map(import_conversation).collect()
}

/// The export ships a bare array; accept the two other wrappers seen in the wild (a
/// `{"conversations": […]}` envelope, and a single conversation object) for free.
fn conversations_in(root: &Value) -> Vec<&Value> {
    match root {
        Value::Array(a) => a.iter().collect(),
        Value::Object(o) => match o.get("conversations") {
            Some(Value::Array(a)) => a.iter().collect(),
            _ if o.contains_key("mapping") => vec![root],
            _ => Vec::new(),
        },
        _ => Vec::new(),
    }
}

fn import_conversation(raw: &Value) -> Option<Conversation> {
    let native_id = non_empty_str(raw.get("id"))
        .or_else(|| non_empty_str(raw.get("conversation_id")))?
        .to_string();
    let mapping = raw.get("mapping")?.as_object()?;

    let mut messages: Vec<Message> = Vec::new();
    // Nearest *emitted* message at or above each node. Skipped nodes (the structural root,
    // hidden scaffolding, empty text) are transparent: a child re-parents onto its nearest
    // surviving ancestor, so the parent chain stays walkable and `on_head_path` in the
    // indexer cannot terminate early on a node that was never inserted.
    let mut anchor: HashMap<&str, Option<&str>> = HashMap::new();
    let mut visited: HashSet<&str> = HashSet::new();

    for start in walk_order(mapping) {
        if visited.contains(start) {
            continue;
        }
        // A node reached out of order — only possible in a malformed mapping — still gets
        // the best parent we know of.
        let seed = parent_of(mapping.get(start)).and_then(|p| anchor.get(p).copied()).flatten();
        let mut stack = vec![(start, seed)];

        while let Some((id, inherited)) = stack.pop() {
            // Doubles as cycle protection: a mapping that loops terminates here.
            if !visited.insert(id) {
                continue;
            }
            let Some(node) = mapping.get(id) else { continue };
            let mut own = inherited;

            if let Some(msg) = node.get("message").filter(|m| m.is_object()) {
                if let Some((role, kind)) = classify(msg) {
                    let text = message_text(msg);
                    let text = text.trim();
                    if !text.is_empty() {
                        // What answered this node, kept on it. The mapping is walked in id
                        // order rather than in time order, so collapsing here would not even
                        // have produced "the first model" — it would have produced whichever
                        // one sorted first (ADR 24).
                        let model = non_empty_str(
                            msg.get("metadata").and_then(|m| m.get("model_slug")),
                        )
                        .and_then(model_name)
                        .filter(|_| role == Role::Assistant)
                        .map(String::from);
                        messages.push(Message {
                            native_id: id.to_string(),
                            parent_native_id: inherited.map(String::from),
                            thread_key: THREAD_KEY.to_string(),
                            is_sidechain: false,
                            // No failure signal in this format; false rather than guessed from the text.
                            is_error: false,
                            seq: messages.len() as i64,
                            role,
                            kind,
                            ts: epoch_millis(msg),
                            model,
                            text: text.to_string(),
                        });
                        own = Some(id);
                    }
                }
            }

            anchor.insert(id, own);
            for child in children_of(node).into_iter().rev() {
                stack.push((child, own));
            }
        }
    }

    // ChatGPT is the one source that knows its own head. If `current_node` names a node we
    // skipped — 73 conversations end on an empty or contentless node — fall back to the
    // nearest ancestor that survived, which is what the UI actually shows.
    let head_native_id = raw
        .get("current_node")
        .and_then(Value::as_str)
        .and_then(|c| anchor.get(c).copied())
        .flatten()
        .map(String::from);

    let titles = Titles {
        // The export has a single `title` field and does not record whether it was
        // auto-generated or renamed by hand. It is user-visible and user-editable, so it is
        // treated as `custom` — the top of the fold (ADR 8).
        custom: non_empty_str(raw.get("title")).map(String::from),
        generated: None,
        first_user: messages
            .iter()
            .find(|m| m.role == Role::User && m.kind == Kind::Prose)
            .map(|m| summarise(&m.text, FIRST_USER_TITLE_CHARS)),
    };

    Some(Conversation {
        source: SOURCE.to_string(),
        native_id: native_id.clone(),
        titles,
        cwd: None,
        git_branch: None,
        // Demoted to a fallback, and it had no business outranking the messages: it is the
        // *picker*, not the pick. 154 conversations carry a `default_model_slug` that no
        // message ever names, 134 of them the literal `auto` — which `model_name` now
        // rejects outright, taking those 134 mislabelled conversations to 5 (ADR 24).
        declared_model: non_empty_str(raw.get("default_model_slug"))
            .and_then(model_name)
            .map(String::from),
        surface: None,
        forked_from_native_id: None,
        head_native_id,
        messages,
    })
}

/// Roots first, then every remaining node, both sorted by id.
///
/// Sorting makes the walk independent of how `serde_json` happens to store an object, so
/// `seq` is reproducible across runs and builds (ADR 7). In practice there is exactly one
/// root and the tail visits nothing new; the tail exists so that a cycle or a dangling
/// `parent` costs ordering rather than data.
fn walk_order<'a>(mapping: &'a serde_json::Map<String, Value>) -> Vec<&'a str> {
    let mut roots: Vec<&str> = mapping
        .iter()
        .filter(|(_, node)| match parent_of(Some(node)) {
            Some(p) => !mapping.contains_key(p),
            None => true,
        })
        .map(|(id, _)| id.as_str())
        .collect();
    roots.sort_unstable();

    let mut rest: Vec<&str> = mapping.keys().map(String::as_str).collect();
    rest.sort_unstable();

    roots.into_iter().chain(rest).collect()
}

fn parent_of(node: Option<&Value>) -> Option<&str> {
    node?.get("parent").and_then(Value::as_str)
}

fn children_of(node: &Value) -> Vec<&str> {
    match node.get("children").and_then(Value::as_array) {
        Some(a) => a.iter().filter_map(Value::as_str).collect(),
        None => Vec::new(),
    }
}

/// Who sent it and what it is — or `None` for a node that is not content.
fn classify(msg: &Value) -> Option<(Role, Kind)> {
    let role_str = msg.get("author").and_then(|a| a.get("role")).and_then(Value::as_str)?;
    let content_type = content_type(msg);
    let recipient = msg.get("recipient").and_then(Value::as_str).unwrap_or("all");
    let is_reasoning = matches!(content_type, "thoughts" | "reasoning_recap");

    // System nodes and nodes the UI hides are scaffolding — custom instructions,
    // model-set-context, safety preambles. Reasoning is the exception: it is often flagged
    // hidden and it is real content.
    if !is_reasoning && (role_str == "system" || is_hidden(msg)) {
        return None;
    }

    let role = match role_str {
        "user" => Role::User,
        "assistant" => Role::Assistant,
        "tool" => Role::Tool,
        "system" => Role::System,
        _ => return None,
    };

    let kind = if is_reasoning {
        Kind::Reasoning
    } else if role == Role::Tool || TOOL_RESULT_CONTENT.contains(&content_type) {
        Kind::ToolResult
    } else if role == Role::Assistant && (content_type == "code" || recipient != "all") {
        // The assistant addressing anyone but `all` is a call to a tool or plugin.
        Kind::ToolCall
    } else {
        Kind::Prose
    };

    Some((role, kind))
}

fn content_type(msg: &Value) -> &str {
    msg.get("content")
        .and_then(|c| c.get("content_type"))
        .and_then(Value::as_str)
        .unwrap_or_default()
}

fn is_hidden(msg: &Value) -> bool {
    msg.get("metadata")
        .and_then(|m| m.get("is_visually_hidden_from_conversation"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

/// Flatten whatever this content type keeps its text in. Unknown shapes yield `""`, which
/// the caller drops.
fn message_text(msg: &Value) -> String {
    let Some(content) = msg.get("content") else { return String::new() };

    match content_type(msg) {
        "thoughts" => thoughts_text(content),
        "reasoning_recap" => {
            content.get("content").and_then(Value::as_str).unwrap_or_default().to_string()
        }
        _ => {
            let mut out: Vec<&str> = content
                .get("parts")
                .and_then(Value::as_array)
                .map(|parts| parts.iter().filter_map(part_text).collect())
                .unwrap_or_default();

            // `code`, `execution_output`, `tether_*` and anything new carry a bare string
            // instead of `parts`.
            if out.is_empty() {
                out = ["text", "result", "content"]
                    .iter()
                    .filter_map(|k| content.get(k).and_then(Value::as_str))
                    .collect();
            }
            join(out)
        }
    }
}

/// A part is either a bare string or a block. Image and audio pointers have no text and
/// drop out; an `audio_transcription` block contributes its `text`.
fn part_text(part: &Value) -> Option<&str> {
    match part {
        Value::String(s) => Some(s.as_str()),
        _ => part.get("text").and_then(Value::as_str),
    }
}

/// Reasoning arrives as `[{summary, content, chunks, finished}]`. `summary` is the visible
/// "Thinking about…" line and `content` the body; `chunks` are streaming fragments that
/// overlap `content`, so they are only a fallback.
fn thoughts_text(content: &Value) -> String {
    let Some(thoughts) = content.get("thoughts").and_then(Value::as_array) else {
        return String::new();
    };
    let mut out: Vec<&str> = Vec::new();
    for thought in thoughts {
        let summary = thought.get("summary").and_then(Value::as_str).unwrap_or_default();
        let body = thought.get("content").and_then(Value::as_str).unwrap_or_default();
        if summary.trim().is_empty() && body.trim().is_empty() {
            if let Some(chunks) = thought.get("chunks").and_then(Value::as_array) {
                out.extend(chunks.iter().filter_map(Value::as_str));
            }
            continue;
        }
        if summary != body {
            out.push(summary);
        }
        out.push(body);
    }
    join(out)
}

fn join(parts: Vec<&str>) -> String {
    parts.into_iter().filter(|p| !p.trim().is_empty()).collect::<Vec<_>>().join("\n")
}

/// `create_time` is float epoch **seconds** in this format; the model stores millis.
fn epoch_millis(msg: &Value) -> Option<i64> {
    let secs = msg
        .get("create_time")
        .and_then(Value::as_f64)
        .or_else(|| msg.get("update_time").and_then(Value::as_f64))?;
    let millis = (secs * 1000.0).round();
    // A NaN or an absurd year is a corrupt field, not a timestamp.
    (millis.is_finite() && millis.abs() < 9e15).then_some(millis as i64)
}

fn non_empty_str(v: Option<&Value>) -> Option<&str> {
    v.and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty())
}

/// One line, at most `max` characters, for the fallback title.
fn summarise(text: &str, max: usize) -> String {
    let one_line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut out: String = one_line.chars().take(max).collect();
    if one_line.chars().count() > max {
        out.push('…');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // Every fixture here is synthetic. The shapes are copied from the real export; none of
    // the text is.

    fn node(
        id: &str,
        parent: Option<&str>,
        children: &[&str],
        author: Value,
        content: Value,
        t: f64,
    ) -> Value {
        json!({
            "id": id,
            "parent": parent,
            "children": children,
            "message": {
                "id": id,
                "author": author,
                "create_time": t,
                "update_time": null,
                "content": content,
                "status": "finished_successfully",
                "end_turn": true,
                "weight": 1.0,
                "metadata": {"model_slug": "gpt-5", "request_id": "req-1"},
                "recipient": "all",
                "channel": null,
            },
        })
    }

    fn text_node(role: &str, id: &str, parent: Option<&str>, kids: &[&str], s: &str, t: f64) -> Value {
        node(
            id,
            parent,
            kids,
            json!({"role": role, "name": null, "metadata": {}}),
            json!({"content_type": "text", "parts": [s]}),
            t,
        )
    }

    fn user(id: &str, parent: Option<&str>, kids: &[&str], s: &str, t: f64) -> Value {
        text_node("user", id, parent, kids, s, t)
    }

    fn assistant(id: &str, parent: Option<&str>, kids: &[&str], s: &str, t: f64) -> Value {
        text_node("assistant", id, parent, kids, s, t)
    }

    /// The structural root: one per conversation, `message: null`.
    fn root(id: &str, kids: &[&str]) -> Value {
        json!({"id": id, "parent": null, "children": kids, "message": null})
    }

    fn conversation(id: &str, title: Value, current: &str, nodes: Vec<Value>) -> Value {
        let mut mapping = serde_json::Map::new();
        for n in nodes {
            mapping.insert(n["id"].as_str().unwrap().to_string(), n);
        }
        json!({
            "id": id,
            "conversation_id": id,
            "title": title,
            "create_time": 1_680_295_988.196_831_f64,
            "update_time": 1_680_296_329.0_f64,
            "mapping": Value::Object(mapping),
            "current_node": current,
            "default_model_slug": "gpt-4o",
            "is_archived": false,
            "moderation_results": [],
        })
    }

    fn export(convs: Vec<Value>) -> Vec<u8> {
        serde_json::to_vec(&Value::Array(convs)).unwrap()
    }

    /// root -> u1 -> a1 -> u2 -> a2
    fn linear() -> Vec<u8> {
        export(vec![conversation(
            "conv-1",
            json!("Looping PostgreSQL queries"),
            "a2",
            vec![
                root("root-0", &["u1"]),
                user("u1", Some("root-0"), &["a1"], "how do I loop a query", 1_680_295_988.196_831),
                assistant("a1", Some("u1"), &["u2"], "with a cursor", 1_680_295_990.0),
                user("u2", Some("a1"), &["a2"], "show me", 1_680_296_000.5),
                assistant("a2", Some("u2"), &[], "here you go", 1_680_296_329.203_718),
            ],
        )])
    }

    #[test]
    fn linear_conversation_keeps_order_roles_and_parent_chain() {
        let convs = import_all(&linear());
        assert_eq!(convs.len(), 1);
        let c = &convs[0];

        assert_eq!(c.source, "chatgpt-export");
        assert_eq!(c.native_id, "conv-1");
        assert_eq!(c.id(), "chatgpt-export:conv-1");
        // The url is `destination`'s to build (chat-search-me9.3); what matters here is that the
        // id it routes on is the export's own, reused verbatim rather than reconstructed.
        assert!(!cs_core::destinations(&c.source, &c.native_id).is_empty());

        let ids: Vec<&str> = c.messages.iter().map(|m| m.native_id.as_str()).collect();
        assert_eq!(ids, ["u1", "a1", "u2", "a2"], "node ids are reused verbatim");

        let seqs: Vec<i64> = c.messages.iter().map(|m| m.seq).collect();
        assert_eq!(seqs, [0, 1, 2, 3]);

        let roles: Vec<Role> = c.messages.iter().map(|m| m.role).collect();
        assert_eq!(roles, [Role::User, Role::Assistant, Role::User, Role::Assistant]);
        assert!(c.messages.iter().all(|m| m.kind == Kind::Prose));

        let parents: Vec<Option<&str>> =
            c.messages.iter().map(|m| m.parent_native_id.as_deref()).collect();
        assert_eq!(
            parents,
            [None, Some("u1"), Some("a1"), Some("u2")],
            "the opening message re-parents onto nothing, not onto the structural root"
        );

        assert!(c.messages.iter().all(|m| m.thread_key == "main" && !m.is_sidechain));
        assert_eq!(c.declared_model.as_deref(), Some("gpt-4o"));
    }

    #[test]
    fn seq_is_stable_across_runs() {
        let key = |c: &[Conversation]| {
            c[0].messages.iter().map(|m| (m.seq, m.native_id.clone())).collect::<Vec<_>>()
        };
        assert_eq!(key(&import_all(&linear())), key(&import_all(&linear())));
    }

    /// The case the DAG exists for: `u2` was edited, so `u2b` appears as a *sibling* under
    /// `a1` instead of overwriting it, and each has its own reply.
    ///
    /// ```text
    ///   root-0 -> u1 -> a1 -> u2  -> a2      (superseded)
    ///                     \-> u2b -> a2b     (current_node)
    /// ```
    fn edit_branch() -> Vec<u8> {
        export(vec![conversation(
            "conv-2",
            json!("Edited question"),
            "a2b",
            vec![
                root("root-0", &["u1"]),
                user("u1", Some("root-0"), &["a1"], "first ask", 1.0),
                assistant("a1", Some("u1"), &["u2", "u2b"], "first answer", 2.0),
                user("u2", Some("a1"), &["a2"], "original follow-up", 3.0),
                assistant("a2", Some("u2"), &[], "answer to the original", 4.0),
                user("u2b", Some("a1"), &["a2b"], "edited follow-up", 5.0),
                assistant("a2b", Some("u2b"), &[], "answer to the edit", 6.0),
            ],
        )])
    }

    #[test]
    fn edit_branch_keeps_both_siblings_and_the_head_picks_one() {
        let convs = import_all(&edit_branch());
        let c = &convs[0];

        let ids: Vec<&str> = c.messages.iter().map(|m| m.native_id.as_str()).collect();
        assert_eq!(
            ids,
            ["u1", "a1", "u2", "a2", "u2b", "a2b"],
            "both branches are imported; neither overwrites the other"
        );

        let parent =
            |id: &str| c.messages.iter().find(|m| m.native_id == id).unwrap().parent_native_id.clone();
        assert_eq!(parent("u2").as_deref(), Some("a1"));
        assert_eq!(parent("u2b").as_deref(), Some("a1"), "the siblings share one parent");

        assert_eq!(c.head_native_id.as_deref(), Some("a2b"));
        assert_eq!(c.resolved_head(), Some("a2b"));

        // The abandoned branch is still present and still searchable.
        assert!(c.messages.iter().any(|m| m.text == "answer to the original"));
    }

    #[test]
    fn current_node_becomes_the_head_even_on_the_older_branch() {
        // Proves the head comes from `current_node`, not from message order: `a2` is not
        // the last message by seq.
        let mut export_value: Value = serde_json::from_slice(&edit_branch()).unwrap();
        export_value[0]["current_node"] = json!("a2");
        let convs = import_all(&serde_json::to_vec(&export_value).unwrap());
        assert_eq!(convs[0].head_native_id.as_deref(), Some("a2"));
        assert_eq!(convs[0].messages.last().unwrap().native_id, "a2b");
    }

    #[test]
    fn a_head_on_a_skipped_node_falls_back_to_its_nearest_real_ancestor() {
        // 73 real conversations end on a node with no usable text. The head must still name
        // a message that exists.
        let bytes = export(vec![conversation(
            "conv-3",
            json!("Trailing empty node"),
            "a-empty",
            vec![
                root("root-0", &["u1"]),
                user("u1", Some("root-0"), &["a-empty"], "ask", 1.0),
                assistant("a-empty", Some("u1"), &[], "   ", 2.0),
            ],
        )]);
        let c = &import_all(&bytes)[0];
        assert_eq!(c.messages.len(), 1, "empty text is skipped");
        assert_eq!(c.head_native_id.as_deref(), Some("u1"));
    }

    #[test]
    fn title_becomes_the_custom_title_and_the_opening_message_is_the_fallback() {
        let c = &import_all(&linear())[0];
        assert_eq!(c.titles.custom.as_deref(), Some("Looping PostgreSQL queries"));
        assert_eq!(c.titles.generated, None);
        assert_eq!(c.titles.first_user.as_deref(), Some("how do I loop a query"));
        assert_eq!(c.titles.resolve(), Some("Looping PostgreSQL queries"));
    }

    #[test]
    fn an_absent_or_blank_title_leaves_the_first_user_message_to_resolve() {
        for title in [json!(null), json!(""), json!("   ")] {
            let bytes = export(vec![conversation(
                "conv-4",
                title,
                "u1",
                vec![
                    root("root-0", &["u1"]),
                    user("u1", Some("root-0"), &[], "a question worth remembering", 1.0),
                ],
            )]);
            let c = &import_all(&bytes)[0];
            assert_eq!(c.titles.custom, None);
            assert_eq!(c.titles.resolve(), Some("a question worth remembering"));
        }
    }

    #[test]
    fn a_long_opening_message_is_truncated_on_a_character_boundary() {
        let long = "é".repeat(200);
        let bytes = export(vec![conversation(
            "conv-5",
            json!(null),
            "u1",
            vec![root("root-0", &["u1"]), user("u1", Some("root-0"), &[], &long, 1.0)],
        )]);
        let t = import_all(&bytes)[0].titles.first_user.clone().unwrap();
        assert_eq!(t.chars().count(), FIRST_USER_TITLE_CHARS + 1, "80 chars plus the ellipsis");
        assert!(t.ends_with('…'));
    }

    #[test]
    fn timestamps_convert_from_float_seconds_to_millis() {
        let c = &import_all(&linear())[0];
        assert_eq!(c.messages[0].ts, Some(1_680_295_988_197));
        assert_eq!(c.messages[3].ts, Some(1_680_296_329_204));
    }

    #[test]
    fn a_missing_or_absurd_timestamp_is_none_rather_than_a_panic() {
        let mut null_ts = user("u1", Some("root-0"), &["u2"], "hi", 1.0);
        null_ts["message"]["create_time"] = json!(null);
        null_ts["message"]["update_time"] = json!(null);
        let mut huge_ts = user("u2", Some("u1"), &[], "hi again", 1.0);
        huge_ts["message"]["create_time"] = json!(1e300);

        let bytes = export(vec![conversation(
            "conv-6",
            json!("Timestamps"),
            "u2",
            vec![root("root-0", &["u1"]), null_ts, huge_ts],
        )]);
        let c = &import_all(&bytes)[0];
        assert_eq!(c.messages[0].ts, None);
        assert_eq!(c.messages[1].ts, None);
    }

    #[test]
    fn structural_hidden_and_empty_nodes_are_skipped_and_children_re_parent() {
        let mut hidden = assistant("hidden", Some("u1"), &["sys"], "internal note", 2.0);
        hidden["message"]["metadata"]["is_visually_hidden_from_conversation"] = json!(true);
        let mut sys = assistant("sys", Some("hidden"), &["blank"], "you are helpful", 3.0);
        sys["message"]["author"]["role"] = json!("system");

        let bytes = export(vec![conversation(
            "conv-7",
            json!("Skips"),
            "a1",
            vec![
                root("root-0", &["u1"]),
                user("u1", Some("root-0"), &["hidden"], "ask", 1.0),
                hidden,
                sys,
                assistant("blank", Some("sys"), &["a1"], "", 4.0),
                assistant("a1", Some("blank"), &[], "the real answer", 5.0),
            ],
        )]);
        let c = &import_all(&bytes)[0];

        let ids: Vec<&str> = c.messages.iter().map(|m| m.native_id.as_str()).collect();
        assert_eq!(ids, ["u1", "a1"]);
        assert_eq!(
            c.messages[1].parent_native_id.as_deref(),
            Some("u1"),
            "three skipped nodes collapse away and the chain stays walkable"
        );
    }

    #[test]
    fn reasoning_tool_and_multimodal_content_map_to_their_kinds() {
        let thoughts = node(
            "th",
            Some("u1"),
            &["recap"],
            json!({"role": "assistant"}),
            json!({
                "content_type": "thoughts",
                "thoughts": [
                    {"summary": "Weighing options", "content": "the user wants X", "chunks": [], "finished": true},
                    {"summary": "", "content": "", "chunks": ["a stray chunk"], "finished": true},
                ],
                "source_analysis_msg_id": "u1",
            }),
            2.0,
        );
        let recap = node(
            "recap",
            Some("th"),
            &["call"],
            json!({"role": "assistant"}),
            json!({"content_type": "reasoning_recap", "content": "Thought for 4 seconds"}),
            3.0,
        );
        let mut call = node(
            "call",
            Some("recap"),
            &["out"],
            json!({"role": "assistant", "name": "python"}),
            json!({"content_type": "code", "language": "python", "text": "print(1)"}),
            4.0,
        );
        call["message"]["recipient"] = json!("python");
        let out = node(
            "out",
            Some("call"),
            &["mm"],
            json!({"role": "tool", "name": "python"}),
            json!({"content_type": "execution_output", "text": "1"}),
            5.0,
        );
        let mm = node(
            "mm",
            Some("out"),
            &[],
            json!({"role": "user"}),
            json!({"content_type": "multimodal_text", "parts": [
                {"content_type": "image_asset_pointer", "asset_pointer": "file-service://x", "width": 10, "height": 10},
                {"content_type": "audio_transcription", "text": "spoken words", "direction": "in"},
                "and typed words",
            ]}),
            6.0,
        );

        let bytes = export(vec![conversation(
            "conv-8",
            json!("Kinds"),
            "mm",
            vec![
                root("root-0", &["u1"]),
                user("u1", Some("root-0"), &["th"], "ask", 1.0),
                thoughts,
                recap,
                call,
                out,
                mm,
            ],
        )]);

        let c = &import_all(&bytes)[0];
        let got: Vec<(&str, Role, Kind, &str)> = c
            .messages
            .iter()
            .map(|m| (m.native_id.as_str(), m.role, m.kind, m.text.as_str()))
            .collect();
        assert_eq!(
            got,
            [
                ("u1", Role::User, Kind::Prose, "ask"),
                (
                    "th",
                    Role::Assistant,
                    Kind::Reasoning,
                    "Weighing options\nthe user wants X\na stray chunk"
                ),
                ("recap", Role::Assistant, Kind::Reasoning, "Thought for 4 seconds"),
                ("call", Role::Assistant, Kind::ToolCall, "print(1)"),
                ("out", Role::Tool, Kind::ToolResult, "1"),
                ("mm", Role::User, Kind::Prose, "spoken words\nand typed words"),
            ]
        );
    }

    #[test]
    fn an_image_only_message_has_no_text_and_is_skipped() {
        let image = node(
            "img",
            Some("root-0"),
            &[],
            json!({"role": "user"}),
            json!({"content_type": "multimodal_text", "parts": [
                {"content_type": "image_asset_pointer", "asset_pointer": "file-service://x"},
            ]}),
            1.0,
        );
        let bytes =
            export(vec![conversation("conv-9", json!("Image"), "img", vec![root("root-0", &["img"]), image])]);
        let c = &import_all(&bytes)[0];
        assert!(c.messages.is_empty());
        assert_eq!(c.head_native_id, None);
    }

    #[test]
    fn many_conversations_come_back_from_one_file() {
        let mut both: Value = serde_json::from_slice(&linear()).unwrap();
        let second: Value = serde_json::from_slice(&edit_branch()).unwrap();
        both.as_array_mut().unwrap().push(second[0].clone());
        let convs = import_all(&serde_json::to_vec(&both).unwrap());
        assert_eq!(convs.len(), 2);
        assert_eq!(convs[0].native_id, "conv-1");
        assert_eq!(convs[1].native_id, "conv-2");
    }

    #[test]
    fn an_envelope_or_a_single_conversation_object_is_accepted() {
        let arr: Value = serde_json::from_slice(&linear()).unwrap();
        let wrapped = json!({"conversations": arr});
        assert_eq!(import_all(&serde_json::to_vec(&wrapped).unwrap()).len(), 1);
        assert_eq!(import_all(&serde_json::to_vec(&arr[0]).unwrap()).len(), 1);
    }

    #[test]
    fn malformed_input_returns_nothing_rather_than_panicking() {
        let cases: Vec<Vec<u8>> = vec![
            b"".to_vec(),
            b"[".to_vec(),
            b"not json at all".to_vec(),
            b"[]".to_vec(),
            b"{}".to_vec(),
            b"null".to_vec(),
            b"[1, 2, 3]".to_vec(),
            b"[{}]".to_vec(),
            // one conversation with no id, one with no mapping
            serde_json::to_vec(&json!([{"title": "x", "mapping": {}}, {"id": "z"}])).unwrap(),
            // mapping is the wrong type
            serde_json::to_vec(&json!([{"id": "a", "mapping": []}])).unwrap(),
            // nodes are the wrong type all the way down
            serde_json::to_vec(&json!([{"id": "a", "mapping": {"n": 7}, "current_node": 4}])).unwrap(),
            // a message that is a string; a message with no author, content or children
            serde_json::to_vec(&json!([{"id": "a", "mapping": {
                "n1": {"id": "n1", "parent": null, "children": ["n2"], "message": "oops"},
                "n2": {"id": "n2", "parent": "n1", "children": [], "message": {"author": {}}},
            }}]))
            .unwrap(),
        ];
        for bytes in cases {
            assert!(import_all(&bytes).iter().all(|c| c.messages.is_empty()));
        }
    }

    #[test]
    fn a_cycle_terminates_and_keeps_every_reachable_message() {
        // Not observed in the real export, but a walk that trusts `children` must not hang.
        let bytes = export(vec![conversation(
            "conv-10",
            json!("Loop"),
            "b",
            vec![
                user("a", Some("b"), &["b"], "one", 1.0),
                user("b", Some("a"), &["a"], "two", 2.0),
            ],
        )]);
        let c = &import_all(&bytes)[0];
        assert_eq!(c.messages.len(), 2);
        assert_eq!(c.head_native_id.as_deref(), Some("b"));
    }

    #[test]
    fn a_conversation_with_no_usable_message_still_yields_its_identity() {
        // Two real conversations are nothing but a structural root. The title and the URL
        // are still worth keeping, and the head is unset rather than dangling.
        let bytes = export(vec![conversation(
            "conv-11",
            json!("Empty"),
            "root-0",
            vec![root("root-0", &[])],
        )]);
        let c = &import_all(&bytes)[0];
        assert!(c.messages.is_empty());
        assert_eq!(c.head_native_id, None);
        assert_eq!(c.resolved_head(), None);
        assert_eq!(c.titles.custom.as_deref(), Some("Empty"));
    }

    /// Sanity check against a real export. Ignored by default and driven by an env var, so
    /// no personal data reaches the repo or CI, and it prints counts only:
    ///
    /// ```sh
    /// CS_CHATGPT_EXPORT=<dir> cargo test -p cs-import -- --ignored --nocapture
    /// ```
    #[test]
    #[ignore = "needs a real export; set CS_CHATGPT_EXPORT to the directory holding conversations-*.json"]
    fn real_export_stats() {
        let Ok(dir) = std::env::var("CS_CHATGPT_EXPORT") else { return };
        let mut files: Vec<_> = std::fs::read_dir(&dir)
            .expect("readable export directory")
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with("conversations-") && n.ends_with(".json"))
            })
            .collect();
        files.sort();

        let (mut convs, mut msgs, mut heads, mut siblings, mut empty) = (0usize, 0usize, 0usize, 0usize, 0usize);
        let mut kinds: HashMap<&str, usize> = HashMap::new();
        for f in &files {
            for c in import_all(&std::fs::read(f).unwrap()) {
                convs += 1;
                msgs += c.messages.len();
                heads += usize::from(c.head_native_id.is_some());
                empty += usize::from(c.messages.is_empty());
                let mut children: HashMap<&str, usize> = HashMap::new();
                for m in &c.messages {
                    *kinds.entry(m.kind.as_str()).or_default() += 1;
                    if let Some(p) = m.parent_native_id.as_deref() {
                        *children.entry(p).or_default() += 1;
                    }
                }
                siblings += children.values().filter(|n| **n > 1).map(|n| n - 1).sum::<usize>();
            }
        }
        let mut kinds: Vec<_> = kinds.into_iter().collect();
        kinds.sort_unstable();
        println!(
            "files={} conversations={convs} messages={msgs} heads={heads} \
             empty_conversations={empty} edit_branch_siblings={siblings} kinds={kinds:?}",
            files.len()
        );
        assert!(convs > 0 && msgs > 0);
    }
}
