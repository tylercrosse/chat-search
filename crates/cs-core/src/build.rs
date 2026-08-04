//! Building an index without a reader ever seeing half of one, and what a reader is told
//! while a build runs.
//!
//! ADR 14 makes the JSON contract load-bearing and states the consequence: clients have to
//! treat "no index yet" and "index being rebuilt" as first-class states rather than as
//! transport errors. They could not. Measured from a second client, querying every 250 ms
//! while `cs index` rewrote the file underneath it (chat-search-me9.22, chat-search-me9.28):
//!
//! ```text
//! t = 0.0 s        818 conversations — complete
//! t = 1.8–4.4 s    exit 1, "index records no importer version, so it predates this schema"
//! t = 4.7–8.0 s    exit 0, 489 conversations
//! t = 8.5–9.3 s    exit 0, 678 conversations
//! after            818 conversations
//! ```
//!
//! Two failures, and the second is the worse one: for five seconds the answers were exit 0,
//! well formed, and quietly missing 40% of the corpus. So this module does two things.
//!
//! [`IndexBuild`] assembles the new index in a sibling file and renames it over the target,
//! which is atomic — the only two things a reader can see are the whole old index and the
//! whole new one, so the partial-answer window is gone rather than labelled.
//!
//! [`IndexState`] names what is left. "There is no index yet" and "one is being built right
//! now" are still different situations with different remedies, and telling a user to run the
//! command that is already running is its own kind of wrong answer.

use crate::index;
use rusqlite::{Connection, OpenFlags};
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};

/// Where a build assembles the new index. A sibling of the target, so the swap is a rename
/// within one directory and never degrades into a copy across a filesystem boundary.
pub fn building_path(db: &Path) -> PathBuf {
    sibling(db, ".building")
}

/// The file a build holds a lock on, and the only thing a reader consults to decide whether
/// one is running.
///
/// Separate from the half-built index next to it because the two cannot be one file: an
/// `flock` and SQLite's own locking do not coexist on macOS, and a build that locked the
/// database it was writing got `database is locked` from its own connection. So the
/// half-built file says a build *started* and this says one is *alive*.
fn claim_path(db: &Path) -> PathBuf {
    sibling(db, ".building.lock")
}

/// What is at the index path, from a reader's side.
///
/// The wire name is what a client branches on — never the sentence beside it, which stays
/// free to be reworded (ADR 12). Two of the four cannot be queried, and for those the name is
/// also the error `code` in the JSON body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexState {
    /// Nothing at the path and nothing building one. `cs index` is the remedy.
    NoIndex,
    /// Nothing to answer with yet, and a build is running. The remedy is to wait, and saying
    /// so is the point: the measured run told the user to run `cs index` while `cs index` was
    /// the process rewriting the file.
    Building,
    /// A complete index, with a newer one being built beside it. Answers are the previous
    /// build's answers *in full* — [`IndexBuild::commit`] swaps the file whole, so there is no
    /// such thing as a partial one — but they are one build behind, and a client that knows
    /// that can offer to ask again in a few seconds.
    Rebuilding,
    /// A complete index and no build running.
    Ready,
}

impl IndexState {
    /// The name this state travels under. Stable; the prose is not.
    pub fn as_str(self) -> &'static str {
        match self {
            IndexState::NoIndex => "no_index",
            IndexState::Building => "building",
            IndexState::Rebuilding => "rebuilding",
            IndexState::Ready => "ready",
        }
    }

    /// What a reader would find at `db` right now.
    pub fn of(db: &Path) -> Self {
        match (db.exists(), build_running(db)) {
            (true, true) => IndexState::Rebuilding,
            (true, false) => IndexState::Ready,
            (false, true) => IndexState::Building,
            (false, false) => IndexState::NoIndex,
        }
    }

    /// Whether there is an index to answer with, whatever else is going on.
    pub fn is_queryable(self) -> bool {
        matches!(self, IndexState::Ready | IndexState::Rebuilding)
    }
}

/// Is a build actually running, or is this the litter of one that was killed?
///
/// The builder holds an exclusive lock on its claim file for as long as it lives, so a lock
/// this can take is one no live process holds. Without the distinction the marker would lie
/// forever after the first `^C` during a rebuild — which is not a rare event — and a user with
/// no index would be told to wait for a build that had already died.
fn build_running(db: &Path) -> bool {
    let Ok(f) = File::open(claim_path(db)) else {
        return false; // no claim, no build
    };
    match f.try_lock_shared() {
        Ok(()) => false,
        Err(std::fs::TryLockError::WouldBlock) => true,
        // A filesystem that cannot lock cannot be interrogated. The file is still evidence a
        // build started, and reporting a dead build as running is the smaller lie than
        // reporting a build in progress as finished.
        Err(std::fs::TryLockError::Error(_)) => true,
    }
}

#[derive(Debug, thiserror::Error)]
pub enum BuildError {
    #[error("another `cs index` is already building {0}")]
    AlreadyBuilding(PathBuf),

    #[error("io error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
}

/// A rebuild in progress: the whole index assembled beside the live one and swapped over it
/// in a single rename.
///
/// The cost is peak disk — two indexes exist for the length of a build, ~660 MB at today's
/// size — and that is the trade ADR 1 makes cheap. The index is a pure function of the
/// archive, so the copy being built is worth nothing until it commits: dropping this without
/// [`IndexBuild::commit`] throws the half-built file away and leaves the live index exactly as
/// it was, which is also what a killed process leaves behind.
pub struct IndexBuild {
    target: PathBuf,
    temp: PathBuf,
    /// Held open, and locked, for the life of the build. Dropped with the struct, which is
    /// what makes a build that dies stop claiming to be running (see [`build_running`]).
    _claim: File,
    conn: Option<Connection>,
    committed: bool,
}

impl IndexBuild {
    /// Start a build, refusing if another one is already running.
    ///
    /// A killed build leaves its half-written file behind. That file is worth nothing — the
    /// archive can produce it again — so a fresh build deletes it rather than resuming or
    /// emptying it. Deleting is also the only thing that works: `CREATE TABLE IF NOT EXISTS`
    /// will not add a column a newer schema introduced, so a reused file is a rebuild that
    /// silently keeps the old shape.
    pub fn begin(target: &Path) -> Result<Self, BuildError> {
        let temp = building_path(target);
        let claim = claim_path(target);
        if build_running(target) {
            return Err(BuildError::AlreadyBuilding(target.to_path_buf()));
        }
        let claim = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .open(&claim)
            .map_err(|source| BuildError::Io { path: claim, source })?;
        // Whoever holds this is a live builder — a reader takes its shared lock and gives it
        // straight back.
        claim.try_lock().map_err(|_| BuildError::AlreadyBuilding(target.to_path_buf()))?;

        remove_database(&temp);
        let conn = index::open(temp.to_str().unwrap_or_default())?;
        // Stamped up front so that a build which writes no conversations — an archive with
        // nothing in it yet — still swaps in an index that reads as current rather than as one
        // predating the schema.
        index::record_importer_version(&conn)?;
        Ok(Self {
            target: target.to_path_buf(),
            temp,
            _claim: claim,
            conn: Some(conn),
            committed: false,
        })
    }

    /// The connection to write the new index through. It addresses the half-built file, which
    /// no reader will ever open.
    pub fn conn(&mut self) -> &mut Connection {
        self.conn.as_mut().expect("a build holds its connection until it commits")
    }

    /// Swap the finished index over the live one.
    ///
    /// What gets renamed has to be one self-contained file, which is why the write-ahead log
    /// is checkpointed away first: a `-wal` is found by path and not by inode, so a renamed
    /// database with a stale log beside it is how an atomic swap still hands the next reader a
    /// torn one. The shipped index is therefore in rollback-journal mode, and readers open it
    /// read-only ([`open_for_read`]), so nothing writes a log against it either.
    ///
    /// Measured at 257–367 ms over a 334 MB index — the checkpoint and the rename together,
    /// against 14–26 s of import.
    pub fn commit(mut self) -> Result<(), BuildError> {
        let conn = self.conn.take().expect("a build commits once");
        conn.pragma_update(None, "journal_mode", "DELETE")?;
        drop(conn);

        std::fs::rename(&self.temp, &self.target)
            .map_err(|source| BuildError::Io { path: self.temp.clone(), source })?;
        // The old index's sidecars, orphaned by the rename. They are inert — journal mode is a
        // property of the database header and the new file's says rollback — but a `-wal`
        // belonging to a file that no longer exists reads as corruption to whoever finds it.
        for suffix in ["-wal", "-shm"] {
            let _ = std::fs::remove_file(sibling(&self.target, suffix));
        }
        // Last, so that the window in which a reader is told a build is running ends only
        // once the new index is the one being read.
        let _ = std::fs::remove_file(claim_path(&self.target));
        self.committed = true;
        Ok(())
    }
}

impl Drop for IndexBuild {
    /// An uncommitted build leaves nothing behind — not the half-built index, and not the
    /// claim that would otherwise tell every reader a build was still running.
    fn drop(&mut self) {
        if self.committed {
            return;
        }
        self.conn.take();
        remove_database(&self.temp);
        let _ = std::fs::remove_file(claim_path(&self.target));
    }
}

fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let mut p = path.as_os_str().to_os_string();
    p.push(suffix);
    PathBuf::from(p)
}

/// A database file and every journal SQLite may have left beside it.
fn remove_database(path: &Path) {
    for suffix in ["", "-wal", "-shm", "-journal"] {
        let _ = std::fs::remove_file(sibling(path, suffix));
    }
}

/// Why an index could not be read, in a form a client can branch on.
///
/// [`Unreadable::code`] is the contract and the message is not: three genuinely different
/// conditions used to arrive as one English sentence on stderr, which is what left a second
/// client grepping another program's prose to work out what had happened
/// (chat-search-me9.22).
#[derive(Debug, thiserror::Error)]
pub enum Unreadable {
    #[error("no index at {path} — run `cs index` to build one")]
    NoIndex { path: PathBuf },

    #[error("no index at {path} yet — `cs index` is building one now")]
    Building { path: PathBuf },

    /// Built by a different importer version, or old enough not to record one. Telling those
    /// apart from a file that is not a database at all is chat-search-me9.29.
    #[error("{0}")]
    Stale(String),

    #[error("{path} could not be opened: {source}")]
    Unopenable {
        path: PathBuf,
        #[source]
        source: rusqlite::Error,
    },
}

impl Unreadable {
    /// The stable name a client branches on.
    pub fn code(&self) -> &'static str {
        match self {
            Unreadable::NoIndex { .. } => IndexState::NoIndex.as_str(),
            Unreadable::Building { .. } => IndexState::Building.as_str(),
            Unreadable::Stale(_) => "index_stale",
            Unreadable::Unopenable { .. } => "index_unreadable",
        }
    }
}

/// An open index, and what was true of the file when it opened.
#[derive(Debug)]
pub struct Reader {
    pub conn: Connection,
    /// [`IndexState::Ready`] or [`IndexState::Rebuilding`] — the other two never get this far.
    pub state: IndexState,
}

/// Open the index for reading, or say why not.
///
/// Read-only, for two reasons that both showed up as silent wrong answers. SQLite creates a
/// database at any path it is handed for writing, so a mistyped `--db` became a new empty
/// index that reported nothing matched (chat-search-me9.22). And a read-only connection to a
/// rollback-mode database — which is what [`IndexBuild::commit`] leaves behind — writes no
/// journal of its own, so nothing appears at `<db>-wal` for a swap to strand there. (An index
/// built before that change is still in WAL mode, and a reader of one does create the
/// sidecars; the first rebuild after this retires them.)
pub fn open_for_read(db: &Path) -> Result<Reader, Unreadable> {
    let state = IndexState::of(db);
    match state {
        IndexState::NoIndex => return Err(Unreadable::NoIndex { path: db.to_path_buf() }),
        IndexState::Building => return Err(Unreadable::Building { path: db.to_path_buf() }),
        IndexState::Ready | IndexState::Rebuilding => {}
    }
    let conn = Connection::open_with_flags(
        db,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|source| Unreadable::Unopenable { path: db.to_path_buf(), source })?;
    index::ensure_current(&conn).map_err(Unreadable::Stale)?;
    Ok(Reader { conn, state })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Conversation, Kind, Message, Role, Titles};

    fn tmpdir() -> PathBuf {
        let p = std::env::temp_dir().join(format!("cs-build-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    /// One conversation, titled, so which build answered a query is identifiable.
    fn conv(native_id: &str, title: &str) -> Conversation {
        Conversation {
            source: "codex".into(),
            native_id: native_id.into(),
            titles: Titles { custom: Some(title.into()), ..Default::default() },
            cwd: None,
            git_branch: None,
            model: None,
            surface: None,
            forked_from_native_id: None,
            head_native_id: None,
            messages: vec![Message {
                native_id: "m1".into(),
                parent_native_id: None,
                thread_key: "t".into(),
                is_sidechain: false,
                is_error: false,
                seq: 0,
                role: Role::User,
                kind: Kind::Prose,
                ts: Some(1_700_000_000_000),
                text: "the borrow checker again".into(),
            }],
        }
    }

    fn titles(db: &Path) -> Vec<String> {
        let r = open_for_read(db).expect("a readable index");
        let mut stmt = r.conn.prepare("SELECT title FROM conversation ORDER BY id").unwrap();
        let rows = stmt.query_map([], |row| row.get::<_, String>(0)).unwrap();
        rows.map(|t| t.unwrap()).collect()
    }

    fn build(db: &Path, convs: &[Conversation]) {
        let mut b = IndexBuild::begin(db).unwrap();
        index::write_conversations(b.conn(), convs.iter()).unwrap();
        b.commit().unwrap();
    }

    #[test]
    fn a_build_in_progress_is_invisible_at_the_index_path() {
        let dir = tmpdir();
        let db = dir.join("index.db");
        build(&db, &[conv("a", "first")]);

        let mut second = IndexBuild::begin(&db).unwrap();
        index::write_conversations(second.conn(), [conv("b", "second")].iter()).unwrap();
        // Written and committed to its own file, but not swapped in: the reader is still
        // answering out of the previous index, in full, rather than out of half a corpus.
        assert_eq!(titles(&db), vec!["first"]);
        assert_eq!(IndexState::of(&db), IndexState::Rebuilding);

        second.commit().unwrap();
        assert_eq!(titles(&db), vec!["second"]);
        assert_eq!(IndexState::of(&db), IndexState::Ready);
    }

    #[test]
    fn an_abandoned_build_leaves_the_live_index_untouched() {
        let dir = tmpdir();
        let db = dir.join("index.db");
        build(&db, &[conv("a", "first")]);

        {
            let mut doomed = IndexBuild::begin(&db).unwrap();
            index::write_conversations(doomed.conn(), [conv("b", "second")].iter()).unwrap();
        }
        assert_eq!(titles(&db), vec!["first"]);
        assert!(!building_path(&db).exists(), "the half-built file outlived its build");
        assert!(!claim_path(&db).exists(), "the claim outlived the build that made it");
        assert_eq!(IndexState::of(&db), IndexState::Ready);
    }

    #[test]
    fn a_first_build_says_building_rather_than_no_index() {
        let dir = tmpdir();
        let db = dir.join("index.db");
        assert_eq!(IndexState::of(&db), IndexState::NoIndex);

        let building = IndexBuild::begin(&db).unwrap();
        assert_eq!(IndexState::of(&db), IndexState::Building);
        // The distinction is the point: one of these says to run `cs index`, the other says
        // one is already running and the wait is finite.
        assert_eq!(open_for_read(&db).unwrap_err().code(), "building");
        drop(building);
        assert_eq!(open_for_read(&db).unwrap_err().code(), "no_index");
    }

    #[test]
    fn the_litter_of_a_killed_build_is_not_a_running_build() {
        let dir = tmpdir();
        let db = dir.join("index.db");
        build(&db, &[conv("a", "first")]);
        // What `^C` during a rebuild leaves behind. Without the lock these read as a build
        // that never ends, and every answer for the rest of time is labelled stale.
        std::fs::write(building_path(&db), b"half an index").unwrap();
        std::fs::write(claim_path(&db), b"").unwrap();
        assert_eq!(IndexState::of(&db), IndexState::Ready);
    }

    #[test]
    fn two_builds_at_once_are_refused_rather_than_interleaved() {
        let dir = tmpdir();
        let db = dir.join("index.db");
        let first = IndexBuild::begin(&db).unwrap();
        assert!(matches!(IndexBuild::begin(&db), Err(BuildError::AlreadyBuilding(_))));
        drop(first);
        assert!(IndexBuild::begin(&db).is_ok(), "the refusal outlived the build holding the lock");
    }

    #[test]
    fn reading_a_path_with_no_index_does_not_create_one() {
        let dir = tmpdir();
        let db = dir.join("typo.db");
        assert_eq!(open_for_read(&db).unwrap_err().code(), "no_index");
        assert!(!db.exists(), "a mistyped --db path became an empty index");
    }

    #[test]
    fn the_swapped_in_index_carries_no_write_ahead_log() {
        let dir = tmpdir();
        let db = dir.join("index.db");
        build(&db, &[conv("a", "first")]);
        // A `-wal` is found by path rather than by inode, so one left beside a renamed
        // database is a log belonging to a file that no longer exists.
        for suffix in ["-wal", "-shm"] {
            assert!(!sibling(&db, suffix).exists(), "{suffix} survived the swap");
        }
    }

    #[test]
    fn an_index_from_a_different_importer_version_is_stale_rather_than_missing() {
        let dir = tmpdir();
        let db = dir.join("index.db");
        build(&db, &[conv("a", "first")]);
        {
            let conn = index::open(db.to_str().unwrap()).unwrap();
            conn.execute("UPDATE build_info SET value = '0' WHERE key = 'importer_version'", [])
                .unwrap();
        }
        assert_eq!(open_for_read(&db).unwrap_err().code(), "index_stale");
    }
}
