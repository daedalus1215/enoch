//! Geometry and drag math for the day timeline, free of egui so it can be
//! unit-tested: pixel <-> time mapping, snapping, which part of a bar the
//! pointer grabs, and how a drag reshapes a session.

pub const MINUTE: i64 = 60_000;
pub const HOUR: i64 = 60 * MINUTE;

/// Maps the time window `[from, to]` onto the pixel span `[x0, x1]`.
#[derive(Clone, Copy, Debug)]
pub struct Axis {
    pub from: i64,
    pub to: i64,
    pub x0: f32,
    pub x1: f32,
}

impl Axis {
    pub fn x(&self, ms: i64) -> f32 {
        let t = (ms - self.from) as f64 / (self.to - self.from) as f64;
        self.x0 + (t * (self.x1 - self.x0) as f64) as f32
    }

    /// Time under pixel `x`, clamped to the window.
    pub fn ms(&self, x: f32) -> i64 {
        let t = ((x - self.x0) / (self.x1 - self.x0)).clamp(0.0, 1.0) as f64;
        self.from + (t * (self.to - self.from) as f64).round() as i64
    }
}

/// Which part of a session bar a drag holds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Grip {
    Start,
    End,
    Body,
}

/// The grip for pointer `x` on a bar spanning `[bx0, bx1]`. Edges take up to
/// `edge` px each (less on narrow bars, so the middle stays movable); an edge
/// that is clipped off-screen cannot be grabbed and moves the body instead.
pub fn grip_at(x: f32, bx0: f32, bx1: f32, edge: f32, start_ok: bool, end_ok: bool) -> Grip {
    let e = edge.min((bx1 - bx0) / 3.0);
    if start_ok && x <= bx0 + e {
        Grip::Start
    } else if end_ok && x >= bx1 - e {
        Grip::End
    } else {
        Grip::Body
    }
}

/// Snapping and bounds applied while dragging.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    /// Snap points are `origin + k * step` (origin: local midnight).
    pub origin: i64,
    pub step: i64,
    /// Nothing may end after this (now).
    pub hi: i64,
    /// Shortest session a drag may produce.
    pub min_len: i64,
}

impl Limits {
    pub fn snap(&self, ms: i64) -> i64 {
        let k = ((ms - self.origin) as f64 / self.step as f64).round() as i64;
        self.origin + k * self.step
    }
}

/// Session `[start, end)` after dragging `grip` by `delta` ms. Moved edges
/// snap; a body drag keeps the length.
pub fn reshape(grip: Grip, start: i64, end: i64, delta: i64, lim: &Limits) -> (i64, i64) {
    match grip {
        Grip::Start => {
            let s = lim.snap(start + delta).min(end - lim.min_len);
            (s, end)
        }
        Grip::End => {
            let e = lim.snap(end + delta).min(lim.hi).max(start + lim.min_len);
            (start, e)
        }
        Grip::Body => {
            let len = end - start;
            let s = lim.snap(start + delta).min(lim.hi - len);
            (s, s + len)
        }
    }
}

/// A new session drawn from `a` to `b` (either direction), snapped and
/// ending no later than `lim.hi`.
pub fn span(a: i64, b: i64, lim: &Limits) -> (i64, i64) {
    let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
    (lim.snap(lo), lim.snap(hi).min(lim.hi))
}

/// The visible window for a day `[day.0, day.1)`: the extent of `points`
/// padded by half an hour and widened to whole hours, at least `min_span`
/// wide, and never outside the day. With no points, 08:00–20:00.
pub fn fit(day: (i64, i64), points: impl IntoIterator<Item = i64>, min_span: i64) -> (i64, i64) {
    let (d0, d1) = day;
    let mut lo = i64::MAX;
    let mut hi = i64::MIN;
    for p in points {
        lo = lo.min(p.clamp(d0, d1));
        hi = hi.max(p.clamp(d0, d1));
    }
    if lo > hi {
        return (d0 + 8 * HOUR, (d0 + 20 * HOUR).min(d1));
    }
    let floor_h = |t: i64| d0 + (t - d0).div_euclid(HOUR) * HOUR;
    let ceil_h = |t: i64| d0 + (t - d0 + HOUR - 1).div_euclid(HOUR) * HOUR;
    let mut lo = floor_h(lo - HOUR / 2).max(d0);
    let mut hi = ceil_h(hi + HOUR / 2).min(d1);
    let short = min_span - (hi - lo);
    if short > 0 {
        lo = floor_h(lo - short / 2);
        hi = lo + min_span;
        if lo < d0 {
            (lo, hi) = (d0, d0 + min_span);
        }
        if hi > d1 {
            (lo, hi) = (d1 - min_span, d1);
        }
    }
    (lo.max(d0), hi.min(d1))
}

/// Which labels to draw, given each one's horizontal extent `(left, right)`
/// in drawing order: each is kept unless it starts within `gap` px of the
/// last one kept.
pub fn spaced(spans: &[(f32, f32)], gap: f32) -> Vec<bool> {
    let mut free = f32::NEG_INFINITY;
    spans
        .iter()
        .map(|&(left, right)| {
            let keep = left >= free;
            if keep {
                free = right + gap;
            }
            keep
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_that_would_touch_are_left_out() {
        // A first label pulled right into its neighbour, which is dropped;
        // the one after that clears the first and stays.
        let spans = [(0.0, 38.0), (30.0, 68.0), (79.0, 117.0), (100.0, 138.0)];
        assert_eq!(spaced(&spans, 6.0), vec![true, false, true, false]);
        // Exactly `gap` apart is fine.
        assert_eq!(spaced(&[(0.0, 10.0), (16.0, 20.0)], 6.0), vec![true, true]);
        assert!(spaced(&[], 6.0).is_empty());
    }

    const DAY: (i64, i64) = (0, 24 * HOUR);

    fn lim() -> Limits {
        Limits {
            origin: 0,
            step: 5 * MINUTE,
            hi: 24 * HOUR,
            min_len: MINUTE,
        }
    }

    #[test]
    fn axis_round_trips() {
        let a = Axis {
            from: 8 * HOUR,
            to: 12 * HOUR,
            x0: 100.0,
            x1: 500.0,
        };
        assert_eq!(a.x(10 * HOUR), 300.0);
        assert_eq!(a.ms(300.0), 10 * HOUR);
        // Outside the axis clamps to the window.
        assert_eq!(a.ms(0.0), 8 * HOUR);
        assert_eq!(a.ms(900.0), 12 * HOUR);
    }

    #[test]
    fn snaps_to_nearest_step() {
        let l = lim();
        assert_eq!(l.snap(12 * MINUTE), 10 * MINUTE);
        assert_eq!(l.snap(13 * MINUTE), 15 * MINUTE);
        assert_eq!(l.snap(-2 * MINUTE), 0);
    }

    #[test]
    fn grips_edges_and_body() {
        assert_eq!(grip_at(101.0, 100.0, 200.0, 6.0, true, true), Grip::Start);
        assert_eq!(grip_at(198.0, 100.0, 200.0, 6.0, true, true), Grip::End);
        assert_eq!(grip_at(150.0, 100.0, 200.0, 6.0, true, true), Grip::Body);
        // Narrow bar: edges shrink so the middle third still moves it.
        assert_eq!(grip_at(105.0, 100.0, 109.0, 6.0, true, true), Grip::Body);
        // A clipped edge is not grabbable.
        assert_eq!(grip_at(101.0, 100.0, 200.0, 6.0, false, true), Grip::Body);
    }

    #[test]
    fn reshape_moves_one_edge_and_snaps() {
        let l = lim();
        let (s, e) = (9 * HOUR, 10 * HOUR);
        assert_eq!(
            reshape(Grip::Start, s, e, -13 * MINUTE, &l),
            (s - 15 * MINUTE, e)
        );
        assert_eq!(
            reshape(Grip::End, s, e, 22 * MINUTE, &l),
            (s, e + 20 * MINUTE)
        );
    }

    #[test]
    fn reshape_never_inverts() {
        let l = lim();
        let (s, e) = (9 * HOUR, 10 * HOUR);
        assert_eq!(reshape(Grip::Start, s, e, 3 * HOUR, &l), (e - MINUTE, e));
        assert_eq!(reshape(Grip::End, s, e, -3 * HOUR, &l), (s, s + MINUTE));
    }

    #[test]
    fn reshape_body_keeps_length_and_stops_at_now() {
        let mut l = lim();
        l.hi = 11 * HOUR;
        let (s, e) = (9 * HOUR, 10 * HOUR);
        assert_eq!(
            reshape(Grip::Body, s, e, 31 * MINUTE, &l),
            (s + 30 * MINUTE, e + 30 * MINUTE)
        );
        assert_eq!(
            reshape(Grip::Body, s, e, 5 * HOUR, &l),
            (10 * HOUR, 11 * HOUR)
        );
        assert_eq!(reshape(Grip::End, s, e, 5 * HOUR, &l), (s, 11 * HOUR));
    }

    #[test]
    fn span_orders_and_snaps() {
        let l = lim();
        assert_eq!(
            span(10 * HOUR + 2 * MINUTE, 9 * HOUR + 58 * MINUTE, &l),
            (10 * HOUR, 10 * HOUR)
        );
        assert_eq!(
            span(9 * HOUR + 58 * MINUTE, 10 * HOUR + 31 * MINUTE, &l),
            (10 * HOUR, 10 * HOUR + 30 * MINUTE)
        );
    }

    #[test]
    fn fit_pads_to_hours() {
        let r = fit(
            DAY,
            [9 * HOUR + 12 * MINUTE, 11 * HOUR + 34 * MINUTE],
            4 * HOUR,
        );
        assert_eq!(r, (8 * HOUR, 13 * HOUR));
    }

    #[test]
    fn fit_widens_short_spans_and_stays_in_day() {
        let r = fit(DAY, [10 * HOUR, 10 * HOUR + 10 * MINUTE], 6 * HOUR);
        assert_eq!(r.1 - r.0, 6 * HOUR);
        assert!(r.0 <= 10 * HOUR && r.1 >= 10 * HOUR + 10 * MINUTE);
        let early = fit(DAY, [10 * MINUTE], 6 * HOUR);
        assert_eq!(early, (0, 6 * HOUR));
        let late = fit(DAY, [23 * HOUR + 50 * MINUTE], 6 * HOUR);
        assert_eq!(late, (18 * HOUR, 24 * HOUR));
    }

    #[test]
    fn fit_empty_day_is_working_hours() {
        assert_eq!(fit(DAY, [], 6 * HOUR), (8 * HOUR, 20 * HOUR));
    }

    #[test]
    fn fit_clips_points_outside_the_day() {
        // A session from the previous evening is shown from midnight.
        let r = fit(DAY, [-3 * HOUR, 2 * HOUR], 4 * HOUR);
        assert_eq!(r.0, 0);
    }
}
