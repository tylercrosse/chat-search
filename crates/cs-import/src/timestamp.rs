//! RFC3339 -> epoch milliseconds, in one place.
//!
//! This is the shared home chat-search-n58.8 asks for. The parser was written three times
//! over — `codex.rs`, `claude_code.rs` and `gemini.rs` each carry a byte-identical copy,
//! along with a copy of the `digits` helper — because each importer needed it before there
//! was anywhere to put it. `google_takeout.rs` would have been the fourth, so the module
//! exists now and that importer uses it. Migrating the other three is left to n58.8: they
//! are covered by tests that would have to move with them, and doing it here would bury a
//! new importer inside a refactor of three working ones.
//!
//! `chatgpt_export.rs`'s `epoch_millis` is deliberately *not* in scope for that migration.
//! It shares a name but takes a `serde_json::Value` and reads float seconds, because that is
//! what ChatGPT stores. It is a different function.

/// Parse an RFC3339 timestamp into epoch milliseconds.
///
/// Accepts an optional fractional part of any length and either `Z` or a `±HH:MM` / `±HHMM`
/// offset. Returns `None` for anything it cannot read rather than guessing: a wrong instant
/// is worse than an absent one, since a conversation with no timestamp is visibly undated
/// while one with a bad timestamp sorts silently into the wrong week.
pub(crate) fn epoch_millis(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() < 19 || !matches!(b[10], b'T' | b't' | b' ') {
        return None;
    }
    if b[4] != b'-' || b[7] != b'-' || b[13] != b':' || b[16] != b':' {
        return None;
    }
    let year = digits(b, 0, 4)?;
    let month = digits(b, 5, 2)?;
    let day = digits(b, 8, 2)?;
    let hour = digits(b, 11, 2)?;
    let minute = digits(b, 14, 2)?;
    let second = digits(b, 17, 2)?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    if hour > 23 || minute > 59 || second > 60 {
        return None;
    }

    let mut i = 19;
    let mut millis_frac = 0i64;
    if b.get(i) == Some(&b'.') {
        i += 1;
        let start = i;
        while i < b.len() && b[i].is_ascii_digit() {
            // Digits past the third are real but below our resolution, so they are read and
            // discarded rather than left to be misparsed as an offset.
            if i - start < 3 {
                millis_frac = millis_frac * 10 + i64::from(b[i] - b'0');
            }
            i += 1;
        }
        if i == start {
            return None;
        }
        for _ in (i - start)..3 {
            millis_frac *= 10;
        }
    }

    // Absent zone means local time, which we cannot resolve, so treat it as UTC.
    let offset_seconds = match b.get(i) {
        None | Some(b'Z') | Some(b'z') => 0,
        Some(sign @ (b'+' | b'-')) => {
            let sign = if *sign == b'-' { -1 } else { 1 };
            let oh = digits(b, i + 1, 2)?;
            // `+HH:MM` and `+HHMM` are both legal; only the separator moves.
            let mm_at = if b.get(i + 3) == Some(&b':') { i + 4 } else { i + 3 };
            let om = digits(b, mm_at, 2)?;
            sign * (oh * 3600 + om * 60)
        }
        Some(_) => return None,
    };

    let days = days_from_civil(year, month, day);
    let seconds = days * 86_400 + hour * 3_600 + minute * 60 + second - offset_seconds;
    Some(seconds * 1_000 + millis_frac)
}

/// Days since 1970-01-01 for a proleptic Gregorian date (Howard Hinnant's `days_from_civil`).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn digits(b: &[u8], start: usize, len: usize) -> Option<i64> {
    let slice = b.get(start..start + len)?;
    let mut n = 0i64;
    for &c in slice {
        if !c.is_ascii_digit() {
            return None;
        }
        n = n * 10 + i64::from(c - b'0');
    }
    Some(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_utc_timestamp_with_millis_parses_to_the_instant_google_recorded() {
        // The earliest record in the real Gemini Apps export. Google's *HTML* export
        // rendered this same instant as "Jan 18, 2025, 11:06:20 AM PDT" — a fixed -07:00
        // applied in a month Los Angeles spends at -08:00, so the HTML wall clock was an
        // hour late. This is the number that made the JSON export worth insisting on.
        assert_eq!(epoch_millis("2025-01-18T18:06:20.586Z"), Some(1_737_223_580_586));
    }

    #[test]
    fn fraction_shorter_or_longer_than_three_digits_still_lands_on_millis() {
        let base = epoch_millis("2026-08-01T17:48:13Z").unwrap();
        assert_eq!(epoch_millis("2026-08-01T17:48:13.9Z"), Some(base + 900));
        assert_eq!(epoch_millis("2026-08-01T17:48:13.90Z"), Some(base + 900));
        assert_eq!(epoch_millis("2026-08-01T17:48:13.908Z"), Some(base + 908));
        // Sub-millisecond digits are below our resolution, and must not be misread as a zone.
        assert_eq!(epoch_millis("2026-08-01T17:48:13.908123Z"), Some(base + 908));
    }

    #[test]
    fn an_explicit_offset_is_applied_and_both_separators_are_accepted() {
        let utc = epoch_millis("2026-04-07T15:37:52Z").unwrap();
        assert_eq!(epoch_millis("2026-04-07T08:37:52-07:00"), Some(utc));
        assert_eq!(epoch_millis("2026-04-07T08:37:52-0700"), Some(utc));
        assert_eq!(epoch_millis("2026-04-07T17:37:52+02:00"), Some(utc));
    }

    #[test]
    fn a_missing_zone_is_read_as_utc_rather_than_rejected() {
        // Gemini in Workspace writes `+00:00` explicitly, but the bare form appears in
        // hand-written fixtures and dropping the turn would cost more than assuming UTC.
        assert_eq!(
            epoch_millis("2026-04-07T15:37:52"),
            epoch_millis("2026-04-07T15:37:52Z")
        );
    }

    #[test]
    fn garbage_is_declined_instead_of_becoming_a_plausible_instant() {
        for bad in [
            "",
            "not a timestamp",
            "2026-08-01",              // date only
            "2026-13-01T00:00:00Z",    // month 13
            "2026-08-32T00:00:00Z",    // day 32
            "2026-08-01T25:00:00Z",    // hour 25
            "2026-08-01T00:61:00Z",    // minute 61
            "2026-08-01T00:00:00.Z",   // empty fraction
            "2026-08-01T00:00:00~",    // junk where a zone belongs
            "2026/08/01T00:00:00Z",    // wrong separators
        ] {
            assert_eq!(epoch_millis(bad), None, "should have declined {bad:?}");
        }
    }

    #[test]
    fn dates_before_the_epoch_go_negative_rather_than_wrapping() {
        assert_eq!(epoch_millis("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(epoch_millis("1969-12-31T23:59:59Z"), Some(-1_000));
    }
}
