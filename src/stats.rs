//! Time per day, and the summaries built from it: totals, streaks, months.
//!
//! Front-end neutral: the window's Stats tab draws these, and nothing here
//! knows how it is drawn.

use std::collections::BTreeMap;

use chrono::{Datelike, Months, NaiveDate};

use crate::db::{SessionRow, Total};
use crate::timefmt::{date_of, day_bounds};

/// The stretch of days a view covers.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Year {
    /// This month and the eleven before it, up to today.
    Last12,
    /// One calendar year, up to today if it is this year.
    Calendar(i32),
}

impl Year {
    /// First and last day covered, both inclusive.
    pub fn days(self, today: NaiveDate) -> (NaiveDate, NaiveDate) {
        match self {
            Year::Last12 => {
                let month = today.with_day(1).expect("day 1 exists");
                let first = month.checked_sub_months(Months::new(11)).unwrap_or(month);
                (first, today)
            }
            Year::Calendar(y) => {
                let jan = NaiveDate::from_ymd_opt(y, 1, 1).unwrap_or(today);
                let dec = NaiveDate::from_ymd_opt(y, 12, 31).unwrap_or(today);
                (jan, dec.min(today))
            }
        }
    }

    pub fn label(self) -> String {
        match self {
            Year::Last12 => "Last 12 months".to_string(),
            Year::Calendar(y) => y.to_string(),
        }
    }

    /// What can be picked: the last 12 months, then each calendar year from
    /// this one back to the year of the first session.
    pub fn choices(first: Option<NaiveDate>, today: NaiveDate) -> Vec<Year> {
        let oldest = first.map_or(today.year(), |d| d.year().min(today.year()));
        std::iter::once(Year::Last12)
            .chain((oldest..=today.year()).rev().map(Year::Calendar))
            .collect()
    }
}

/// Time per local calendar day and activity, over a stretch of days.
///
/// A session that runs past midnight counts on each day it touches, the
/// way the window's Day tab shows it, and a running one counts up to now.
#[derive(Debug, Default)]
pub struct Ledger {
    days: BTreeMap<NaiveDate, Vec<(String, i64)>>,
}

impl Ledger {
    /// Build from `sessions` (those overlapping the days, as
    /// [`Db::sessions_in`](crate::db::Db::sessions_in) returns them),
    /// counting only time from `first` through `last`.
    pub fn new(sessions: &[SessionRow], first: NaiveDate, last: NaiveDate, now: i64) -> Self {
        let mut days: BTreeMap<NaiveDate, Vec<(String, i64)>> = BTreeMap::new();
        for s in sessions {
            let end = s.ended_at.unwrap_or(now);
            if end <= s.started_at {
                continue;
            }
            let mut d = date_of(s.started_at).max(first);
            let stop = date_of(end).min(last);
            while d <= stop {
                let (d0, d1) = day_bounds(d);
                let ms = end.min(d1) - s.started_at.max(d0);
                if ms > 0 {
                    let day = days.entry(d).or_default();
                    match day.iter_mut().find(|(n, _)| *n == s.name) {
                        Some((_, t)) => *t += ms,
                        None => day.push((s.name.clone(), ms)),
                    }
                }
                let Some(next) = d.succ_opt() else { break };
                d = next;
            }
        }
        Self { days }
    }

    /// Each day's time on the activities `keep` accepts. Days without any
    /// are left out.
    pub fn per_day(&self, keep: impl Fn(&str) -> bool) -> BTreeMap<NaiveDate, i64> {
        self.days
            .iter()
            .filter_map(|(d, acts)| {
                let ms: i64 = acts.iter().filter(|(n, _)| keep(n)).map(|(_, t)| t).sum();
                (ms > 0).then_some((*d, ms))
            })
            .collect()
    }

    /// Per-activity time on `day`, largest first.
    pub fn on(&self, day: NaiveDate, keep: impl Fn(&str) -> bool) -> Vec<Total> {
        let acts = self.days.get(&day).map(Vec::as_slice).unwrap_or_default();
        sorted(acts.iter().filter(|(n, _)| keep(n)).cloned())
    }

    /// Per-activity time over every day, largest first.
    pub fn totals(&self, keep: impl Fn(&str) -> bool) -> Vec<Total> {
        let mut sum: BTreeMap<&str, i64> = BTreeMap::new();
        for (n, t) in self.days.values().flatten() {
            if keep(n) {
                *sum.entry(n).or_default() += t;
            }
        }
        sorted(sum.into_iter().map(|(n, t)| (n.to_string(), t)))
    }
}

fn sorted(it: impl Iterator<Item = (String, i64)>) -> Vec<Total> {
    let mut v: Vec<Total> = it.map(|(name, ms)| Total { name, ms }).collect();
    v.sort_by(|a, b| b.ms.cmp(&a.ms).then_with(|| a.name.cmp(&b.name)));
    v
}

#[derive(Debug, Default, PartialEq)]
pub struct Summary {
    pub total: i64,
    /// Days with any time.
    pub active_days: usize,
    /// Days in a row with time, ending today; while today has none yet,
    /// ending yesterday.
    pub current_streak: usize,
    pub longest_streak: usize,
}

/// Totals and streaks over `per_day`, as [`Ledger::per_day`] returns it.
pub fn summarize(per_day: &BTreeMap<NaiveDate, i64>, today: NaiveDate) -> Summary {
    let mut longest = 0;
    let mut run = 0;
    let mut prev: Option<NaiveDate> = None;
    for d in per_day.keys() {
        run = if prev.and_then(|p| p.succ_opt()) == Some(*d) {
            run + 1
        } else {
            1
        };
        longest = longest.max(run);
        prev = Some(*d);
    }
    let mut current = 0;
    let mut d = if per_day.contains_key(&today) {
        Some(today)
    } else {
        today.pred_opt()
    };
    while let Some(day) = d.filter(|day| per_day.contains_key(day)) {
        current += 1;
        d = day.pred_opt();
    }
    Summary {
        total: per_day.values().sum(),
        active_days: per_day.len(),
        current_streak: current,
        longest_streak: longest,
    }
}

/// Time per calendar month, for each month from `first`'s to `last`'s, as
/// (first day of the month, ms), oldest first.
pub fn months(
    per_day: &BTreeMap<NaiveDate, i64>,
    first: NaiveDate,
    last: NaiveDate,
) -> Vec<(NaiveDate, i64)> {
    let mut out: Vec<(NaiveDate, i64)> = Vec::new();
    let mut m = first.with_day(1).expect("day 1 exists");
    while m <= last {
        out.push((m, 0));
        let Some(next) = m.checked_add_months(Months::new(1)) else {
            break;
        };
        m = next;
    }
    for (d, ms) in per_day.range(first..=last) {
        let key = d.with_day(1).expect("day 1 exists");
        if let Some(slot) = out.iter_mut().find(|(m, _)| *m == key) {
            slot.1 += ms;
        }
    }
    out
}

/// Heatmap shade for a day's time: 0 none, 1 under an hour, 2 under four,
/// 3 under eight, 4 eight hours or more.
pub fn heat_level(ms: i64) -> u8 {
    const H: i64 = 3_600_000;
    match ms {
        ..=0 => 0,
        t if t < H => 1,
        t if t < 4 * H => 2,
        t if t < 8 * H => 3,
        _ => 4,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::timefmt::local_midnight_ms;

    const H: i64 = 3_600_000;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    /// Local time `h` hours after midnight on `day` (may run past it).
    fn at(day: NaiveDate, h: f64) -> i64 {
        local_midnight_ms(day) + (h * H as f64) as i64
    }

    fn sess(name: &str, start: i64, end: Option<i64>) -> SessionRow {
        SessionRow {
            id: 0,
            name: name.to_string(),
            started_at: start,
            ended_at: end,
        }
    }

    fn pairs(v: Vec<Total>) -> Vec<(String, i64)> {
        v.into_iter().map(|t| (t.name, t.ms)).collect()
    }

    #[test]
    fn last_12_months_starts_on_the_first_of_a_month() {
        assert_eq!(
            Year::Last12.days(d(2026, 11, 8)),
            (d(2025, 12, 1), d(2026, 11, 8))
        );
        assert_eq!(
            Year::Last12.days(d(2026, 1, 31)),
            (d(2025, 2, 1), d(2026, 1, 31))
        );
        assert_eq!(
            Year::Calendar(2025).days(d(2026, 11, 8)),
            (d(2025, 1, 1), d(2025, 12, 31))
        );
        assert_eq!(
            Year::Calendar(2026).days(d(2026, 11, 8)),
            (d(2026, 1, 1), d(2026, 11, 8))
        );
    }

    #[test]
    fn year_choices_go_back_to_the_first_session() {
        let today = d(2026, 11, 8);
        assert_eq!(
            Year::choices(Some(d(2024, 6, 1)), today),
            vec![
                Year::Last12,
                Year::Calendar(2026),
                Year::Calendar(2025),
                Year::Calendar(2024)
            ]
        );
        assert_eq!(
            Year::choices(None, today),
            vec![Year::Last12, Year::Calendar(2026)]
        );
    }

    #[test]
    fn ledger_splits_sessions_at_midnight() {
        let day = d(2026, 3, 10);
        let next = d(2026, 3, 11);
        let s = [
            // 22:00 to 01:30 the next day.
            sess("code", at(day, 22.0), Some(at(day, 25.5))),
            sess("read", at(day, 9.0), Some(at(day, 10.0))),
            sess("code", at(day, 13.0), Some(at(day, 14.0))),
        ];
        let l = Ledger::new(&s, day, next, at(next, 12.0));
        assert_eq!(
            pairs(l.on(day, |_| true)),
            vec![("code".into(), 3 * H), ("read".into(), H)]
        );
        assert_eq!(
            pairs(l.on(next, |_| true)),
            vec![("code".into(), H * 3 / 2)]
        );
        assert_eq!(
            l.per_day(|n| n == "code").into_iter().collect::<Vec<_>>(),
            vec![(day, 3 * H), (next, H * 3 / 2)]
        );
        assert_eq!(
            pairs(l.totals(|_| true)),
            vec![("code".into(), H * 9 / 2), ("read".into(), H)]
        );
    }

    #[test]
    fn ledger_clips_to_its_days_and_counts_running_time_to_now() {
        let (first, last) = (d(2026, 3, 10), d(2026, 3, 11));
        let s = [
            // Started the day before the ledger begins.
            sess("code", at(first, -2.0), Some(at(first, 1.0))),
            // Still running.
            sess("read", at(last, 23.0), None),
            // Ends exactly at midnight: nothing on the next day.
            sess("tea", at(first, 23.5), Some(at(last, 0.0))),
        ];
        let l = Ledger::new(&s, first, last, at(last, 23.25));
        assert_eq!(
            pairs(l.on(first, |_| true)),
            vec![("code".into(), H), ("tea".into(), H / 2)]
        );
        assert_eq!(pairs(l.on(last, |_| true)), vec![("read".into(), H / 4)]);
        assert!(l.on(first.pred_opt().unwrap(), |_| true).is_empty());
    }

    #[test]
    fn streaks() {
        let today = d(2026, 3, 20);
        let days =
            |ds: &[NaiveDate]| -> BTreeMap<NaiveDate, i64> { ds.iter().map(|x| (*x, H)).collect() };
        // 1–4 (four days), 10–11, then 18–19: today has nothing yet, so the
        // run up to yesterday still counts.
        let pd = days(&[
            d(2026, 3, 1),
            d(2026, 3, 2),
            d(2026, 3, 3),
            d(2026, 3, 4),
            d(2026, 3, 10),
            d(2026, 3, 11),
            d(2026, 3, 18),
            d(2026, 3, 19),
        ]);
        let s = summarize(&pd, today);
        assert_eq!(
            (s.longest_streak, s.current_streak, s.active_days),
            (4, 2, 8)
        );
        assert_eq!(s.total, 8 * H);
        // A gap before yesterday ends it.
        assert_eq!(summarize(&days(&[d(2026, 3, 18)]), today).current_streak, 0);
        assert_eq!(
            summarize(&days(&[d(2026, 3, 19), d(2026, 3, 20)]), today).current_streak,
            2
        );
        assert_eq!(summarize(&BTreeMap::new(), today), Summary::default());
    }

    #[test]
    fn months_cover_every_month_in_the_stretch() {
        let pd: BTreeMap<NaiveDate, i64> = [
            (d(2025, 12, 31), H),
            (d(2026, 2, 1), 2 * H),
            (d(2026, 2, 28), 3 * H),
        ]
        .into();
        assert_eq!(
            months(&pd, d(2025, 12, 1), d(2026, 2, 28)),
            vec![
                (d(2025, 12, 1), H),
                (d(2026, 1, 1), 0),
                (d(2026, 2, 1), 5 * H)
            ]
        );
        // Days outside the stretch don't count.
        assert_eq!(
            months(&pd, d(2026, 1, 1), d(2026, 1, 31)),
            vec![(d(2026, 1, 1), 0)]
        );
    }

    #[test]
    fn heat_levels() {
        let levels: Vec<u8> = [0, 1, H - 1, H, 4 * H, 8 * H, 20 * H]
            .into_iter()
            .map(heat_level)
            .collect();
        assert_eq!(levels, vec![0, 1, 1, 2, 3, 4, 4]);
    }
}
