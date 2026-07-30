//! Clock and calendar, shared by every client.
//!
//! Timestamps are stored as absolute epoch millis and become local only at the edge, where
//! something is rendered or bucketed by day (ADR 12's JSON contract carries the absolute
//! value *and* the rendered date, so a non-Rust client never has to re-derive the rule).
//! Local-ness is a presentation concern; storing it would make the index depend on where the
//! machine was sitting when it was built.

use chrono::{Local, TimeZone};

/// Now, as epoch millis — the unit `message.ts` and `conversation.ended_at` are stored in.
///
/// Not chrono, deliberately: an instant carries no zone, so there is nothing here for a
/// calendar to do. `unwrap_or(0)` covers a clock set before 1970, which only costs the
/// recency decay its reference point rather than failing a search.
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// `YYYY-MM-DD` for an instant, in the machine's own zone.
///
/// The single home of the date rule. It previously existed three times over — a hand-rolled
/// civil-from-days conversion in the `cs` binary, a `strflocaltime` in the cs-fzf jq, and
/// implicitly in whatever the next client wrote — and the binary's copy counted UTC days, so
/// a conversation logged after 17:00 PDT was labelled with tomorrow's date.
pub fn local_ymd(ms: i64) -> Option<String> {
    ymd_in(&Local, ms)
}

/// Zone-explicit form of [`local_ymd`]. Production passes `Local`; tests pass a named zone so
/// DST behaviour does not depend on where the machine running them happens to sit.
///
/// `single()` cannot lose a real timestamp: an instant maps to exactly one local time, and
/// only the reverse direction (a wall clock during a DST transition) is ever ambiguous. The
/// `None` is a value outside chrono's ±262,000-year range — corrupt rather than merely
/// missing, but every caller already renders both the same way.
pub fn ymd_in<Tz: TimeZone>(tz: &Tz, ms: i64) -> Option<String> {
    tz.timestamp_millis_opt(ms)
        .single()
        .map(|dt| dt.date_naive().format("%Y-%m-%d").to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use chrono_tz::America::Los_Angeles;

    #[test]
    fn an_evening_timestamp_keeps_its_local_day_instead_of_tomorrows_utc_one() {
        let ms = Los_Angeles.with_ymd_and_hms(2026, 7, 18, 21, 0, 0).unwrap().timestamp_millis();
        // The instant really is the 19th in UTC — that is exactly what the old formatter
        // printed, and why a session held on a Saturday evening was filed under Sunday.
        assert_eq!(ymd_in(&Utc, ms).unwrap(), "2026-07-19");
        assert_eq!(ymd_in(&Los_Angeles, ms).unwrap(), "2026-07-18");
    }

    #[test]
    fn the_offset_a_day_is_named_by_moves_with_dst() {
        // Why a single offset read once from TZ was rejected: 16:30 local is the next day in
        // UTC under PST (-8) and the same day under PDT (-7), so whichever half of the year
        // the offset was sampled in, it names the wrong day for the other half.
        let at = |month, day| {
            Los_Angeles.with_ymd_and_hms(2026, month, day, 16, 30, 0).unwrap().timestamp_millis()
        };
        let (winter, summer) = (at(1, 15), at(7, 15));
        assert_eq!(ymd_in(&Utc, winter).unwrap(), "2026-01-16");
        assert_eq!(ymd_in(&Utc, summer).unwrap(), "2026-07-15");
        assert_eq!(ymd_in(&Los_Angeles, winter).unwrap(), "2026-01-15");
        assert_eq!(ymd_in(&Los_Angeles, summer).unwrap(), "2026-07-15");
    }

    #[test]
    fn now_ms_is_millis_rather_than_seconds() {
        // The recency decay divides `now - ts` by a year in millis. Seconds here would put
        // `now` before every stored timestamp, so `max(0, now - ts)` would clamp to zero and
        // the decay would silently stop applying rather than fail.
        let now = now_ms();
        assert!(now > 1_767_225_600_000, "before 2026-01-01: {now}");
        assert!(now < 4_102_444_800_000, "after 2100-01-01: {now}");
    }
}
