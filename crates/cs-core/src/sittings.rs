//! An activity log read back as conversations (chat-search-o1i.5).
//!
//! Google Takeout's `MyActivity.json` is an activity log, not a transcript: it carries no
//! conversation, thread or session key at any nesting level — verified across all 1,318
//! records of the reference export. So the importer emits one two-message conversation per
//! record, and a twenty-turn Gemini chat arrives as twenty separate conversations. A search
//! for a word the person used throughout one of them returns it three or four times over.
//!
//! Putting the regrouping in the importer would fix that and break something worse. A
//! conversation id is permanent (ADR 16), so an id derived from a gap threshold changes the
//! day the threshold does: every Gemini conversation would take a new id the first time
//! anyone tuned it, and the archive would gain a second copy of the whole corpus. Here,
//! being wrong is free. The fold is computed from the index on first use, lives in `temp`,
//! and is not part of any id, any archived byte, or any column `cs index` writes.
//!
//! # What a sitting is
//!
//! Records of one `(source, surface)` separated by less than [`GAP_MS`] of silence — one
//! stretch at the keyboard, which is the thing the export destroyed. Deliberately *not* a
//! "thread": the glossary already spends that word on a linear stream of messages **inside**
//! a conversation, and this is the opposite direction. A sitting is several conversations
//! read as one.
//!
//! Measured over the 1,271 activity records in the live index:
//!
//! ```text
//!    2 min -> 929 sittings (1.4 records each)     30 min -> 462 (2.8)   <- GAP_MS
//!    5 min -> 682          (1.9)                  60 min -> 420 (3.0)
//!   15 min -> 514          (2.5)                 120 min -> 386 (3.3)
//! ```
//!
//! The curve is flat from 15 to 60 minutes, which is the useful property: the threshold is
//! not delicate, so this is not a number anyone will be tempted to keep tuning. 30 minutes
//! folds 1,031 of those records into 222 rows and leaves 240 standing alone.
//!
//! # How it is used
//!
//! [`ensure`] materialises two temp tables, and [`crate::search`] joins them. `conv_sitting`
//! maps a member conversation to the conversation that opened its sitting and to where its
//! messages start within it; `sitting` carries the totals the row displays. Everything the
//! search layer needs is then a `LEFT JOIN` away, which is what keeps the fold out of the
//! Rust that ranks and hydrates — see [`OF_MESSAGE`] and the constants beside it.
//!
//! Conversations that are their own sitting are absent from both tables rather than present
//! as a one-member row. A `LEFT JOIN` that finds nothing is exactly "this conversation stands
//! for itself", the two tables stay near 1,000 rows instead of the whole corpus, and nothing
//! downstream has to tell a real fold from a trivial one.
//!
//! The search layer joins; a reader cannot, because it is handed one id and has to arrive at a
//! set. [`resolve`] is that direction, and [`Sitting`] is what any answer standing for several
//! conversations says about itself.

use rusqlite::{params, Connection, OptionalExtension};

/// How long a silence has to be to end a sitting.
///
/// Not a [`crate::search::SearchOptions`] knob. The measurement in this module's header is
/// the argument for that: anything from 15 to 60 minutes lands within 20% of the same answer,
/// so a per-query threshold would be a dial with nothing on the other end of it — and this
/// crate has already been burned once by an options struct that accepted seven values it
/// silently ignored (see [`crate::search::recent`]).
pub const GAP_MS: i64 = 30 * 60 * 1_000;

/// The `(source, surface)` pairs whose conversations are single activity-log records.
///
/// An allowlist, not a rule inferred from the rows, because "one record per turn" is a
/// property of the *export format* and the index keeps no trace of it. `gemini-workspace`
/// arrives through the same source id and is a real multi-turn transcript with an id of its
/// own; folding two of those together because they happened ten minutes apart would invent a
/// conversation that never existed. Sniffing for it structurally — one instant, at most two
/// messages — would catch a short Workspace conversation too, and would be a guess wearing a
/// predicate.
///
/// The cost is that a Google product this list does not name does not fold. That is today's
/// behaviour rather than a wrong one, which is the direction to fail in.
const ACTIVITY_LOGS: [(&str, &str); 2] =
    [("google-takeout", "gemini-apps"), ("google-takeout", "ai-mode")];

/// Attach the fold to a query that has `message m` in scope.
///
/// The alias is `sit` rather than a letter because these fragments are pasted into statements
/// that already name `c`, `m` and an FTS table, and a one-letter alias inside a `format!`ed
/// SQL string is the kind of thing that silently binds to the wrong table.
pub const OF_MESSAGE: &str = "LEFT JOIN conv_sitting sit ON sit.conv_id = m.conv_id";

/// The row a message belongs to: its sitting when it has one, its conversation otherwise.
///
/// This is the whole fold in one expression. Grouping by it rather than by `m.conv_id` is
/// what makes the twenty rows one — scores, match counts and totals all follow from where the
/// grouping key came from, so nothing downstream needs a second merge pass.
pub const GROUP_ID: &str = "ifnull(sit.sitting_id, m.conv_id)";

/// Where a message sits inside its row, counting from the start of the sitting.
///
/// `m.seq` is 0-based per conversation, so without the offset the second record's turns would
/// land on top of the first's and [`crate::search::match_density`] would draw every match of a
/// twenty-record sitting in the opening tenth of the strip.
pub const POSITION: &str = "m.seq + ifnull(sit.msg_offset, 0)";

/// Attach the fold to a query that has `conversation c` in scope.
///
/// Two joins, because the two questions differ: `sit` is non-null for the conversation that
/// *opens* a sitting and carries its totals, `mem` is non-null for every conversation that is
/// *in* one. A record inside a sitting but not opening it has `mem` and no `sit`.
pub const OF_CONVERSATION: &str = "LEFT JOIN sitting sit      ON sit.id = c.id\n         \
                                   LEFT JOIN conv_sitting mem ON mem.conv_id = c.id";

/// Keep one row per sitting: the conversation that opened it, or one that is in none.
pub const OPENERS_ONLY: &str = "(mem.conv_id IS NULL OR mem.sitting_id = c.id)";

/// A conversation's display total, taking the sitting's whenever it stands for one.
///
/// A function of the column name so the four of them cannot drift: `msg_count` is the
/// denominator [`crate::search::match_density`] divides by, and a row whose matches are
/// numbered across a sitting while its total is one record's would draw them all off the end
/// of the strip.
pub fn total(column: &str) -> String {
    format!("ifnull(sit.{column}, c.{column})")
}

const DDL: &str = "
CREATE TABLE IF NOT EXISTS temp.conv_sitting(
  conv_id    TEXT PRIMARY KEY,
  sitting_id TEXT NOT NULL,   -- the conversation that opened it: a real, permanent id
  msg_offset INTEGER NOT NULL -- messages of earlier members, so seq becomes a position
);
CREATE INDEX IF NOT EXISTS temp.idx_conv_sitting ON conv_sitting(sitting_id);

CREATE TABLE IF NOT EXISTS temp.sitting(
  id          TEXT PRIMARY KEY,
  ended_at    INTEGER,
  msg_count   INTEGER NOT NULL,
  prose_count INTEGER NOT NULL,
  user_turns  INTEGER NOT NULL
);

-- What the two tables above were built from, so a stale fold is detectable rather than
-- silently wrong. See `ensure`.
CREATE TABLE IF NOT EXISTS temp.sitting_build(
  records INTEGER NOT NULL,
  latest  INTEGER NOT NULL,
  gap_ms  INTEGER NOT NULL
);
";

/// Build the fold if it is missing or out of date, and touch nothing otherwise.
///
/// Called at the top of every search entry point, so the cost when warm is what matters, and
/// it is 0.04 ms: two cached statements, one of them a count answered out of
/// `idx_conversation_source` without reading a table row. The build behind it is 8–24 ms
/// against the live index — a third of the worst keystroke's whole budget if it ran per
/// query, which is why this is a temp table filled once rather than a map loaded in Rust on
/// every search. Both numbers are guarded by `search::sitting_cost`.
///
/// The fingerprint is how many activity-log conversations there are and when the last one
/// ended. A connection held open across a `cs index` — a running TUI, say — would otherwise
/// keep folding by a picture of a corpus that no longer exists, and a stale grouping is the
/// worst kind, because every row still looks plausible. Two numbers cannot prove a corpus
/// unchanged, but they cannot miss a reindex either, which is the case that actually happens.
pub fn ensure(conn: &Connection) -> rusqlite::Result<()> {
    if !exists(conn)? {
        conn.execute_batch(DDL)?;
    }
    let now = fingerprint(conn)?;
    let built = conn
        .prepare_cached("SELECT records, latest, gap_ms FROM sitting_build")?
        .query_row([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .optional()?;
    if built == Some(now) {
        return Ok(());
    }
    build(conn, GAP_MS)?;
    conn.execute("DELETE FROM sitting_build", [])?;
    conn.execute(
        "INSERT INTO sitting_build(records, latest, gap_ms) VALUES (?1, ?2, ?3)",
        params![now.0, now.1, now.2],
    )?;
    Ok(())
}

fn exists(conn: &Connection) -> rusqlite::Result<bool> {
    let n: i64 = conn
        .prepare_cached("SELECT count(*) FROM temp.sqlite_master WHERE name = 'sitting_build'")?
        .query_row([], |r| r.get(0))?;
    Ok(n == 1)
}

/// How many activity-log conversations there are, and when the last of them ended.
///
/// Filtered on `source` alone rather than on the `(source, surface)` pairs:
/// `idx_conversation_source` covers `(source, ended_at)`, so this is answered out of the index
/// without reading a single table row, where a surface predicate would turn it into a
/// thousand row lookups per keystroke to sharpen a number that only has to *change* when the
/// corpus does.
fn fingerprint(conn: &Connection) -> rusqlite::Result<(i64, i64, i64)> {
    let quoted: Vec<String> = sources().iter().map(|s| format!("'{s}'")).collect();
    let sql = format!(
        "SELECT count(*), ifnull(max(ended_at), 0)
         FROM conversation WHERE source IN ({})",
        quoted.join(", ")
    );
    let (records, latest) =
        conn.prepare_cached(&sql)?.query_row([], |r| Ok((r.get(0)?, r.get(1)?)))?;
    Ok((records, latest, GAP_MS))
}

/// Recompute both tables from scratch at a given threshold.
///
/// Whole-table rather than incremental, for the reason `cs index` has no migrations: it is a
/// pure function of what `conversation` holds, it costs 8–24 ms, and an incremental version
/// would be a second implementation of the gap rule that could disagree with this one.
fn build(conn: &Connection, gap_ms: i64) -> rusqlite::Result<()> {
    conn.execute_batch("DELETE FROM conv_sitting; DELETE FROM sitting;")?;

    // Three passes over the same rows in one statement. `opens` marks a record whose
    // predecessor is far enough back to have ended the previous sitting; the running sum of
    // that flag numbers the sittings; the third pass reads each sitting's opener and how many
    // messages precede each member inside it.
    //
    // `ORDER BY ended_at, id` rather than `ended_at` alone, because two records can share a
    // millisecond and a window whose order is ambiguous would put the same corpus into
    // different sittings on different runs.
    let sql = format!(
        "INSERT INTO conv_sitting(conv_id, sitting_id, msg_offset)
         WITH marked AS (
           SELECT source, surface, id, ended_at, msg_count,
                  CASE WHEN LAG(ended_at) OVER w IS NULL
                         OR ended_at - LAG(ended_at) OVER w >= ?1
                       THEN 1 ELSE 0 END AS opens
           FROM conversation
           WHERE (source, surface) IN (VALUES {pairs}) AND ended_at IS NOT NULL
           WINDOW w AS (PARTITION BY source, surface ORDER BY ended_at, id)
         ),
         numbered AS (
           SELECT source, surface, id, ended_at, msg_count,
                  SUM(opens) OVER (PARTITION BY source, surface ORDER BY ended_at, id) AS run
           FROM marked
         ),
         placed AS (
           SELECT id,
                  FIRST_VALUE(id) OVER w AS sitting_id,
                  COUNT(*) OVER (PARTITION BY source, surface, run) AS members,
                  ifnull(SUM(msg_count) OVER (w ROWS BETWEEN UNBOUNDED PRECEDING
                                                        AND 1 PRECEDING), 0) AS msg_offset
           FROM numbered
           WINDOW w AS (PARTITION BY source, surface, run ORDER BY ended_at, id)
         )
         -- A sitting of one is just the conversation, and saying so in a table would make
         -- every join downstream ask a question it does not need the answer to.
         SELECT id, sitting_id, msg_offset FROM placed WHERE members > 1",
        pairs = pairs()
    );
    conn.execute(&sql, params![gap_ms])?;

    // Totals read back off the membership rather than recomputed in the statement above, so
    // there is one definition of who is in a sitting instead of two.
    conn.execute(
        "INSERT INTO sitting(id, ended_at, msg_count, prose_count, user_turns)
         SELECT m.sitting_id, max(c.ended_at),
                sum(c.msg_count), sum(c.prose_count), sum(c.user_turns)
         FROM conv_sitting m JOIN conversation c ON c.id = m.conv_id
         GROUP BY m.sitting_id",
        [],
    )?;
    Ok(())
}

/// Every conversation in a sitting, earliest first; empty when the id opens no sitting.
///
/// One query per folded row rather than one for the page: only a handful of rows on a page
/// are sittings at all, and the index on `sitting_id` makes each of them a point lookup.
///
/// Takes the id that *opened* a sitting. A reader holding some other member's id wants
/// [`resolve`], which is the same question asked from where a client actually stands.
pub fn members(conn: &Connection, sitting_id: &str) -> rusqlite::Result<Vec<String>> {
    conn.prepare_cached(
        "SELECT conv_id FROM conv_sitting WHERE sitting_id = ?1 ORDER BY msg_offset, conv_id",
    )?
    .query_map(params![sitting_id], |r| r.get(0))?
    .collect()
}

/// Every conversation an id has to be read with, earliest first — itself alone when it is in
/// no sitting.
///
/// Never empty, and never a rewrite: the answer for an ordinary conversation is that one id,
/// so a caller writes one loop rather than branching on whether the fold applies. An id that
/// is in the index nowhere comes back as itself, which leaves "no such conversation" to the
/// caller that was going to have to say it anyway.
///
/// **Any member resolves to the whole sitting, not just the opener.** `Sitting::members` is on
/// the wire, so a client copying an id out of it has no reason to prefer the first — and the
/// hop through `sitting_id` is what stops the eleventh record opening as two messages while
/// the first opens as twenty-two (chat-search-o1i.8).
///
/// Assumes [`ensure`] has run, like every other reader of these tables.
pub fn resolve(conn: &Connection, conv_id: &str) -> rusqlite::Result<Vec<String>> {
    let opener: Option<String> = conn
        .prepare_cached("SELECT sitting_id FROM conv_sitting WHERE conv_id = ?1")?
        .query_row(params![conv_id], |r| r.get(0))
        .optional()?;
    match opener {
        Some(opener) => members(conn, &opener),
        None => Ok(vec![conv_id.to_string()]),
    }
}

/// Several conversations that one chat was shredded into, named by whatever answer stands for
/// them.
///
/// Carried by `cs search`'s row, `cs show`'s transcript and `cs explain`'s verdict alike,
/// because all three now answer for the same unit and a reader has to be able to tell that the
/// unit was reconstructed. The fold is a heuristic over timestamps (see [`GAP_MS`]) and the
/// records really were separate HTTP requests, so a client that draws a sitting silently as one
/// conversation is asserting something Google never recorded.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Sitting {
    /// Every conversation folded into this answer, earliest first. The first opened it, and
    /// each is a real permanent id — nothing here is coined, which is what lets the fold be a
    /// heuristic without touching ADR 16.
    pub members: Vec<String>,
    /// The silence that delimited it. Carried so an answer can say what produced the fold
    /// rather than implying the export drew the boundary.
    pub gap_ms: i64,
}

impl Sitting {
    /// The sitting an id belongs to, or `None` when the conversation stands for itself.
    ///
    /// Built from [`resolve`] rather than beside it, so "is this a fold at all" is the one
    /// question `> 1` answers and three surfaces cannot come to three different views of a
    /// one-record sitting.
    pub fn of(conn: &Connection, conv_id: &str) -> rusqlite::Result<Option<Sitting>> {
        let members = resolve(conn, conv_id)?;
        Ok((members.len() > 1).then_some(Sitting { members, gap_ms: GAP_MS }))
    }
}

/// `('google-takeout', 'gemini-apps'), ('google-takeout', 'ai-mode')`.
///
/// Interpolated rather than bound: this is a compile-time constant, so it produces exactly one
/// statement text, and no value in it can ever come from a query.
fn pairs() -> String {
    ACTIVITY_LOGS
        .iter()
        .map(|(source, surface)| format!("('{source}', '{surface}')"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The distinct source ids named in [`ACTIVITY_LOGS`], in order.
fn sources() -> Vec<&'static str> {
    let mut out: Vec<&str> = Vec::new();
    for (source, _) in ACTIVITY_LOGS {
        if !out.contains(&source) {
            out.push(source);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINUTE: i64 = 60_000;

    /// An index holding `(id, surface, ended_at)` activity records, two messages each.
    fn corpus(records: &[(&str, &str, i64)]) -> Connection {
        let conn = crate::open(":memory:").unwrap();
        for (id, surface, ended_at) in records {
            conn.execute(
                "INSERT INTO conversation(id, source, native_id, surface, title, started_at,
                                          ended_at, msg_count, prose_count, user_turns)
                 VALUES (?1, 'google-takeout', ?1, ?2, ?1, ?3, ?3, 2, 2, 1)",
                params![id, surface, ended_at],
            )
            .unwrap();
        }
        conn
    }

    fn folded(conn: &Connection) -> Vec<(String, String, i64)> {
        conn.prepare("SELECT conv_id, sitting_id, msg_offset FROM conv_sitting ORDER BY conv_id")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    }

    #[test]
    fn records_closer_together_than_the_gap_become_one_sitting_opened_by_the_earliest() {
        let conn = corpus(&[
            ("a", "gemini-apps", 0),
            ("b", "gemini-apps", 20 * MINUTE),
            ("c", "gemini-apps", 35 * MINUTE),
        ]);
        ensure(&conn).unwrap();
        // Every gap here is under 30 minutes even though the first and last records are 35
        // apart: the rule is about silence between neighbours, not about how long someone
        // stayed. A two-hour conversation is one sitting so long as nobody walked away.
        assert_eq!(
            folded(&conn),
            [
                ("a".into(), "a".into(), 0),
                ("b".into(), "a".into(), 2),
                ("c".into(), "a".into(), 4),
            ]
        );
    }

    #[test]
    fn a_silence_at_least_as_long_as_the_gap_starts_a_new_sitting() {
        let conn = corpus(&[
            ("a", "gemini-apps", 0),
            ("b", "gemini-apps", 30 * MINUTE),
            ("c", "gemini-apps", 40 * MINUTE),
        ]);
        ensure(&conn).unwrap();
        assert_eq!(folded(&conn), [("b".into(), "b".into(), 0), ("c".into(), "b".into(), 2)]);
        // `a` is alone, so it is absent rather than a sitting of one.
        assert_eq!(members(&conn, "a").unwrap(), Vec::<String>::new());
        assert_eq!(members(&conn, "b").unwrap(), ["b", "c"]);
    }

    #[test]
    fn any_member_resolves_to_the_whole_sitting_rather_than_to_its_opener() {
        // The bug in chat-search-o1i.8, at the level it was actually wrong: `members` answers
        // for an opener, and a reader handed the *third* record of a sitting has no way to get
        // back to the other two. `Sitting::members` is on the wire, so this is an id a client
        // has been given and told is real.
        let conn = corpus(&[
            ("a", "gemini-apps", 0),
            ("b", "gemini-apps", MINUTE),
            ("c", "gemini-apps", 2 * MINUTE),
        ]);
        ensure(&conn).unwrap();
        for id in ["a", "b", "c"] {
            assert_eq!(resolve(&conn, id).unwrap(), ["a", "b", "c"], "resolving {id}");
        }
    }

    #[test]
    fn a_conversation_in_no_sitting_resolves_to_itself_alone() {
        // Which is what lets every caller write one loop. An id nothing knows about comes back
        // the same way rather than empty: "no such conversation" is the caller's sentence, and
        // an empty vec here would make it the fold's.
        let conn = corpus(&[("solo", "gemini-apps", 0)]);
        ensure(&conn).unwrap();
        assert_eq!(resolve(&conn, "solo").unwrap(), ["solo"]);
        assert_eq!(resolve(&conn, "chatgpt-export:nothing").unwrap(), ["chatgpt-export:nothing"]);
        assert!(Sitting::of(&conn, "solo").unwrap().is_none(), "one record is not a fold");
    }

    #[test]
    fn a_sitting_names_its_records_and_the_silence_that_delimited_them() {
        let conn = corpus(&[("a", "gemini-apps", 0), ("b", "gemini-apps", MINUTE)]);
        ensure(&conn).unwrap();
        let sitting = Sitting::of(&conn, "b").unwrap().expect("b is folded with a");
        assert_eq!(sitting.members, ["a", "b"]);
        // Reported rather than assumed: the boundary is this project's heuristic, not
        // something Google recorded, and a client cannot say so without the number.
        assert_eq!(sitting.gap_ms, GAP_MS);
    }

    #[test]
    fn two_products_used_in_the_same_minute_are_two_sittings() {
        // Asking Gemini something while a Search AI Mode answer is still on screen is two
        // conversations, and merging them would put two subjects on one row.
        let conn = corpus(&[("a", "gemini-apps", 0), ("b", "ai-mode", MINUTE)]);
        ensure(&conn).unwrap();
        assert!(folded(&conn).is_empty(), "different surfaces never share a sitting");
    }

    #[test]
    fn a_sitting_totals_what_its_members_hold() {
        let conn = corpus(&[("a", "gemini-apps", 0), ("b", "gemini-apps", MINUTE)]);
        ensure(&conn).unwrap();
        let got: (String, i64, i64, i64, i64) = conn
            .query_row(
                "SELECT id, ended_at, msg_count, prose_count, user_turns FROM sitting",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .unwrap();
        // Keyed on the opener but ending when the *last* record did, because that is the
        // instant a reader means by "when was this" and the one the row sorts by.
        assert_eq!(got, ("a".into(), MINUTE, 4, 4, 2));
    }

    #[test]
    fn a_source_that_is_not_an_activity_log_is_never_folded() {
        let conn = crate::open(":memory:").unwrap();
        for (id, source, surface) in [
            // A Workspace conversation is a real transcript with an id of its own, and it
            // arrives under the same source id as the records above.
            ("w1", "google-takeout", Some("gemini-workspace")),
            ("w2", "google-takeout", Some("gemini-workspace")),
            ("x1", "chatgpt-export", None),
            ("x2", "chatgpt-export", None),
        ] {
            conn.execute(
                "INSERT INTO conversation(id, source, native_id, surface, ended_at,
                                          msg_count, prose_count, user_turns)
                 VALUES (?1, ?2, ?1, ?3, 1000, 2, 2, 1)",
                params![id, source, surface],
            )
            .unwrap();
        }
        ensure(&conn).unwrap();
        assert!(folded(&conn).is_empty());
    }

    #[test]
    fn a_record_with_no_timestamp_cannot_be_placed_and_stands_alone() {
        // NotebookLM records no per-turn time anywhere in the export, and an activity surface
        // could yet arrive the same way. A record with no instant has no neighbours.
        let conn = crate::open(":memory:").unwrap();
        conn.execute(
            "INSERT INTO conversation(id, source, native_id, surface, msg_count, prose_count,
                                      user_turns)
             VALUES ('u', 'google-takeout', 'u', 'gemini-apps', 2, 2, 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO conversation(id, source, native_id, surface, ended_at, msg_count,
                                      prose_count, user_turns)
             VALUES ('v', 'google-takeout', 'v', 'gemini-apps', 0, 2, 2, 1)",
            [],
        )
        .unwrap();
        ensure(&conn).unwrap();
        assert!(folded(&conn).is_empty());
    }

    #[test]
    fn the_threshold_is_the_only_thing_that_decides_where_a_sitting_ends() {
        let conn = corpus(&[("a", "gemini-apps", 0), ("b", "gemini-apps", 20 * MINUTE)]);
        ensure(&conn).unwrap();
        // Two records, one pair of records, two answers. Nothing else about them changed, so
        // the fold cannot be a property of the data — which is the argument for computing it
        // here rather than in the importer, where it would have become an id.
        build(&conn, 10 * MINUTE).unwrap();
        assert!(folded(&conn).is_empty(), "20 minutes apart, folded at 10");
        build(&conn, 30 * MINUTE).unwrap();
        assert_eq!(folded(&conn).len(), 2, "the same records, folded at 30");
    }

    #[test]
    fn indexing_more_records_under_a_held_connection_rebuilds_the_fold() {
        // The case the fingerprint exists for: a TUI holds one connection open while
        // `cs index` rewrites the corpus underneath it. Without the check, every search after
        // the first would keep folding by the first one's picture of the world.
        let conn = corpus(&[("a", "gemini-apps", 0)]);
        ensure(&conn).unwrap();
        assert!(folded(&conn).is_empty());

        conn.execute(
            "INSERT INTO conversation(id, source, native_id, surface, ended_at, msg_count,
                                      prose_count, user_turns)
             VALUES ('b', 'google-takeout', 'b', 'gemini-apps', ?1, 2, 2, 1)",
            params![MINUTE],
        )
        .unwrap();
        ensure(&conn).unwrap();
        assert_eq!(folded(&conn).len(), 2, "the new record joined the sitting");
    }
}
