//! Time helpers: unix-millis clocks, calendar period bounds, duration formatting.

use chrono::{Datelike, Duration, Local, NaiveDate, NaiveDateTime};

pub fn now_ms() -> i64 {
    Local::now().timestamp_millis()
}

/// Live clock style: `1h 04m 09s`, `4m 09s`, `9s`.
pub fn fmt_clock(ms: i64) -> String {
    let s = ms.div_euclid(1000);
    let (h, m, sec) = (s / 3600, (s % 3600) / 60, s % 60);
    if h > 0 {
        format!("{h}h {m:02}m {sec:02}s")
    } else if m > 0 {
        format!("{m}m {sec:02}s")
    } else {
        format!("{sec}s")
    }
}

/// Report style: `12d 4h`, `5h 40m`, `45m`, `12s`.
pub fn fmt_short(ms: i64) -> String {
    let s = ms.div_euclid(1000);
    let (d, h, m) = (s / 86400, (s % 86400) / 3600, (s % 3600) / 60);
    if d > 0 {
        format!("{d}d {h}h")
    } else if h > 0 {
        format!("{h}h {m}m")
    } else if m > 0 {
        format!("{m}m")
    } else {
        format!("{s}s")
    }
}

/// Unix millis of local midnight on date `d`.
///
/// Ambiguous midnights (DST fall-back) resolve to the earliest instant; a
/// midnight that does not exist (DST gap) falls back to local noon.
fn local_midnight_ms(d: NaiveDate) -> i64 {
    let naive = d.and_hms_opt(0, 0, 0).expect("midnight is valid");
    naive
        .and_local_timezone(Local)
        .earliest()
        .map(|dt| dt.timestamp_millis())
        .unwrap_or_else(|| {
            let noon = d.and_hms_opt(12, 0, 0).expect("noon is valid");
            noon.and_local_timezone(Local)
                .earliest()
                .expect("cannot resolve local noon; check system timezone")
                .timestamp_millis()
        })
}

/// Local calendar day containing `now`, as (start_ms, end_ms).
pub fn day_bounds(now: NaiveDateTime) -> (i64, i64) {
    let d = now.date();
    (local_midnight_ms(d), local_midnight_ms(d + Duration::days(1)))
}

/// ISO week (Monday-start) containing `now`, as (start_ms, end_ms).
pub fn week_bounds(now: NaiveDateTime) -> (i64, i64) {
    let d = now.date();
    let monday = d - Duration::days(d.weekday().num_days_from_monday() as i64);
    (local_midnight_ms(monday), local_midnight_ms(monday + Duration::days(7)))
}
