//! When a query's answers happened — the distribution, not the page.
//!
//! `poc/ui/DESIGN-BRIEF.md` gives the window a fifth region: "a timeline of whatever survives the
//! current filters, with a scrubber". The prototype draws one mark per conversation because it
//! had all 3,059 of them in memory. A client that spawns `cs` does not: it holds a `--limit 60`
//! page, and the page is a *biased* sample of this axis, because ranking is not chronological.
//! A timeline over the top sixty ranked rows is a different picture from a timeline over the 354
//! that matched, and it is wrong in the direction nobody can see.
//!
//! So the counting happens here and only counts cross the wire. Two properties follow, and both
//! are why this is a bucketed histogram rather than a list of instants:
//!
//! - **The reply is a fixed size.** [`Timeline::buckets`] is a picture's worth of numbers
//!   whatever the corpus does, where one row per conversation grows with the archive forever —
//!   on a path that runs per keystroke.
//! - **A bucket is a whole number of civil days**, walked with [`crate::time::shift_days_in`]
//!   rather than divided out of a span, so a bucket is the same number of *days* either side of
//!   a DST boundary instead of the same number of milliseconds.
//!
//! # What it draws, and what it deliberately does not
//!
//! **The bars ignore `date:`.** [`Timeline::buckets`] counts everything surviving every *other*
//! filter, which is `poc/ui`'s `visible(true)` and the one subtlety worth keeping: a timeline
//! that also filtered itself by the selected window would draw a solid block and nothing else,
//! and would never tell you what widening would get you. The window is drawn *over* that
//! picture ([`Timeline::window`]) rather than applied to it.
//!
//! **The axis is the corpus, not the query.** `from` and `until` are the whole index's dated
//! span, so the axis does not move while somebody types — a scrubber whose coordinate system
//! changed on every keystroke would be a scrubber you cannot aim.
//!
//! **`matches` is the free text, `conversations` is not.** The two series answer the two
//! questions the brief names at once: "when was I working on this" is every row the filters
//! keep, and "when did this query land" is the ones a term matched. For a query with no
//! searchable text there is nothing to have matched, and `matches` is zero throughout rather
//! than a copy of the row above it.
//!
//! # The scrubber's half of the grammar
//!
//! A rail hands each chip the query text clicking it produces, which is what keeps a client
//! from ever assembling an `agent:` token. A scrubber cannot be enumerated that way — a drag is
//! two instants out of a continuum — so [`drag`] is the same trade made the other way round:
//! hand over two instants, get back the whole query text. `docs/TUI-DESIGN.md` §5 is why it is
//! not simply spelled out client-side: `Window::value_in` rounds each edge outward to a whole
//! second and writes a midnight as a bare date, and a second renderer of those rules in a
//! language that cannot link this crate is the local-date bug wearing a new hat.

use std::time::Instant;

use rusqlite::types::Value;
use rusqlite::{params_from_iter, Connection};
use serde::Serialize;

use crate::build::Reader;
use crate::facets::AllChip;
use crate::query::{Facet, Query, Window};
use crate::search::SearchOptions;

/// Contract version, moving under `docs/JSON-CONTRACT.md`'s rule — a field that changes
/// *meaning*, never an addition.
const V: u32 = 1;

/// How many buckets a caller gets when it does not say.
///
/// The count is the picture's resolution and therefore a property of a drawing surface, not of
/// the corpus — but it lives here rather than in each client for the reason every other shared
/// number does. At the 1,280 days this archive spans it makes a bucket eight days wide, and at a
/// 900 pt window it makes a bar about five points wide, which is the mockup's mark.
pub const BUCKETS: usize = 180;

/// The time distribution of one query, and the window it currently names.
#[derive(Debug, Clone, Serialize)]
pub struct Timeline {
    pub v: u32,
    /// The query as parsed, so a client can tell which of several replies it is holding — the
    /// same field, for the same reason, as the search envelope's.
    pub query: String,
    /// How long this took, in milliseconds, rounded to two places by the same rule the search
    /// envelope uses.
    pub ms: f64,
    /// [`crate::IndexState`] as its wire name.
    pub index_state: &'static str,
    /// The axis: the first bucket's start and the last one's end, half-open. **The corpus's
    /// dated span, not this query's** — see the module docs.
    pub from: i64,
    pub until: i64,
    /// The same two instants as local days, for labelling the ends of the axis.
    ///
    /// On the wire for the reason [`crate::Group::ended_date`] is: the local-date bug happened
    /// because three clients each derived the day themselves, and a client that formats an
    /// instant is a fourth. Null exactly when there are no buckets.
    pub from_date: Option<String>,
    pub until_date: Option<String>,
    /// How many civil days one bucket covers. Carried so a client can label the axis without
    /// dividing the span by the bucket count and getting 5.97 days.
    pub bucket_days: i64,
    /// Source ids in the order [`Bucket::sources`] counts them, sorted so a stacked bar does not
    /// reshuffle between keystrokes.
    pub sources: Vec<String>,
    /// Oldest first, abutting, covering the whole axis. Empty when the index holds no dated
    /// conversation at all.
    pub buckets: Vec<Bucket>,
    /// Conversations the filters keep that have no `ended_at` and so are in no bucket. Four of
    /// this corpus's 4,426, and carried for the reason `DirFacet.undirected` is: a picture that
    /// silently drops what it cannot place is a picture claiming to be everything.
    pub undated: usize,
    /// Of everything the bars draw, how many are inside the `date:` window — the number the bars
    /// themselves refuse to say, because they are drawn with the window left out. Free text
    /// ignored, like the bars: this is "how much was going on then".
    ///
    /// Equal to the sum of `conversations` when the query names no window.
    pub in_range: usize,
    /// How many conversations the query selects with `limit` ignored, which is "how much of it
    /// this query found" and is the *same number* [`crate::Answer::total`] settles to, spelled
    /// the same way for the same reason.
    ///
    /// Counted here rather than read off the search because the two are two processes and can
    /// land a keystroke apart, and a drawer disagreeing with the footer above it is the failure
    /// this whole module exists to avoid. That they agree is a claim a test holds to, and it is
    /// the pin that keeps this predicate and `count_matching`'s from drifting.
    pub total: usize,
    /// The window the query's `date:` tokens resolve to, or null when it names none — and also
    /// null when the only one it names is negated, because the complement of a window is not a
    /// rectangle and drawing it as one would misplace it exactly.
    pub window: Option<Selected>,
    /// The click that clears the selection: no `date:` token at all. Spelled as a rail's All
    /// chip because that is what it is.
    pub all: AllChip,
    /// What a drag would write, and only when one was asked about — see [`drag`].
    pub drag: Option<Drag>,
}

/// One bar: a span of time and what fell in it.
#[derive(Debug, Clone, Serialize)]
pub struct Bucket {
    /// Half-open, `[from, until)`, so consecutive buckets tile without an instant falling in
    /// both or neither — the same reading as [`Window`] and as `date:`'s own span.
    pub from: i64,
    pub until: i64,
    /// Rows surviving every filter but `date:`, free text ignored.
    pub conversations: usize,
    /// Of a searchable query's matches, how many landed here. Never more than `conversations`.
    pub matches: usize,
    /// `conversations` broken down by source, parallel to [`Timeline::sources`] and summing to
    /// it. Carried rather than derived so a client can stack the bar in the palette's own source
    /// hues without a second census.
    pub sources: Vec<usize>,
}

/// The `date:` window in force, as instants and as the text that names them.
#[derive(Debug, Clone, Serialize)]
pub struct Selected {
    /// Null for an open edge: `date:<7d` reaches to now and beyond, and a client that read a
    /// missing bound as the end of the axis would draw a rectangle that stops where the data
    /// does rather than where the filter does.
    pub from: Option<i64>,
    pub until: Option<i64>,
    /// The window written back as a `date:` value — `2026-07-28..2026-08-02`. Null for a window
    /// this grammar cannot spell, which is one bounded at neither end.
    pub value: Option<String>,
}

/// What a drag writes.
#[derive(Debug, Clone, Serialize)]
pub struct Drag {
    /// The `date:` value the two instants are typed as, or null when they name no span — which
    /// is what a drag narrower than the bucket it started in produces.
    pub value: Option<String>,
    /// The whole query text after the drag. Paste it into the box; do not splice a token.
    ///
    /// A drag onto the window already in force takes it back off, because this is
    /// [`Query::toggling`] and that is what every chip in this interface does. It is also the
    /// only gesture that clears a selection without a control of its own.
    pub query: String,
}

/// The distribution of `query`, in about `buckets` bars.
///
/// Takes a [`Reader`] rather than a bare `Connection` for the reason [`crate::answer`] does:
/// `index_state` then lands by construction instead of by a caller remembering it. Takes the
/// same [`SearchOptions`] the search was given, `now_ms` included — a `date:` filter resolved
/// against a second instant describes a second set, and a drawer under a list is the one place
/// that has to be showing the same one.
///
/// [`SearchOptions::field`] and `include_off_path` are that rule's other two edges, and they are
/// the ones a caller forgets, because nothing about a picture of tool traffic looks different
/// from a picture of prose. `cs timeline` grew flags for both under `chat-search-me9.8.25`; the
/// pin is `the_drawer_reads_whichever_field_the_search_beside_it_was_asked_for` below. `limit` is
/// the one field here ignores, since this counts what the query selects with the page left out.
pub fn timeline(
    reader: &Reader,
    query: &Query,
    opts: &SearchOptions,
    buckets: usize,
    dragged: Option<(i64, i64)>,
) -> rusqlite::Result<Timeline> {
    let started = Instant::now();
    let conn = &reader.conn;
    crate::sittings::ensure(conn)?;

    // The picture is of everything the *other* filters keep. Reparsed through the grammar rather
    // than assembled, and with this query's own reading of its last word — see
    // `Query::without_facet`.
    let undated_query = query.without_facet(Facet::Date);
    let axis = span(conn)?;
    let edges = axis.and_then(|(first, last)| walk(first, last, buckets));

    let ask = Ask { drawn: &undated_query, asked: query, opts, edges: edges.as_ref() };
    let mut counts = Counts::new(edges.as_ref().map_or(0, |e| e.stops.len().saturating_sub(1)));
    let sources = pool(conn, &ask, &mut counts)?;
    hits(conn, &ask, &mut counts)?;

    let stops = edges.as_ref().map_or(&[][..], |e| &e.stops);
    let bars = counts.bars(stops, sources.len());
    // Only when there is an axis. `local_ymd(0)` is a real day — 1970-01-01 — and an empty index
    // labelled with it is the one wrong answer a client cannot tell from a right one.
    let day = |at: Option<&i64>| at.copied().and_then(crate::time::local_ymd);

    Ok(Timeline {
        v: V,
        query: query.raw().to_string(),
        ms: crate::answer::elapsed_ms(started),
        index_state: reader.state.as_str(),
        from: stops.first().copied().unwrap_or(0),
        until: stops.last().copied().unwrap_or(0),
        from_date: day(stops.first()),
        until_date: day(stops.last()),
        bucket_days: edges.as_ref().map_or(0, |e| e.days),
        sources,
        buckets: bars,
        undated: counts.undated,
        in_range: counts.in_range,
        total: counts.total_in_range,
        window: selected(query, opts.now_ms),
        all: AllChip {
            selected: query.selection(Facet::Date).is_empty(),
            query: query.without(Facet::Date),
        },
        drag: dragged.map(|(from, until)| drag(query, from, until)),
    })
}

/// What a drag from one instant to another writes into the query line.
///
/// The whole of the scrubber's grammar, and the reason it is here rather than in the client:
/// `Window::value_in` rounds each edge outward to a whole second and writes a midnight as a bare
/// date, and a `date:` value assembled anywhere else is a second, partial renderer of this
/// grammar (`docs/TUI-DESIGN.md` §5).
///
/// The two instants are taken in whichever order the pointer visited them. A window that names
/// no span — the ends equal, or a drag that never left the bucket it started in — clears the
/// filter instead of writing an empty one, which is `poc/ui`'s "a drag under 1% of the span
/// clears the selection" arrived at through the grammar rather than through a magic fraction.
pub fn drag(query: &Query, a: i64, b: i64) -> Drag {
    let window = Window { from: Some(a.min(b)), until: Some(a.max(b)) };
    match window.value() {
        Some(value) => Drag { query: query.toggling(Facet::Date, &value), value: Some(value) },
        None => Drag { value: None, query: query.without(Facet::Date) },
    }
}

/// The window the query's positive `date:` tokens resolve to.
///
/// Two of them intersect, which is why this is a fold rather than a first: `Query::toggling`
/// replaces rather than widens for exactly this reason, so a hand-typed pair is the only way to
/// get here with two and the honest drawing of it is the overlap.
fn selected(query: &Query, now_ms: i64) -> Option<Selected> {
    let mut from: Option<i64> = None;
    let mut until: Option<i64> = None;
    let mut any = false;
    for (window, negated) in query.date_windows(now_ms) {
        if negated {
            continue;
        }
        any = true;
        from = [from, window.from].into_iter().flatten().max();
        until = [until, window.until].into_iter().flatten().min();
    }
    let window = Window { from, until };
    any.then(|| Selected { from, until, value: window.value() })
}

/// The one ask both counting passes run under.
///
/// Two queries and not one, which is the whole arrangement in a struct: `drawn` has the `date:`
/// tokens stripped and is what both statements *filter* by, so the bars are not narrowed by the
/// window they are drawn under; `asked` still carries them and is what the "inside the window"
/// column is built from. A signature that took one query could not say which, and the two are a
/// filter and a picture of it.
struct Ask<'a> {
    drawn: &'a Query,
    asked: &'a Query,
    opts: &'a SearchOptions,
    edges: Option<&'a Edges>,
}

/// The bucket edges, and how many days one bucket is.
struct Edges {
    /// `buckets + 1` instants, oldest first.
    stops: Vec<i64>,
    days: i64,
}

impl Edges {
    /// Which bucket an instant falls in. Half-open, so the last stop belongs to no bucket and an
    /// instant past the axis is clamped into the last one rather than dropped — the axis is
    /// taken before the counting queries run, and a conversation written to the index in between
    /// is a bar being off by one bucket rather than a row vanishing from the picture.
    fn bucket(&self, ms: i64) -> usize {
        let after = self.stops.partition_point(|&stop| stop <= ms);
        after.saturating_sub(1).min(self.stops.len().saturating_sub(2))
    }
}

/// The corpus's dated span, or `None` when it holds no dated conversation.
///
/// Every conversation rather than the sitting-folded rows: the earliest and latest instants are
/// the same either way, and the axis is a statement about the archive rather than about a query.
fn span(conn: &Connection) -> rusqlite::Result<Option<(i64, i64)>> {
    let bounds: (Option<i64>, Option<i64>) = conn
        .prepare_cached("SELECT MIN(ended_at), MAX(ended_at) FROM conversation")?
        .query_row([], |r| Ok((r.get(0)?, r.get(1)?)))?;
    Ok(match bounds {
        (Some(first), Some(last)) => Some((first, last)),
        _ => None,
    })
}

/// Bucket edges covering `first..=last`, aligned to local midnights and a whole number of days
/// wide.
///
/// Days rather than milliseconds because a bucket is read as a stretch of the calendar, and
/// because `crate::time::shift_days_in` is the one place this project derives one: an axis
/// divided out of a span would drift an hour twice a year and put the same conversation in
/// different buckets before and after a clock change.
fn walk(first: i64, last: i64, want: usize) -> Option<Edges> {
    let tz = chrono::Local;
    let start = crate::time::day_start_in(&tz, first)?;
    // The end of the day the last conversation landed on, so that day is whole rather than a
    // sliver at the right edge.
    let end = crate::time::shift_days_in(&tz, crate::time::day_start_in(&tz, last)?, 1)?;
    // Rounded, not divided: a span holding a DST change is a whole number of days plus or minus
    // an hour, and truncating that is a day lost off the axis every autumn.
    let days = (((end - start) as f64 / 86_400_000.0).round() as i64).max(1);
    let want = want.max(1) as i64;
    let step = ((days + want - 1) / want).max(1);

    let mut stops = vec![start];
    while *stops.last().expect("seeded above") < end {
        let next = crate::time::shift_days_in(&tz, *stops.last().expect("seeded above"), step)?;
        // A zone arithmetic that did not advance would spin here forever rather than fail.
        if next <= *stops.last().expect("seeded above") {
            return None;
        }
        stops.push(next);
    }
    Some(Edges { stops, days: step })
}

/// The tallies both queries pour into, so the two passes share one bucketing rule.
struct Counts {
    conversations: Vec<usize>,
    matches: Vec<usize>,
    /// `[bucket][source]`, filled once the source order is known.
    by_source: Vec<Vec<usize>>,
    undated: usize,
    in_range: usize,
    total_in_range: usize,
}

impl Counts {
    fn new(buckets: usize) -> Self {
        Counts {
            conversations: vec![0; buckets],
            matches: vec![0; buckets],
            by_source: Vec::new(),
            undated: 0,
            in_range: 0,
            total_in_range: 0,
        }
    }

    fn bars(&self, stops: &[i64], sources: usize) -> Vec<Bucket> {
        stops
            .windows(2)
            .enumerate()
            .map(|(i, edge)| Bucket {
                from: edge[0],
                until: edge[1],
                conversations: self.conversations[i],
                matches: self.matches[i],
                sources: self.by_source.get(i).cloned().unwrap_or_else(|| vec![0; sources]),
            })
            .collect()
    }
}

/// Every row the filters keep, placed on the axis and broken down by source.
///
/// Returns the source order the breakdown is in. Sorted by id, which is [`crate::inventory`]'s
/// order and for the same reason: a stack that reordered itself when a keystroke emptied one
/// source would be a picture rearranging under the reader.
///
/// One row per *sitting* rather than per conversation, joined exactly as `count_filtered` joins,
/// so the bars and the number beside the list are counting the same things. The instant is the
/// row's own end — a sitting's, when it stands for one — which is the instant the list draws
/// beside it.
fn pool(conn: &Connection, ask: &Ask, counts: &mut Counts) -> rusqlite::Result<Vec<String>> {
    let mut binds: Vec<Value> = Vec::new();
    let filters = crate::search::filter_sql(ask.drawn, ask.opts.now_ms, &mut binds);
    let inside = crate::search::date_sql(ask.asked, ask.opts.now_ms, &mut binds);
    let sql = format!(
        "SELECT c.source, {ended}, {inside}
         FROM conversation c
         {join}
         WHERE {openers}{filters}",
        ended = crate::sittings::total("ended_at"),
        inside = inside.as_deref().unwrap_or("1"),
        join = crate::sittings::OF_CONVERSATION,
        openers = crate::sittings::OPENERS_ONLY,
    );

    let mut ids: Vec<String> = Vec::new();
    let mut rows: Vec<(String, Option<i64>, bool)> = Vec::new();
    let mut stmt = conn.prepare_cached(&sql)?;
    let mut cursor = stmt.query(params_from_iter(binds.iter()))?;
    while let Some(row) = cursor.next()? {
        let source: String = row.get(0)?;
        if !ids.contains(&source) {
            ids.push(source.clone());
        }
        rows.push((source, row.get(1)?, row.get::<_, i64>(2)? != 0));
    }
    ids.sort();

    counts.by_source = vec![vec![0; ids.len()]; counts.conversations.len()];
    for (source, ended, in_window) in rows {
        counts.in_range += usize::from(in_window);
        let Some(ended) = ended else {
            counts.undated += 1;
            continue;
        };
        let Some(edges) = ask.edges else { continue };
        let bucket = edges.bucket(ended);
        counts.conversations[bucket] += 1;
        if let Some(at) = ids.iter().position(|id| *id == source) {
            counts.by_source[bucket][at] += 1;
        }
    }
    Ok(ids)
}

/// The same rows again, narrowed to the ones a term matched.
///
/// `count_matching`'s predicate with the count replaced by the instants, so the number of groups
/// this walks past *is* that function's answer for the same ask. The row's end comes off the
/// sitting rather than off the matching member: a nine-record sitting where only the first
/// record matched still ends when the sitting ended, and drawing the tick under the bar it
/// belongs to matters more than which record carried the term.
fn hits(conn: &Connection, ask: &Ask, counts: &mut Counts) -> rusqlite::Result<()> {
    if !ask.drawn.is_searchable() {
        // Nothing was ranked, so nothing "landed" anywhere — but the rows are still an answer,
        // and the list below is showing them. `in_range` covers that; a `matches` series copied
        // off the bars above it would claim a search happened.
        counts.total_in_range = counts.in_range;
        return Ok(());
    }
    let table = ask.opts.field.table();
    let mut binds = vec![
        Value::Text(ask.drawn.match_expr()),
        Value::Integer(ask.opts.include_off_path as i64),
    ];
    let filters = crate::search::filter_sql(ask.drawn, ask.opts.now_ms, &mut binds);
    let inside = crate::search::date_sql(ask.asked, ask.opts.now_ms, &mut binds);
    let sql = format!(
        "SELECT MAX(ifnull(grp.ended_at, c.ended_at)), MAX({inside})
         FROM {table}
         JOIN message m      ON m.rowid = {table}.rowid
         JOIN conversation c ON c.id = m.conv_id
         {join}
         LEFT JOIN sitting grp ON grp.id = {group_id}
         WHERE {table} MATCH ?1
           AND (?2 = 1 OR m.on_head_path = 1){filters}
         GROUP BY {group_id}",
        inside = inside.as_deref().unwrap_or("1"),
        join = crate::sittings::OF_MESSAGE,
        group_id = crate::sittings::GROUP_ID,
    );

    let mut stmt = conn.prepare_cached(&sql)?;
    let mut cursor = stmt.query(params_from_iter(binds.iter()))?;
    while let Some(row) = cursor.next()? {
        let ended: Option<i64> = row.get(0)?;
        // A group survives the window when any of its records is inside it, which is what the
        // predicate does when it is a `WHERE` rather than a column.
        if row.get::<_, Option<i64>>(1)?.unwrap_or(0) != 0 {
            counts.total_in_range += 1;
        }
        let (Some(ended), Some(edges)) = (ended, ask.edges) else { continue };
        counts.matches[edges.bucket(ended)] += 1;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build::IndexState;
    use crate::model::{Conversation, Kind, Message, Role, Titles};
    use crate::search::Field;

    /// 2026-08-05T00:00:00Z, later than every fixture instant so nothing is dated in the future.
    const NOW: i64 = 1_785_888_000_000;
    const DAY: i64 = 86_400_000;

    fn opts() -> SearchOptions {
        SearchOptions { limit: 10, ..SearchOptions::new(NOW) }
    }

    /// One conversation, at one instant, made of one message per text given.
    ///
    /// `ended_at` is the indexer's own answer rather than a column set here, which is what makes
    /// these tests about the timeline instead of about a fixture: the axis is read out of the
    /// same column `date:` filters on.
    fn at(source: &str, id: &str, ended: i64, texts: &[&str]) -> Conversation {
        let messages = texts
            .iter()
            .enumerate()
            .map(|(i, text)| Message {
                native_id: format!("m{i}"),
                parent_native_id: (i > 0).then(|| format!("m{}", i - 1)),
                thread_key: "main".into(),
                is_sidechain: false,
                is_error: false,
                seq: i as i64,
                role: Role::User,
                kind: Kind::Prose,
                model: None,
                // The last message is what `ended_at` becomes, so the conversation ends where
                // it was asked to.
                ts: Some(ended - (texts.len() - 1 - i) as i64),
                text: (*text).into(),
            })
            .collect();
        Conversation {
            source: source.into(),
            native_id: id.into(),
            titles: Titles { custom: Some(id.into()), ..Default::default() },
            cwd: Some("/Users/x/dev/chat-search".into()),
            git_branch: None,
            declared_model: None,
            surface: None,
            forked_from_native_id: None,
            head_native_id: None,
            messages,
        }
    }

    fn reader(convs: &[Conversation]) -> Reader {
        let mut conn = crate::open(":memory:").unwrap();
        crate::write_conversations(&mut conn, convs.iter()).unwrap();
        Reader { conn, state: IndexState::Ready }
    }

    /// Forty days of conversations, one every second day, alternating source, with a `rust` in
    /// every third one — long enough that a bucket is one day and short enough to count by hand.
    fn corpus() -> Vec<Conversation> {
        (0..20)
            .map(|i| {
                let source = if i % 2 == 0 { "codex" } else { "claude-code" };
                let text = if i % 3 == 0 { "rust lifetimes" } else { "python imports" };
                at(source, &format!("c{i}"), NOW - (40 - i * 2) * DAY, &[text])
            })
            .collect()
    }

    /// The same forty days with tool traffic hung off them, plus one conversation whose only
    /// tool call is on a branch that was edited away.
    ///
    /// `lifetimes` is deliberately in both tables and in *different* conversations: seven have
    /// it in prose, five in a tool call, and the sixth tool call is off the head path. A drawer
    /// that read the wrong table, or ignored `include_off_path`, therefore comes back with a
    /// number rather than with the same number by luck.
    fn tool_corpus() -> Vec<Conversation> {
        let mut convs: Vec<Conversation> = corpus()
            .into_iter()
            .enumerate()
            .map(|(i, c)| tooled(c, if i % 4 == 0 { "rg lifetimes src" } else { "cargo build" }))
            .collect();
        convs.push(edited_away(
            at("codex", "edited", NOW - 5 * DAY, &["python imports"]),
            "rg lifetimes src",
        ));
        convs
    }

    /// A tool call hung off the conversation's last message, at the same instant it ended, so
    /// what changes between the two fixtures is which table a term is found in and nothing else.
    fn tooled(mut conv: Conversation, call: &str) -> Conversation {
        let last = conv.messages.last().expect("`at` writes a message per text").clone();
        conv.messages.push(Message {
            native_id: format!("{}t", last.native_id),
            parent_native_id: Some(last.native_id.clone()),
            seq: last.seq + 1,
            role: Role::Assistant,
            kind: Kind::ToolCall,
            text: call.into(),
            ..last
        });
        conv
    }

    /// Two replies under the same parent, the later one being the one that stayed — the
    /// edit-branch case, and the only shape in which a match is off the head path. The
    /// abandoned sibling is what `include_off_path` reaches and nothing else does.
    fn edited_away(mut conv: Conversation, call: &str) -> Conversation {
        let parent = conv.messages.last().expect("`at` writes a message per text").clone();
        conv.messages.push(Message {
            native_id: "abandoned".into(),
            parent_native_id: Some(parent.native_id.clone()),
            seq: parent.seq + 1,
            role: Role::Assistant,
            kind: Kind::ToolCall,
            text: call.into(),
            ..parent.clone()
        });
        conv.messages.push(Message {
            native_id: "kept".into(),
            parent_native_id: Some(parent.native_id.clone()),
            seq: parent.seq + 2,
            role: Role::Assistant,
            kind: Kind::Prose,
            text: "python imports, once more".into(),
            ..parent
        });
        conv
    }

    fn drawn(reader: &Reader, text: &str) -> Timeline {
        timeline(reader, &Query::exact(text), &opts(), BUCKETS, None).unwrap()
    }

    #[test]
    fn buckets_abut_and_cover_the_whole_axis() {
        let t = drawn(&reader(&corpus()), "");
        assert_eq!(t.buckets.first().unwrap().from, t.from);
        assert_eq!(t.buckets.last().unwrap().until, t.until);
        for pair in t.buckets.windows(2) {
            assert_eq!(pair[0].until, pair[1].from, "a half-open bucket ends where the next begins");
        }
        assert_eq!(t.bucket_days, 1, "39 days over 180 buckets is a day each, not 0.22 of one");
    }

    #[test]
    fn every_conversation_the_filters_keep_lands_in_exactly_one_bucket() {
        let t = drawn(&reader(&corpus()), "");
        let placed: usize = t.buckets.iter().map(|b| b.conversations).sum();
        assert_eq!(placed, 20);
        assert_eq!(t.undated, 0);
    }

    #[test]
    fn a_conversation_that_never_ended_is_counted_and_not_placed() {
        // Four of the real corpus's 4,426. A picture that dropped them silently would be a
        // picture claiming to be everything, which is why the number is on the wire.
        let mut convs = corpus();
        convs.push(at("codex", "undated", NOW - 10 * DAY, &["rust lifetimes"]));
        let mut conn = crate::open(":memory:").unwrap();
        crate::write_conversations(&mut conn, convs.iter()).unwrap();
        conn.execute("UPDATE conversation SET ended_at = NULL WHERE native_id = 'undated'", [])
            .unwrap();
        let reader = Reader { conn, state: IndexState::Ready };

        let t = drawn(&reader, "");
        assert_eq!(t.undated, 1);
        assert_eq!(t.buckets.iter().map(|b| b.conversations).sum::<usize>(), 20);
    }

    #[test]
    fn a_buckets_sources_sum_to_the_conversations_in_it() {
        let t = drawn(&reader(&corpus()), "");
        assert_eq!(t.sources, ["claude-code", "codex"], "sorted, so a stack does not reshuffle");
        for bucket in &t.buckets {
            assert_eq!(bucket.sources.len(), t.sources.len());
            assert_eq!(bucket.sources.iter().sum::<usize>(), bucket.conversations);
        }
    }

    #[test]
    fn nothing_is_reported_as_having_matched_a_query_with_nothing_in_it() {
        // The two series answer two questions, and a browse is not a search. Copying the bars
        // into the ticks would claim a term landed everywhere it did not.
        let t = drawn(&reader(&corpus()), "");
        assert_eq!(t.buckets.iter().map(|b| b.matches).sum::<usize>(), 0);
        assert!(t.buckets.iter().any(|b| b.conversations > 0));
    }

    #[test]
    fn a_match_is_never_counted_in_a_bucket_its_conversation_is_not_in() {
        let t = drawn(&reader(&corpus()), "rust");
        assert_eq!(t.buckets.iter().map(|b| b.matches).sum::<usize>(), 7);
        for bucket in &t.buckets {
            assert!(bucket.matches <= bucket.conversations, "a tick with no bar under it");
        }
    }

    #[test]
    fn the_bars_are_not_narrowed_by_the_window_drawn_over_them() {
        // `poc/ui`'s `visible(true)`, and the whole reason the drawer is worth having: a
        // timeline that also filtered itself by the selection would draw a solid block and
        // could never say what widening would get you.
        let reader = reader(&corpus());
        let whole = drawn(&reader, "");
        let value = Window { from: Some(NOW - 10 * DAY), until: Some(NOW) }.value().unwrap();
        let narrowed = drawn(&reader, &format!("date:{value}"));

        assert_eq!(
            narrowed.buckets.iter().map(|b| b.conversations).sum::<usize>(),
            whole.buckets.iter().map(|b| b.conversations).sum::<usize>(),
            "the bars are the same bars"
        );
        assert!(narrowed.in_range < 20, "and the window is still counting");
        assert_eq!(narrowed.from, whole.from, "the axis is the corpus, not the query");
        assert_eq!(narrowed.until, whole.until);
    }

    #[test]
    fn a_filter_narrows_the_bars_it_is_not_a_window() {
        let reader = reader(&corpus());
        let all = drawn(&reader, "");
        let codex = drawn(&reader, "agent:codex");
        assert_eq!(codex.buckets.iter().map(|b| b.conversations).sum::<usize>(), 10);
        assert_eq!(codex.from, all.from, "and the axis still does not move");
        assert_eq!(codex.sources, ["codex"]);
    }

    #[test]
    fn what_a_drag_writes_is_a_window_the_parser_reads_back() {
        // The round trip that proves there is no second filter state: the drag becomes text,
        // and the selection this reply draws is derived from that text rather than kept beside
        // it. Each edge rounds outward to a whole second (`Window::value_in`), so the window
        // read back contains the one dragged rather than equalling it.
        let reader = reader(&corpus());
        let (a, b) = (NOW - 9 * DAY - 1234, NOW - 3 * DAY + 4321);
        let written = drag(&Query::exact("rust"), b, a);
        assert!(written.value.is_some(), "two instants a day apart name a span");

        let after = drawn(&reader, &written.query);
        let window = after.window.expect("the query now names one");
        assert!(window.from.unwrap() <= a && window.until.unwrap() >= b);
        assert!(window.from.unwrap() > a - 1000 && window.until.unwrap() < b + 1000);
        assert_eq!(window.value, written.value, "one spelling, not two");
        assert!(after.query.contains("rust"), "the free text is left where it was");
    }

    #[test]
    fn a_drag_that_names_no_span_clears_the_window_rather_than_writing_an_empty_one() {
        // `poc/ui` reaches this with "a drag under 1% of the span clears the selection". The
        // grammar gets there on its own: a window whose ends meet is one `DateSpec::between`
        // refuses, so there is no magic fraction to keep in step with the drawing.
        let query = Query::exact("rust date:week");
        let written = drag(&query, NOW, NOW);
        assert_eq!(written.value, None);
        assert!(!written.query.contains("date:"));
        assert!(written.query.contains("rust"));
    }

    #[test]
    fn dragging_the_window_already_in_force_takes_it_back_off() {
        // `Query::toggling`, which is what every chip in this interface does — and it is the
        // only gesture that clears a selection without a control of its own.
        let first = drag(&Query::exact("rust"), NOW - 5 * DAY, NOW);
        let again = drag(&Query::exact(&first.query), NOW - 5 * DAY, NOW);
        assert!(first.query.contains("date:"));
        assert!(!again.query.contains("date:"));
    }

    #[test]
    fn a_negated_date_narrows_the_count_and_draws_no_rectangle() {
        // The complement of a window is not a rectangle, and drawing it as one would put the
        // selection over exactly the stretch the filter threw away.
        let reader = reader(&corpus());
        let value = Window { from: Some(NOW - 10 * DAY), until: Some(NOW) }.value().unwrap();
        let t = drawn(&reader, &format!("-date:{value}"));
        assert!(t.window.is_none());
        assert!(t.in_range > 0 && t.in_range < 20, "and it is still counting: {}", t.in_range);
    }

    #[test]
    fn the_matched_count_is_the_search_s_own_total() {
        // The one claim that keeps the drawer and the list describing the same set. Both walk
        // `count_matching`'s predicate, and they are two statements, so a change to one that
        // missed the other would show up as a drawer disagreeing with the footer above it.
        let reader = reader(&corpus());
        let value = Window { from: Some(NOW - 21 * DAY), until: Some(NOW) }.value().unwrap();
        for text in ["rust", "rust agent:codex", &format!("rust date:{value}"), "python"] {
            let query = Query::exact(text);
            let mut answer = crate::answer(&reader, &query, &opts()).unwrap();
            answer.settle(&reader).unwrap();
            let t = timeline(&reader, &query, &opts(), BUCKETS, None).unwrap();
            assert_eq!(t.total, answer.total, "{text}");
        }
    }

    #[test]
    fn the_drawer_reads_whichever_field_the_search_beside_it_was_asked_for() {
        // The same claim, made once per way of asking rather than once for prose. A drawer
        // that always read `fts_prose` passes the test above and still puts a prose number
        // under a tools list — and tool traffic is where 66–85% of message-level matches land,
        // so that list is the common one rather than an exotic one.
        let reader = reader(&tool_corpus());
        let asks = [
            SearchOptions { field: Field::Prose, ..opts() },
            SearchOptions { field: Field::Tools, ..opts() },
            SearchOptions { field: Field::Tools, include_off_path: true, ..opts() },
        ];
        let mut totals = Vec::new();
        for ask in &asks {
            for text in ["lifetimes", "lifetimes agent:codex", "imports"] {
                let query = Query::exact(text);
                let mut answer = crate::answer(&reader, &query, ask).unwrap();
                answer.settle(&reader).unwrap();
                let t = timeline(&reader, &query, ask, BUCKETS, None).unwrap();
                assert_eq!(t.total, answer.total, "{text} as {:?}", ask.field);
                if text == "lifetimes" {
                    totals.push(t.total);
                }
            }
        }
        // And the three readings are three different sets, which is what stops this passing
        // against a drawer that ignored the options it was handed.
        assert_eq!(totals, vec![7, 5, 6], "prose, tool traffic, and the branch edited away");
    }

    #[test]
    fn the_all_chip_is_the_query_with_no_date_in_it() {
        let reader = reader(&corpus());
        let bare = drawn(&reader, "rust");
        assert!(bare.all.selected);
        assert_eq!(bare.all.query, "rust");

        let windowed = drawn(&reader, "rust date:week");
        assert!(!windowed.all.selected);
        assert_eq!(windowed.all.query, "rust");
    }

    #[test]
    fn an_index_with_nothing_dated_in_it_draws_no_axis_rather_than_a_wrong_one() {
        let reader = reader(&[]);
        let t = drawn(&reader, "rust");
        assert!(t.buckets.is_empty());
        assert_eq!((t.from, t.until, t.bucket_days), (0, 0, 0));
        // And not 1970-01-01, which is what an instant of zero honestly renders as.
        assert_eq!((t.from_date, t.until_date), (None, None));
    }

    #[test]
    fn a_relative_window_is_drawn_open_at_the_end_it_is_open_at() {
        // `date:<7d` reaches to now and past it. A client reading a missing bound as the end of
        // the axis would draw a rectangle stopping where the data stops rather than where the
        // filter does.
        let t = drawn(&reader(&corpus()), "date:week");
        let window = t.window.expect("a span is a span however it was spelled");
        assert!(window.from.is_some());
        assert_eq!(window.until, None);
    }
}

/// What a drawer costs the keystroke it is drawn on.
///
/// Measured the same way and for the same reason as [`crate::search`]'s own cost tests: best of
/// seven with the floor taken, against the real index, because the thing being guarded is a
/// shape that only goes wrong at corpus scale.
///
/// The budget is the search beside it. `chat-search-me9.22` measured keystroke→frame at 30–40 ms
/// p50 with one process per character, and a drawer is a second process on the same keystroke —
/// so what matters is not that this is fast but that it stays within the same order as the
/// counting pass the search already pays for.
#[cfg(test)]
mod cost {
    use super::*;
    use rusqlite::Connection;

    #[test]
    #[ignore = "needs a real index; set CS_INDEX to an index.db"]
    fn a_drawer_costs_about_what_counting_the_same_set_costs() {
        let Ok(path) = std::env::var("CS_INDEX") else { return };
        let conn = Connection::open(path).expect("readable index");
        let reader = Reader { conn, state: crate::IndexState::Ready };
        let opts = || SearchOptions { limit: 60, ..SearchOptions::new(crate::time::now_ms()) };

        let floor = |run: &mut dyn FnMut()| {
            (0..7)
                .map(|_| {
                    let t0 = std::time::Instant::now();
                    run();
                    t0.elapsed().as_secs_f64() * 1000.0
                })
                .fold(f64::INFINITY, f64::min)
        };

        let mut worst: f64 = 0.0;
        println!("   count  timeline   ratio   bars   query");
        // The blank query first — the frame the window opens on — then the prefixes that leave
        // a total unsettled, which are the expensive half of everything this touches.
        for text in ["", "borrow checker", "rust", "ind", "con", "the"] {
            let query = Query::typeahead(text);
            let counting = floor(&mut || {
                if query.is_searchable() {
                    std::hint::black_box(
                        crate::search::count_matching(&reader.conn, &query, &opts()).unwrap(),
                    );
                }
            });
            let drawing = floor(&mut || {
                std::hint::black_box(timeline(&reader, &query, &opts(), BUCKETS, None).unwrap());
            });
            let t = timeline(&reader, &query, &opts(), BUCKETS, None).unwrap();
            let ratio = drawing / counting.max(0.01);
            println!("{counting:8.1} {drawing:9.1} {ratio:7.1} {:6}   {text}", t.buckets.len());
            worst = worst.max(drawing);
        }
        println!("worst drawer: {worst:.1} ms");
        // One order with the count beside it. A regression into a per-row subquery or a sort of
        // every posting — which is what the first draft of the hits pass was — lands far outside
        // this, and it lands there only on the broad prefixes nothing smaller reproduces.
        assert!(worst < 120.0, "drawing the timeline took {worst:.1} ms");
    }
}
