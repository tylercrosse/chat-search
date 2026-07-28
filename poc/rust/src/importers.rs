use crate::types::*;
use crate::walk::walk;
use serde_json::Value;
use std::path::PathBuf;

pub struct SourceFile {
    pub source: &'static str,
    pub path: PathBuf,
}

fn s(v: Option<&Value>) -> Option<String> {
    v.and_then(|x| x.as_str()).map(|x| x.to_string())
}

// ---------------------------------------------------------------- codex

pub fn import_codex(path: &PathBuf) -> Option<Conv> {
    let buf = std::fs::read(path).ok()?;
    // `codex resume` writes a new rollout file that reuses the original session_id, so
    // the per-file ordinal must be scoped by the file or ids collide across branches.
    let stem = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let mut session_id: Option<String> = None;
    let mut cwd = None;
    let mut model = None;
    let mut forked_from = None;
    let mut title: Option<String> = None;
    // A subagent thread shares its parent's session_id and lands in its own rollout file.
    // That is why several files map to one conversation — it is not a resume.
    let mut is_sidechain = false;
    let mut messages: Vec<Msg> = Vec::new();
    let mut seq: i64 = 0;

    for line in lines(&buf) {
        // truncated final line on an in-flight session is expected, not an error
        let Ok(d) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let Some(p) = d.get("payload").filter(|p| p.is_object()) else {
            continue;
        };
        let dtype = d.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let ptype = p.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let ts = parse_ts(d.get("timestamp"));

        if dtype == "session_meta" {
            session_id = s(p.get("session_id")).or_else(|| s(p.get("id")));
            cwd = s(p.get("cwd"));
            model = s(p.get("model_provider"));
            forked_from = s(p.get("forked_from_id"));
            is_sidechain = p.get("thread_source").and_then(|v| v.as_str()) == Some("subagent")
                || p.get("source").and_then(|v| v.get("subagent")).is_some();
            continue;
        }

        let (role, kind, text) = match (dtype, ptype) {
            ("event_msg", "user_message") => {
                ("user", Kind::Prose, flatten(p.get("message").unwrap_or(&Value::Null)))
            }
            ("event_msg", "agent_message") => {
                ("assistant", Kind::Prose, flatten(p.get("message").unwrap_or(&Value::Null)))
            }
            ("response_item", "reasoning") => (
                "assistant",
                Kind::Reasoning,
                flatten(p.get("summary").or_else(|| p.get("content")).unwrap_or(&Value::Null)),
            ),
            ("response_item", "function_call") | ("response_item", "custom_tool_call") => (
                "assistant",
                Kind::ToolCall,
                format!(
                    "{}\n{}",
                    p.get("name").and_then(|v| v.as_str()).unwrap_or(""),
                    flatten(p.get("arguments").or_else(|| p.get("input")).unwrap_or(&Value::Null))
                ),
            ),
            ("response_item", "function_call_output")
            | ("response_item", "custom_tool_call_output") => (
                "tool",
                Kind::ToolResult,
                flatten(p.get("output").unwrap_or(&Value::Null)),
            ),
            _ => continue,
        };

        let text = text.trim().to_string();
        if text.is_empty() {
            continue;
        }
        if kind == Kind::Prose && role == "user" && title.is_none() {
            title = Some(title_from(&text));
        }

        messages.push(Msg {
            native_id: format!("{stem}:{seq:06}"),
            parent_id: None,
            seq,
            role: role.to_string(),
            kind,
            ts,
            is_sidechain,
            text,
        });
        seq += 1;
    }

    let session_id = session_id?;
    if messages.is_empty() {
        return None;
    }
    Some(Conv {
        source: "codex",
        resume_cmd: format!("codex resume {session_id}"),
        native_id: session_id,
        title,
        cwd,
        git_branch: None,
        model,
        forked_from,
        messages,
    })
}

// ---------------------------------------------------------- claude-code

pub fn import_claude_code(path: &PathBuf) -> Option<Conv> {
    let buf = std::fs::read(path).ok()?;
    let mut session_id: Option<String> = None;
    let mut cwd: Option<String> = None;
    let mut git_branch: Option<String> = None;
    let mut model: Option<String> = None;
    let mut title: Option<String> = None;
    let mut custom_title: Option<String> = None;
    let mut first_user: Option<String> = None;
    let mut messages: Vec<Msg> = Vec::new();
    let mut seq: i64 = 0;

    for line in lines(&buf) {
        let Ok(d) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if session_id.is_none() {
            session_id = s(d.get("sessionId")).or_else(|| s(d.get("session_id")));
        }
        if cwd.is_none() {
            cwd = s(d.get("cwd"));
        }
        if git_branch.is_none() {
            git_branch = s(d.get("gitBranch"));
        }

        let dtype = d.get("type").and_then(|v| v.as_str()).unwrap_or("");
        // Title is a fold over an append-only log, not a stored value. A user rename emits
        // `custom-title` (re-emitted on every save, so last wins) and outranks the generated
        // `ai-title`. Both outrank falling back to the first user message.
        if dtype == "custom-title" {
            if let Some(t) = s(d.get("customTitle")) {
                custom_title = Some(title_from(&t));
            }
            continue;
        }
        if dtype == "ai-title" {
            if let Some(t) = s(d.get("aiTitle")) {
                title = Some(title_from(&t));
            }
            continue;
        }
        if dtype != "user" && dtype != "assistant" {
            continue;
        }
        if d.get("isMeta").and_then(|v| v.as_bool()) == Some(true) {
            continue; // synthetic system-injected turn
        }
        let Some(m) = d.get("message") else { continue };
        if dtype == "assistant" && model.is_none() {
            model = s(m.get("model"));
        }

        let ts = parse_ts(d.get("timestamp"));
        let sidechain = d.get("isSidechain").and_then(|v| v.as_bool()) == Some(true);
        let parent_id = s(d.get("parentUuid"));
        let uuid = s(d.get("uuid")).unwrap_or_else(|| format!("{seq:06}"));

        // content is either a bare string or an array of typed blocks
        let owned;
        let blocks: &Vec<Value> = match m.get("content") {
            Some(Value::Array(a)) => a,
            Some(other) => {
                owned = vec![serde_json::json!({"type": "text", "text": other})];
                &owned
            }
            None => continue,
        };

        let mut part_idx = 0usize;
        for b in blocks {
            let btype = b.get("type").and_then(|v| v.as_str()).unwrap_or("");
            let (role, kind, text) = match btype {
                "text" => (
                    dtype.to_string(),
                    Kind::Prose,
                    flatten(b.get("text").unwrap_or(&Value::Null)),
                ),
                "thinking" => (
                    dtype.to_string(),
                    Kind::Reasoning,
                    flatten(b.get("thinking").unwrap_or(&Value::Null)),
                ),
                "tool_use" => {
                    let input = b.get("input").unwrap_or(&Value::Null);
                    let flat = flatten(input);
                    let body = if flat.is_empty() { input.to_string() } else { flat };
                    (
                        dtype.to_string(),
                        Kind::ToolCall,
                        format!(
                            "{}\n{}",
                            b.get("name").and_then(|v| v.as_str()).unwrap_or(""),
                            body
                        ),
                    )
                }
                "tool_result" => (
                    "tool".to_string(),
                    Kind::ToolResult,
                    flatten(b.get("content").unwrap_or(&Value::Null)),
                ),
                _ => continue,
            };

            let text = text.trim().to_string();
            if text.is_empty() {
                continue;
            }
            if kind == Kind::Prose && role == "user" && first_user.is_none() {
                first_user = Some(title_from(&text));
            }

            messages.push(Msg {
                native_id: if part_idx == 0 {
                    uuid.clone()
                } else {
                    format!("{uuid}#{part_idx}")
                },
                parent_id: parent_id.clone(),
                seq,
                role,
                kind,
                ts,
                is_sidechain: sidechain,
                text,
            });
            seq += 1;
            part_idx += 1;
        }
    }

    let session_id = session_id?;
    if messages.is_empty() {
        return None;
    }
    Some(Conv {
        source: "claude-code",
        resume_cmd: format!("claude --resume {session_id}"),
        native_id: session_id,
        title: custom_title.or(title).or(first_user),
        cwd,
        git_branch,
        model,
        forked_from: None,
        messages,
    })
}

// ------------------------------------------------------------ discovery

pub fn discover() -> Vec<SourceFile> {
    let home = std::env::var("HOME").unwrap_or_default();
    let mut out = Vec::new();

    let mut codex = Vec::new();
    walk(&PathBuf::from(&home).join(".codex/sessions"), &mut codex);
    for p in codex {
        let name = p.to_string_lossy();
        if name.ends_with(".jsonl") && name.contains("rollout-") {
            out.push(SourceFile { source: "codex", path: p });
        }
    }

    let mut cc = Vec::new();
    walk(&PathBuf::from(&home).join(".claude/projects"), &mut cc);
    for p in cc {
        if p.to_string_lossy().ends_with(".jsonl") {
            out.push(SourceFile { source: "claude-code", path: p });
        }
    }
    // Directory enumeration order is not guaranteed and differs between runtimes and
    // machines, so it must be pinned. Sort on the string form, not PathBuf: Path's Ord
    // compares component-wise, which orders `<id>/subagents/x.jsonl` before `<id>.jsonl`
    // while a plain string compare does the opposite.
    out.sort_by(|a, b| a.path.to_string_lossy().cmp(&b.path.to_string_lossy()));
    out
}
