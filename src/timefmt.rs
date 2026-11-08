//! Time helpers: unix-millis clocks, calendar period bounds, duration formatting.

use chrono::{Datelike, Duration, Local, NaiveDate};

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

/// Long totals in hours rather than days: `989h`, `42h 10m`, `45m`, `12s`.
/// Minutes are left out from 100 hours on.
pub fn fmt_hours(ms: i64) -> String {
    let s = ms.div_euclid(1000);
    let (h, m) = (s / 3600, (s % 3600) / 60);
    if h >= 100 {
        format!("{h}h")
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
pub fn local_midnight_ms(d: NaiveDate) -> i64 {
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

/// Local calendar day `d`, as (start_ms, end_ms).
pub fn day_bounds(d: NaiveDate) -> (i64, i64) {
    (local_midnight_ms(d), local_midnight_ms(d + Duration::days(1)))
}

/// ISO week (Monday-start) containing `d`, as (start_ms, end_ms).
pub fn week_bounds(d: NaiveDate) -> (i64, i64) {
    let monday = d - Duration::days(d.weekday().num_days_from_monday() as i64);
    (local_midnight_ms(monday), local_midnight_ms(monday + Duration::days(7)))
}

/// The month containing `d`, as (start_ms, end_ms).
pub fn month_bounds(d: NaiveDate) -> (i64, i64) {
    let first = d.with_day(1).expect("day 1 exists");
    let next = (if d.month() == 12 {
        d.with_year(d.year() + 1)
            .and_then(|y| y.with_month(1))
    } else {
        d.with_month(d.month() + 1)
    })
    .and_then(|m| m.with_day(1))
    .expect("first of next month exists");
    (local_midnight_ms(first), local_midnight_ms(next))
}

/// Local calendar date containing unix millis `ms`.
pub fn date_of(ms: i64) -> NaiveDate {
    chrono::DateTime::from_timestamp_millis(ms)
        .expect("valid millis")
        .with_timezone(&Local)
        .date_naive()
}

/// `AUG 2026`.
pub fn fmt_month(d: NaiveDate) -> String {
    d.format("%b %Y").to_string().to_uppercase()
}

/// `28 AUG 2026`.
pub fn fmt_day_title(d: NaiveDate) -> String {
    format!(
        "{} {} {}",
        d.day(),
        d.format("%b").to_string().to_uppercase(),
        d.year()
    )
}

/// `09:12–11:34`; across a local midnight, `28 AUG 09:12 – 29 AUG 11:34`.
pub fn fmt_range(start_ms: i64, end_ms: i64) -> String {
    let s = chrono::DateTime::from_timestamp_millis(start_ms)
        .expect("valid millis")
        .with_timezone(&Local);
    let e = chrono::DateTime::from_timestamp_millis(end_ms)
        .expect("valid millis")
        .with_timezone(&Local);
    if s.date_naive() == e.date_naive() {
        format!("{}–{}", s.format("%H:%M"), e.format("%H:%M"))
    } else {
        format!(
            "{} – {}",
            s.format("%-d %b %H:%M").to_string().to_uppercase(),
            e.format("%-d %b %H:%M").to_string().to_uppercase()
        )
    }
}

/// Local clock `HH:MM` for unix millis.
pub fn fmt_hm(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .expect("valid millis")
        .with_timezone(&Local)
        .format("%H:%M")
        .to_string()
}

/// Unix millis as local `YYYY-MM-DDTHH:MM:SS`.
pub fn iso_local(ms: i64) -> String {
    match chrono::DateTime::from_timestamp_millis(ms) {
        Some(dt) => dt
            .with_timezone(&Local)
            .naive_local()
            .format("%Y-%m-%dT%H:%M:%S")
            .to_string(),
        None => String::new(),
    }
}

/// Parse `HH:MM` as a clock time on local date `d`; None if malformed.
pub fn parse_hm_on(d: NaiveDate, s: &str) -> Option<i64> {
    let (h, m) = s.split_once(':')?;
    let h: u32 = h.parse().ok()?;
    let m: u32 = m.parse().ok()?;
    if h > 23 || m > 59 {
        return None;
    }
    d.and_hms_opt(h, m, 0)?
        .and_local_timezone(Local)
        .earliest()
        .map(|dt| dt.timestamp_millis())
}

/// A duration like `5m`, `90s`, `1h30m`, or a bare number of minutes, in
/// milliseconds; None if malformed.
pub fn parse_duration(s: &str) -> Option<i64> {
    let s = s.trim();
    if let Ok(mins) = s.parse::<i64>() {
        return (mins >= 0).then_some(mins * 60_000);
    }
    let (mut total, mut num, mut parts) = (0i64, String::new(), 0);
    for c in s.chars() {
        if c.is_ascii_digit() {
            num.push(c);
            continue;
        }
        let unit = match c {
            'h' => 3_600_000,
            'm' => 60_000,
            's' => 1_000,
            _ => return None,
        };
        total = total.checked_add(num.parse::<i64>().ok()?.checked_mul(unit)?)?;
        num.clear();
        parts += 1;
    }
    (num.is_empty() && parts > 0).then_some(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hours() {
        const H: i64 = 3_600_000;
        assert_eq!(fmt_hours(989 * H + 20 * 60_000), "989h");
        assert_eq!(fmt_hours(42 * H + 10 * 60_000), "42h 10m");
        assert_eq!(fmt_hours(45 * 60_000), "45m");
        assert_eq!(fmt_hours(12_000), "12s");
    }

    #[test]
    fn durations() {
        assert_eq!(parse_duration("5"), Some(300_000));
        assert_eq!(parse_duration("5m"), Some(300_000));
        assert_eq!(parse_duration("90s"), Some(90_000));
        assert_eq!(parse_duration("1h30m"), Some(5_400_000));
        assert_eq!(parse_duration("0"), Some(0));
        for bad in ["", "m", "5x", "1h30", "-5", "5 m"] {
            assert_eq!(parse_duration(bad), None, "{bad:?}");
        }
    }
}
