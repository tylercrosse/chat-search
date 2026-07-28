use chrono::DateTime;
use serde_json::Value;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Prose,
    Reasoning,
    ToolCall,
    ToolResult,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Prose => "prose",
            Kind::Reasoning => "reasoning",
            Kind::ToolCall => "tool_call",
            Kind::ToolResult => "tool_result",
        }
    }
    pub fn is_tool(self) -> bool {
        matches!(self, Kind::ToolCall | Kind::ToolResult)
    }
}

pub struct Msg {
    pub native_id: String,
    pub parent_id: Option<String>,
    pub seq: i64,
    pub role: String,
    pub kind: Kind,
    pub ts: Option<i64>,
    pub is_sidechain: bool,
    pub text: String,
}

pub struct Conv {
    pub source: &'static str,
    pub native_id: String,
    pub title: Option<String>,
    pub cwd: Option<String>,
    pub git_branch: Option<String>,
    pub model: Option<String>,
    pub forked_from: Option<String>,
    pub resume_cmd: String,
    pub messages: Vec<Msg>,
}

pub fn parse_ts(v: Option<&Value>) -> Option<i64> {
    let s = v?.as_str()?;
    DateTime::parse_from_rfc3339(s).ok().map(|d| d.timestamp_millis())
}

pub fn title_from(s: &str) -> String {
    let collapsed = s.split_whitespace().collect::<Vec<_>>().join(" ");
    collapsed.chars().take(80).collect()
}

/// Collapse arbitrary content (string | block | block[]) into searchable text.
pub fn flatten(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        Value::Array(a) => {
            let parts: Vec<String> =
                a.iter().map(flatten).filter(|s| !s.is_empty()).collect();
            parts.join("\n")
        }
        Value::Object(o) => {
            for k in ["text", "content", "output", "message", "thinking"] {
                if let Some(inner) = o.get(k) {
                    return flatten(inner);
                }
            }
            String::new()
        }
        other => other.to_string(),
    }
}

/// Iterate JSONL lines from a byte buffer, skipping invalid UTF-8 lines.
pub fn lines(buf: &[u8]) -> impl Iterator<Item = &str> {
    buf.split(|&b| b == b'\n')
        .filter(|l| !l.is_empty())
        .filter_map(|l| std::str::from_utf8(l).ok())
}
