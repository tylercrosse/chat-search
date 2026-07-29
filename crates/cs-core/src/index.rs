use crate::model::{Conversation, Kind};
use crate::schema::{DDL, IMPORTER_VERSION};
use rusqlite::{params, Connection, Transaction};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Default, Clone, Copy)]
pub struct IndexStats {
    pub conversations: u64,
    pub messages: u64,
    pub prose: u64,
    pub duplicates: u64,
    pub text_bytes: u64,
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
    let mut stats = IndexStats::default();
    let tx = conn.transaction()?;
    for c in convs {
        write_one(&tx, c, &mut stats)?;
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

fn write_one(tx: &Transaction, c: &Conversation, stats: &mut IndexStats) -> rusqlite::Result<()> {
    let conv_id = c.id();
    let head_id = c.resolved_head().map(|h| c.message_id(h));
    let on_path = on_head_path(c);

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
            on_head_path, text)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
    )?;
    let mut ins_prose = tx.prepare_cached("INSERT INTO fts_prose(rowid, text) VALUES (?1,?2)")?;
    let mut ins_tools = tx.prepare_cached("INSERT INTO fts_tools(rowid, text) VALUES (?1,?2)")?;

    for m in &c.messages {
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
            m.text,
        ])?;
        if changed == 0 {
            stats.duplicates += 1;
            continue;
        }
        let rowid = tx.last_insert_rowid();
        match m.kind {
            Kind::Prose => {
                ins_prose.execute(params![rowid, m.text])?;
                stats.prose += 1;
            }
            k if k.is_tool() => {
                ins_tools.execute(params![rowid, m.text])?;
            }
            _ => {}
        }
        stats.messages += 1;
        stats.text_bytes += m.text.len() as u64;
    }

    stats.conversations += 1;
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
}
