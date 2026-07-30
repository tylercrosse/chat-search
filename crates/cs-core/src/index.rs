use crate::model::{Conversation, Kind};
use crate::schema::{DDL, IMPORTER_VERSION};
use rusqlite::{params, Connection, Transaction};
use std::collections::{HashMap, HashSet};

/// How much of a tool call or result to keep in the index.
///
/// Tool traffic is 85% of the corpus text and none of it is searched by default (ADR 5),
/// yet it dominated the index: 201 MB of tool_result and 40 MB of tool_call against 41 MB of
/// prose. All of it is reproducible from the archive, so by ADR 1 it does not have to be
/// here at all — but the *head* of a tool call earns its keep, because it holds the tool
/// name and the start of its arguments, which is what makes "which conversation ran cargo
/// build" answerable.
///
/// Measured: capping at 1 KiB cuts stored text 66%, truncating 12% of tool calls and 45% of
/// tool results. Set to 0 to drop tool text entirely (85%).
pub const TOOL_TEXT_MAX: usize = 1024;

#[derive(Debug, Clone, Copy)]
pub struct IndexOptions {
    pub tool_text_limit: usize,
}

impl Default for IndexOptions {
    fn default() -> Self {
        Self { tool_text_limit: TOOL_TEXT_MAX }
    }
}

/// Keep the head of a tool message and say how much was dropped.
///
/// The marker matters: a silently shortened tool result looks like the tool returned little,
/// which is a different and more misleading thing than "there is more in the archive".
fn clip_tool_text(text: &str, limit: usize) -> String {
    if limit == 0 {
        return String::new();
    }
    if text.len() <= limit {
        return text.to_string();
    }
    // truncate on a char boundary — byte slicing panics on multi-byte UTF-8
    let end = (0..=limit).rev().find(|&i| text.is_char_boundary(i)).unwrap_or(0);
    format!("{}…[{} more bytes in the archive]", &text[..end], text.len() - end)
}

#[derive(Debug, Default, Clone, Copy)]
pub struct IndexStats {
    /// Distinct conversation *rows* written — what the index actually holds.
    ///
    /// Not the number of conversation objects the importers produced, which is larger
    /// whenever the same conversation arrives twice. A ChatGPT export is a whole-account
    /// snapshot, so a second export re-delivers every conversation in the first: counting
    /// objects reported 4,022 for a corpus of 2,011, and the dedup it looked like a failure
    /// of was working correctly the whole time (chat-search-a7k.8).
    pub conversations: u64,
    /// Conversation objects that folded into a row already present. Two legitimate causes:
    /// one conversation split across several transcript files (ADR 7), and a re-delivered
    /// snapshot.
    pub merged: u64,
    pub messages: u64,
    pub prose: u64,
    /// Messages already present, by id. Same idea as `merged`, one level down.
    pub duplicates: u64,
    pub text_bytes: u64,
}

/// The importer version that built this index, if it recorded one.
///
/// `None` for an index written before `build_info` existed, which is indistinguishable from
/// a corrupt one and is treated the same way: stale.
pub fn built_by(conn: &Connection) -> Option<u32> {
    conn.query_row(
        "SELECT value FROM build_info WHERE key = 'importer_version'",
        [],
        |r| r.get::<_, String>(0),
    )
    .ok()?
    .parse()
    .ok()
}

/// Refuse to read an index this binary did not build.
///
/// `build_info` has been written since the schema existed and read by nothing, so a schema
/// change surfaced as `no such column` from somewhere deep in a query — an error about
/// SQLite when the actual situation is "this index predates the binary". The index is a pure
/// function of the archive (ADR 1), so the remedy is always the same and worth saying.
pub fn ensure_current(conn: &Connection) -> Result<(), String> {
    match built_by(conn) {
        Some(v) if v == IMPORTER_VERSION => Ok(()),
        Some(v) => Err(format!(
            "index was built by importer version {v}, this is version {IMPORTER_VERSION} — \
             run `cs index` to rebuild"
        )),
        None => Err(
            "index records no importer version, so it predates this schema — \
             run `cs index` to rebuild"
                .to_string(),
        ),
    }
}

pub fn open(path: &str) -> rusqlite::Result<Connection> {
    let conn = Connection::open(path)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "OFF")?;
    conn.execute_batch(DDL)?;
    Ok(conn)
}

/// Open an index from scratch, discarding whatever was there.
///
/// The file is *deleted* rather than emptied. Two reasons, both learned the hard way:
/// `DELETE FROM` a contentless fts5 table fails once it holds rows (it silently succeeds
/// while empty, so the bug hides until the second run), and `CREATE TABLE IF NOT EXISTS`
/// will not add a column that a newer schema introduced.
///
/// Deleting is also simply the correct move under ADR 1 — the index is a pure function of
/// the archive, so there is no state worth preserving and no migration to write. If this
/// ever needs to become an in-place migration, something has gone in that the archive
/// cannot reproduce.
pub fn open_fresh(path: &str) -> rusqlite::Result<Connection> {
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{path}{suffix}"));
    }
    open(path)
}

/// Empty an already-open index. Only usable on tables that support it — prefer
/// [`open_fresh`] for a real rebuild.
pub fn reset(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "DELETE FROM conversation; DELETE FROM message; DELETE FROM build_info;
         INSERT INTO fts_prose(fts_prose) VALUES('delete-all');
         INSERT INTO fts_tools(fts_tools) VALUES('delete-all');",
    )
}

pub fn write_conversations<'a, I>(conn: &mut Connection, convs: I) -> rusqlite::Result<IndexStats>
where
    I: IntoIterator<Item = &'a Conversation>,
{
    write_conversations_with(conn, convs, IndexOptions::default())
}

pub fn write_conversations_with<'a, I>(
    conn: &mut Connection,
    convs: I,
    opts: IndexOptions,
) -> rusqlite::Result<IndexStats>
where
    I: IntoIterator<Item = &'a Conversation>,
{
    let mut stats = IndexStats::default();
    let tx = conn.transaction()?;
    for c in convs {
        write_one(&tx, c, &mut stats, opts)?;
    }
    tx.execute_batch(
        "UPDATE conversation SET
           msg_count    = (SELECT COUNT(*) FROM message m WHERE m.conv_id = conversation.id),
           prose_count  = (SELECT COUNT(*) FROM message m WHERE m.conv_id = conversation.id AND m.kind='prose'),
           user_turns   = (SELECT COUNT(*) FROM message m WHERE m.conv_id = conversation.id AND m.kind='prose' AND m.role='user'),
           thread_count = (SELECT COUNT(DISTINCT m.thread_key) FROM message m WHERE m.conv_id = conversation.id),
           started_at   = (SELECT MIN(m.ts) FROM message m WHERE m.conv_id = conversation.id),
           ended_at     = (SELECT MAX(m.ts) FROM message m WHERE m.conv_id = conversation.id)",
    )?;
    tx.execute(
        "INSERT OR REPLACE INTO build_info(key, value) VALUES ('importer_version', ?1)",
        params![IMPORTER_VERSION.to_string()],
    )?;
    tx.commit()?;
    Ok(stats)
}

fn write_one(
    tx: &Transaction,
    c: &Conversation,
    stats: &mut IndexStats,
    opts: IndexOptions,
) -> rusqlite::Result<()> {
    let conv_id = c.id();
    let head_id = c.resolved_head().map(|h| c.message_id(h));
    let on_path = on_head_path(c);

    // Asked before the upsert because the upsert cannot answer it: `ON CONFLICT DO UPDATE`
    // reports one changed row whether it inserted or merged, so there is no way to tell
    // afterwards. A point lookup on the primary key is the cost of the report being true.
    let already: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM conversation WHERE id = ?1)",
        params![conv_id],
        |r| r.get(0),
    )?;

    // COALESCE rather than first-write-wins: a conversation can be assembled from several
    // transcript files and only some carry the title, cwd or model (ADR 7).
    tx.execute(
        "INSERT INTO conversation
           (id, source, native_id, title, title_origin, cwd, git_branch, model, surface,
            forked_from, head_id, resume_cmd)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)
         ON CONFLICT(id) DO UPDATE SET
           title        = COALESCE(conversation.title, excluded.title),
           title_origin = COALESCE(conversation.title_origin, excluded.title_origin),
           cwd          = COALESCE(conversation.cwd, excluded.cwd),
           git_branch   = COALESCE(conversation.git_branch, excluded.git_branch),
           model        = COALESCE(conversation.model, excluded.model),
           surface      = COALESCE(conversation.surface, excluded.surface),
           forked_from  = COALESCE(conversation.forked_from, excluded.forked_from),
           head_id      = COALESCE(conversation.head_id, excluded.head_id),
           resume_cmd   = COALESCE(conversation.resume_cmd, excluded.resume_cmd)",
        params![
            conv_id,
            c.source,
            c.native_id,
            c.titles.resolve(),
            title_origin(c),
            c.cwd,
            c.git_branch,
            c.model,
            c.surface,
            c.forked_from_native_id.as_ref().map(|f| format!("{}:{}", c.source, f)),
            head_id,
            c.resume_cmd,
        ],
    )?;

    let mut ins_msg = tx.prepare_cached(
        "INSERT OR IGNORE INTO message
           (id, conv_id, parent_id, thread_key, is_sidechain, seq, role, kind, ts,
            on_head_path, is_error, text)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
    )?;
    let mut ins_prose = tx.prepare_cached("INSERT INTO fts_prose(rowid, text) VALUES (?1,?2)")?;
    let mut ins_tools = tx.prepare_cached("INSERT INTO fts_tools(rowid, text) VALUES (?1,?2)")?;

    for m in &c.messages {
        // Tool text is clipped on the way in rather than at query time, so the bytes never
        // reach the database and the FTS index only ever sees what is stored.
        let stored_text = if m.kind.is_tool() {
            clip_tool_text(&m.text, opts.tool_text_limit)
        } else {
            m.text.clone()
        };
        let changed = ins_msg.execute(params![
            c.message_id(&m.native_id),
            conv_id,
            m.parent_native_id.as_ref().map(|p| c.message_id(p)),
            m.thread_key,
            m.is_sidechain as i64,
            m.seq,
            m.role.as_str(),
            m.kind.as_str(),
            m.ts,
            on_path.contains(m.native_id.as_str()) as i64,
            m.is_error as i64,
            stored_text,
        ])?;
        if changed == 0 {
            stats.duplicates += 1;
            continue;
        }
        let rowid = tx.last_insert_rowid();
        match m.kind {
            Kind::Prose => {
                ins_prose.execute(params![rowid, stored_text])?;
                stats.prose += 1;
            }
            k if k.is_tool() && !stored_text.is_empty() => {
                ins_tools.execute(params![rowid, stored_text])?;
            }
            _ => {}
        }
        stats.messages += 1;
        stats.text_bytes += stored_text.len() as u64;
    }

    if already {
        stats.merged += 1;
    } else {
        stats.conversations += 1;
    }
    Ok(())
}

fn title_origin(c: &Conversation) -> Option<&'static str> {
    if c.titles.custom.is_some() {
        Some("custom")
    } else if c.titles.generated.is_some() {
        Some("generated")
    } else if c.titles.first_user.is_some() {
        Some("first_user")
    } else {
        None
    }
}

/// Messages reachable by walking parents from each thread's leaf.
///
/// Per *thread*, not just from the conversation head: a subagent thread is parallel to the
/// main one, not an abandoned branch, so marking it off-path would misreport it. What this
/// does catch is the edit-branch case — a superseded sibling under the same parent.
fn on_head_path(c: &Conversation) -> HashSet<&str> {
    let by_id: HashMap<&str, &crate::model::Message> =
        c.messages.iter().map(|m| (m.native_id.as_str(), m)).collect();

    // leaf of each thread = highest seq
    let mut leaves: HashMap<&str, &crate::model::Message> = HashMap::new();
    for m in &c.messages {
        leaves
            .entry(m.thread_key.as_str())
            .and_modify(|cur| {
                if m.seq > cur.seq {
                    *cur = m
                }
            })
            .or_insert(m);
    }
    if let Some(head) = c.head_native_id.as_deref().and_then(|h| by_id.get(h)) {
        leaves.insert(head.thread_key.as_str(), head);
    }

    let mut on = HashSet::new();
    for leaf in leaves.values() {
        let mut cur = Some(*leaf);
        while let Some(m) = cur {
            if !on.insert(m.native_id.as_str()) {
                break; // already walked this ancestry
            }
            cur = m.parent_native_id.as_deref().and_then(|p| by_id.get(p)).copied();
        }
    }
    on
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Message, Role, Titles};

    fn conv(messages: Vec<Message>) -> Conversation {
        Conversation {
            source: "codex".into(),
            native_id: "c1".into(),
            titles: Titles { first_user: Some("hello".into()), ..Default::default() },
            cwd: None,
            git_branch: None,
            model: None,
            surface: None,
            forked_from_native_id: None,
            resume_cmd: Some("codex resume c1".into()),
            head_native_id: None,
            messages,
        }
    }

    fn m(id: &str, parent: Option<&str>, seq: i64, thread: &str, side: bool, text: &str) -> Message {
        Message {
            native_id: id.into(),
            parent_native_id: parent.map(String::from),
            thread_key: thread.into(),
            is_sidechain: side,
            is_error: false,
            seq,
            role: Role::User,
            kind: Kind::Prose,
            ts: Some(1_700_000_000_000 + seq),
            text: text.into(),
        }
    }

    #[test]
    fn an_edited_away_sibling_is_marked_off_path() {
        // b and b2 share parent a; b2 is later, so b is the superseded edit
        let c = conv(vec![
            m("a", None, 1, "main", false, "root"),
            m("b", Some("a"), 2, "main", false, "first attempt"),
            m("b2", Some("a"), 3, "main", false, "edited version"),
        ]);
        let on = on_head_path(&c);
        assert!(on.contains("a") && on.contains("b2"));
        assert!(!on.contains("b"), "the superseded sibling should be off-path");
    }

    #[test]
    fn a_subagent_thread_stays_on_path() {
        // parallel, not abandoned — marking it off-path would misreport search hits
        let c = conv(vec![
            m("a", None, 1, "main", false, "root"),
            m("b", Some("a"), 2, "main", false, "next"),
            m("s1", None, 3, "sub-1", true, "subagent work"),
        ]);
        let on = on_head_path(&c);
        assert!(on.contains("s1"), "subagent messages are parallel, not superseded");
        assert!(on.contains("b"));
    }

    #[test]
    fn writes_and_aggregates() {
        let path = std::env::temp_dir().join(format!("cs-idx-{}.db", uuid::Uuid::new_v4()));
        let mut conn = open(path.to_str().unwrap()).unwrap();
        let c = conv(vec![
            m("a", None, 1, "main", false, "sqlite full text search"),
            m("b", Some("a"), 2, "main", false, "more about ranking"),
            m("s1", None, 3, "sub-1", true, "subagent output"),
        ]);
        let stats = write_conversations(&mut conn, [&c]).unwrap();
        assert_eq!(stats.conversations, 1);
        assert_eq!(stats.messages, 3);
        assert_eq!(stats.prose, 3);

        let (msgs, threads, head): (i64, i64, String) = conn
            .query_row(
                "SELECT msg_count, thread_count, head_id FROM conversation WHERE id='codex:c1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(msgs, 3);
        assert_eq!(threads, 2, "main plus one subagent thread");
        assert_eq!(head, "codex:c1:b", "head is the last main-thread message");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn tool_text_is_clipped_and_says_so() {
        assert_eq!(clip_tool_text("short", 1024), "short");
        let long = "x".repeat(2000);
        let clipped = clip_tool_text(&long, 100);
        assert!(clipped.starts_with(&"x".repeat(100)));
        assert!(clipped.contains("1900 more bytes in the archive"),
                "truncation must be visible: a silently shortened result reads as a short result");
        assert_eq!(clip_tool_text(&long, 0), "", "0 drops tool text entirely");
    }

    #[test]
    fn clipping_never_splits_a_character() {
        // a limit landing mid-codepoint would panic on a byte slice
        let s = "héllo wörld ünïcode ▲▼◆".repeat(20);
        for limit in 1..64 {
            let out = clip_tool_text(&s, limit);
            assert!(out.is_char_boundary(0));
            assert!(std::str::from_utf8(out.as_bytes()).is_ok());
        }
    }

    #[test]
    fn prose_is_never_clipped() {
        let path = std::env::temp_dir().join(format!("cs-clip-{}.db", uuid::Uuid::new_v4()));
        let mut conn = open(path.to_str().unwrap()).unwrap();
        let long = "prose ".repeat(500);
        let mut c = conv(vec![m("a", None, 1, "main", false, &long)]);
        c.messages.push(Message { kind: Kind::ToolResult, ..c.messages[0].clone() });
        c.messages[1].native_id = "b".into();
        write_conversations_with(&mut conn, [&c], IndexOptions { tool_text_limit: 50 }).unwrap();

        let kept: i64 = conn
            .query_row("SELECT length(text) FROM message WHERE kind='prose'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(kept as usize, long.len(), "prose must survive intact");
        let tool: i64 = conn
            .query_row("SELECT length(text) FROM message WHERE kind='tool_result'", [], |r| r.get(0))
            .unwrap();
        assert!(tool < 120, "tool text should be clipped, got {tool}");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn reimporting_the_same_conversation_is_idempotent() {
        let path = std::env::temp_dir().join(format!("cs-idx-{}.db", uuid::Uuid::new_v4()));
        let mut conn = open(path.to_str().unwrap()).unwrap();
        let c = conv(vec![m("a", None, 1, "main", false, "hello")]);
        write_conversations(&mut conn, [&c]).unwrap();
        let second = write_conversations(&mut conn, [&c]).unwrap();
        assert_eq!(second.messages, 0);
        assert_eq!(second.duplicates, 1);
        let n: i64 = conn.query_row("SELECT COUNT(*) FROM message", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 1);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn a_re_delivered_snapshot_is_counted_as_merged_rather_than_as_a_second_conversation() {
        // A ChatGPT export is a whole-account snapshot, so a second export re-delivers every
        // conversation in the first. Counting objects rather than rows reported 4,022 for a
        // corpus of 2,011 and made working dedup look broken (chat-search-a7k.8).
        let path = std::env::temp_dir().join(format!("cs-snap-{}.db", uuid::Uuid::new_v4()));
        let mut conn = open(path.to_str().unwrap()).unwrap();
        let c = conv(vec![m("a", None, 1, "main", false, "hello")]);

        // Both copies in one pass, which is what indexing two archived exports looks like.
        let stats = write_conversations(&mut conn, [&c, &c]).unwrap();
        assert_eq!(stats.conversations, 1, "one row, so one conversation");
        assert_eq!(stats.merged, 1, "and the second delivery said so rather than vanishing");

        let rows: i64 =
            conn.query_row("SELECT COUNT(*) FROM conversation", [], |r| r.get(0)).unwrap();
        assert_eq!(rows, stats.conversations as i64, "the count must be what the table holds");
        std::fs::remove_file(&path).ok();
    }
}
