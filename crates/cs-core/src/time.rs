//! Clock and calendar, shared by every client.
//!
//! Timestamps are stored as absolute epoch millis and become local only at the edge, where
//! something is rendered or bucketed by day (ADR 12's JSON contract carries the absolute
//! value *and* the rendered date, so a non-Rust client never has to re-derive the rule).
//! Local-ness is a presentation concern; storing it would make the index depend on where the
//! machine was sitting when it was built.

use chrono::{DateTime, Days, Local, Months, NaiveDate, NaiveDateTime, TimeZone};

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
/// civil-from-days conversion in the `cs` binary, a `strflocaltime` in a shell client's jq, and
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

/// Midnight opening the civil day that contains `ms`.
///
/// The lower edge of `date:today`. Zone-explicit for the same reason [`ymd_in`] is.
pub fn day_start_in<Tz: TimeZone>(tz: &Tz, ms: i64) -> Option<i64> {
    let local = tz.timestamp_millis_opt(ms).single()?;
    let midnight = local.date_naive().and_hms_opt(0, 0, 0)?;
    Some(resolve(tz, midnight)?.timestamp_millis())
}

/// [`day_start_in`] against the machine's own zone.
pub fn local_day_start(ms: i64) -> Option<i64> {
    day_start_in(&Local, ms)
}

/// A wall clock a person typed, back to the instant it names.
///
/// Accepts `YYYY-MM-DD`, `YYYY-MM-DDTHH:MM` and `YYYY-MM-DDTHH:MM:SS`, with a space accepted
/// wherever the `T` goes. A bare date is the midnight opening that day, so a half-open
/// `2026-08-04 .. 2026-08-05` is the whole of the 4th and consecutive days tile.
///
/// Local rather than UTC because what is being named is something that happened to the person
/// typing it. "The morning I spent benchmarking" is a wall clock; nobody knows their own
/// mornings in UTC.
pub fn local_instant(text: &str) -> Option<i64> {
    instant_in(&Local, text)
}

/// Zone-explicit form of [`local_instant`], for the same reason [`ymd_in`] has one.
pub fn instant_in<Tz: TimeZone>(tz: &Tz, text: &str) -> Option<i64> {
    let text = text.trim();
    // Longest form first: `%Y-%m-%dT%H:%M` would accept `…T10:00:30` and silently drop the
    // seconds, which is the wrong direction for a bound.
    let naive = ["%Y-%m-%dT%H:%M:%S", "%Y-%m-%d %H:%M:%S", "%Y-%m-%dT%H:%M", "%Y-%m-%d %H:%M"]
        .iter()
        .find_map(|f| NaiveDateTime::parse_from_str(text, f).ok())
        .or_else(|| {
            NaiveDate::parse_from_str(text, "%Y-%m-%d").ok()?.and_hms_opt(0, 0, 0)
        })?;
    Some(resolve(tz, naive)?.timestamp_millis())
}

/// `ms` moved by whole civil days, keeping its wall-clock time of day.
///
/// Civil rather than a fixed 86,400,000 ms per step, because across a DST boundary a day
/// really is 23 or 25 hours. `date:<1d` means "since this time yesterday"; subtracting a
/// fixed 24 h lands an hour off on two days a year, and always in the direction that makes
/// yesterday's last hour disappear from a filter that claims to include it.
pub fn shift_days_in<Tz: TimeZone>(tz: &Tz, ms: i64, days: i64) -> Option<i64> {
    shift(tz, ms, |naive| match days >= 0 {
        true => naive.checked_add_days(Days::new(days as u64)),
        false => naive.checked_sub_days(Days::new(days.unsigned_abs())),
    })
}

/// `ms` moved by whole civil months, clamping to the last day of a shorter month.
///
/// chrono's own clamping rule: one month before 31 March is 28 February, not 3 March.
pub fn shift_months_in<Tz: TimeZone>(tz: &Tz, ms: i64, months: i64) -> Option<i64> {
    let magnitude = u32::try_from(months.unsigned_abs()).ok()?;
    shift(tz, ms, |naive| match months >= 0 {
        true => naive.checked_add_months(Months::new(magnitude)),
        false => naive.checked_sub_months(Months::new(magnitude)),
    })
}

fn shift<Tz: TimeZone>(
    tz: &Tz,
    ms: i64,
    move_it: impl FnOnce(NaiveDateTime) -> Option<NaiveDateTime>,
) -> Option<i64> {
    let local = tz.timestamp_millis_opt(ms).single()?;
    let moved = move_it(local.naive_local())?;
    Some(resolve(tz, moved)?.timestamp_millis())
}

/// A wall clock back to the instant it names.
///
/// The direction that can genuinely fail, unlike [`ymd_in`]'s. An hour repeats every autumn
/// and one never happens each spring, so a civil-time calculation can land on a reading no
/// clock in that zone ever showed. Taking the earlier of an ambiguous pair and walking
/// forward out of a gap keeps a filter edge monotonic, which is what callers depend on: a
/// window's lower bound must not overtake its upper one twice a year.
fn resolve<Tz: TimeZone>(tz: &Tz, naive: NaiveDateTime) -> Option<DateTime<Tz>> {
    use chrono::LocalResult;
    match tz.from_local_datetime(&naive) {
        LocalResult::Single(dt) => Some(dt),
        LocalResult::Ambiguous(earlier, _) => Some(earlier),
        // No zone shifts by more than a couple of hours, so the first valid minute is well
        // inside this walk; the bound is here so a malformed zone cannot spin.
        LocalResult::None => (1..=180).find_map(|minutes| {
            let stepped = naive.checked_add_signed(chrono::Duration::minutes(minutes))?;
            match tz.from_local_datetime(&stepped) {
                LocalResult::Single(dt) => Some(dt),
                LocalResult::Ambiguous(earlier, _) => Some(earlier),
                LocalResult::None => None,
            }
        }),
    }
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

    /// Noon in Los Angeles, which is never inside a DST transition.
    fn noon(month: u32, day: u32) -> i64 {
        Los_Angeles.with_ymd_and_hms(2026, month, day, 12, 0, 0).unwrap().timestamp_millis()
    }

    #[test]
    fn a_civil_day_is_23_hours_across_spring_forward_and_25_across_fall_back() {
        // The reason `date:` cannot do its arithmetic in fixed 86,400,000 ms steps. In 2026
        // Los Angeles springs forward at 02:00 on 8 March and falls back at 02:00 on
        // 1 November. The short and long days are therefore those two days themselves —
        // measured midnight to midnight, the transition has to fall *inside* the interval.
        let day_after = |a: i64| {
            let start = day_start_in(&Los_Angeles, a).unwrap();
            let next = shift_days_in(&Los_Angeles, start, 1).unwrap();
            (next - start) / 1000
        };
        assert_eq!(day_after(noon(3, 8)), 82_800, "spring forward");
        assert_eq!(day_after(noon(11, 1)), 90_000, "fall back");
        assert_eq!(day_after(noon(6, 1)), 86_400, "an ordinary day");
    }

    #[test]
    fn shifting_by_a_day_keeps_the_time_of_day_rather_than_subtracting_24_hours() {
        // `date:<1d` means "since this time yesterday". A fixed 24 h would land at 11:00 or
        // 13:00 on the two days a year the calendar disagrees with the clock.
        // Noon on the 8th is PDT and noon on the 7th is PST, so the same wall clock really is
        // 23 h apart; 1 November against 31 October is the 25 h case.
        for (from, to) in [((3, 8), (3, 7)), ((11, 1), (10, 31)), ((6, 2), (6, 1))] {
            let moved = shift_days_in(&Los_Angeles, noon(from.0, from.1), -1).unwrap();
            assert_eq!(moved, noon(to.0, to.1), "{from:?} back one civil day");
        }
        assert_eq!(noon(3, 8) - noon(3, 7), 82_800_000, "and the gap really is 23 h");
        assert_eq!(noon(11, 1) - noon(10, 31), 90_000_000, "and 25 h the other way");
    }

    #[test]
    fn a_month_back_clamps_to_the_last_day_of_a_shorter_month() {
        let march31 = Los_Angeles.with_ymd_and_hms(2026, 3, 31, 9, 0, 0).unwrap().timestamp_millis();
        let got = shift_months_in(&Los_Angeles, march31, -1).unwrap();
        assert_eq!(ymd_in(&Los_Angeles, got).unwrap(), "2026-02-28");
    }

    #[test]
    fn midnight_that_never_happened_resolves_forward_into_the_day() {
        // Some zones shift at midnight rather than at 02:00, so `date:today` has to have an
        // answer on a day whose first wall-clock minute does not exist. Havana springs
        // forward at 00:00, making 2026-03-08 start at 01:00 local.
        use chrono_tz::America::Havana;
        let ms = Havana.with_ymd_and_hms(2026, 3, 8, 12, 0, 0).unwrap().timestamp_millis();
        let start = day_start_in(&Havana, ms).unwrap();
        assert_eq!(ymd_in(&Havana, start).unwrap(), "2026-03-08", "stays inside its own day");
        assert!(start <= ms, "the day cannot start after noon on it");
    }

    #[test]
    fn a_typed_bound_means_the_same_wall_clock_however_much_of_it_was_written() {
        // The four accepted spellings of one instant, plus the bare date that opens its day.
        let at_ten = Los_Angeles.with_ymd_and_hms(2026, 8, 4, 10, 0, 0).unwrap().timestamp_millis();
        for text in ["2026-08-04T10:00", "2026-08-04 10:00", "2026-08-04T10:00:00", "2026-08-04 10:00:00"] {
            assert_eq!(instant_in(&Los_Angeles, text), Some(at_ten), "{text}");
        }
        let midnight = day_start_in(&Los_Angeles, at_ten).unwrap();
        assert_eq!(instant_in(&Los_Angeles, "2026-08-04"), Some(midnight));
        assert_eq!(instant_in(&Los_Angeles, "  2026-08-04  "), Some(midnight), "surrounding space");
    }

    #[test]
    fn a_bound_that_is_not_a_wall_clock_is_refused_rather_than_guessed_at() {
        // Same rule as `date:` in the query language: a value nothing can be resolved against
        // must not silently become one that can, because a bound off by a day silently changes
        // what a span covers.
        for text in ["", "today", "2026-08", "08/04/2026", "2026-13-01", "2026-08-04T25:00", "1785855660000"] {
            assert_eq!(instant_in(&Los_Angeles, text), None, "{text:?}");
        }
    }

    #[test]
    fn a_typed_bound_survives_a_wall_clock_that_never_happened() {
        // Havana springs forward at midnight, so 2026-03-08 has no 00:00. A bound that cannot
        // be refused into thin air — it is the edge of a span — walks forward into the day.
        use chrono_tz::America::Havana;
        let got = instant_in(&Havana, "2026-03-08").unwrap();
        assert_eq!(ymd_in(&Havana, got).unwrap(), "2026-03-08");
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
