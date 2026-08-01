//! Throwaway harness for chat-search-6eb.38: what does a prefix index cost, and what does it buy?
//!
//! Usage: `cargo run --release --example prefix_bench -- <db> <label>`
//!
//! Columns, per query and per mode:
//! - `rank0` — `search_grouped` at nested 0, which is ranking alone and is what
//!   chat-search-6eb.38's headline numbers measured.
//! - `rank3` — the same at nested 3, TUI settings, so ranking plus hydration.
//! - `mark` — `highlight::spans_for` over the candidate set that produced, by whichever route
//!   it picks today.
//! - `idx` / `batch` — the same marking forced down each of the two routes, which is what says
//!   whether the two-branch rule in `spans_for` still has a reason to exist.

use cs_core::{highlight, Field, Query, SearchOptions};
use std::time::Instant;

/// Queries the bead and chat-search-6eb.29 name, plus the exact controls they were measured
/// against.
const QUERIES: [&str; 9] =
    ["pro", "the", "commit", "borrow", "rus", "rust", "deep", "learning", "deep learning"];

fn median(mut xs: Vec<f64>) -> f64 {
    xs.sort_by(|a, b| a.partial_cmp(b).unwrap());
    xs[xs.len() / 2]
}

/// Median wall time of `n` runs, in ms, after three warming ones. Every number this feeds is
/// steady-state: first touch of a term costs 700-1300 ms on this index (chat-search-6eb.29).
fn timed(n: usize, mut f: impl FnMut()) -> f64 {
    for _ in 0..3 {
        f();
    }
    median(
        (0..n)
            .map(|_| {
                let t = Instant::now();
                f();
                t.elapsed().as_secs_f64() * 1e3
            })
            .collect(),
    )
}

fn main() {
    let mut args = std::env::args().skip(1);
    let db = args.next().expect("db path");
    let label = args.next().unwrap_or_default();
    let conn = cs_core::open(&db).expect("open");
    let now = cs_core::now_ms();

    println!("## {label} ({db})");
    println!();
    println!("| query | mode | rank0 | rank3 | rows | KB | mark | idx | batch |");
    println!("| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |");

    for text in QUERIES {
        for typeahead in [true, false] {
            let query = if typeahead { Query::typeahead(text) } else { Query::exact(text) };
            let mut opts = SearchOptions::new(now);
            opts.limit = 50;
            opts.field = Field::Prose;

            let rank0 = timed(5, || {
                let _ = cs_core::search_grouped(&conn, &query, &opts).expect("rank0");
            });
            opts.nested = 3;
            let rank3 = timed(5, || {
                let _ = cs_core::search_grouped(&conn, &query, &opts).expect("rank3");
            });

            // The candidate set `search_grouped` really marks, rebuilt here so the marking can
            // be timed on its own and forced down either route.
            let groups = cs_core::search_grouped(&conn, &query, &opts).expect("rows");
            let terms = query.marking_terms();
            let ids: Vec<String> =
                groups.iter().flat_map(|g| g.hits.iter().map(|h| h.msg_id.clone())).collect();
            let mut rows: Vec<(i64, String)> = Vec::new();
            for id in &ids {
                let got = conn.query_row("SELECT rowid, text FROM message WHERE id = ?1", [id], |r| {
                    Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
                });
                if let Ok(row) = got {
                    rows.push(row);
                }
            }
            let borrowed: Vec<(i64, &str)> = rows.iter().map(|(id, t)| (*id, t.as_str())).collect();
            let texts: Vec<&str> = rows.iter().map(|(_, t)| t.as_str()).collect();
            let bytes: usize = rows.iter().map(|(_, t)| t.len()).sum();

            let mark = timed(3, || {
                let _ = highlight::spans_for(&conn, Field::Prose, &borrowed, &terms);
            });
            let idx = timed(3, || {
                for &(rowid, t) in &borrowed {
                    let _ = highlight::spans_indexed(&conn, Field::Prose, rowid, t, &terms);
                }
            });
            let batch = timed(3, || {
                let _ = highlight::spans_many(&texts, &terms);
            });

            println!(
                "| `{text}` | {} | {rank0:.1} | {rank3:.1} | {} | {:.0} | {mark:.1} | {idx:.1} | {batch:.1} |",
                if typeahead { "typeahead" } else { "exact" },
                borrowed.len(),
                bytes as f64 / 1024.0,
            );
        }
    }
}
