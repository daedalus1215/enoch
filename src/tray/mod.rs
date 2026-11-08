//! Tray icon (`enoch tray`): what is running, start/stop, and the window,
//! from the desktop's tray or the macOS menu bar.
//!
//! This module is the platform-neutral part: what the tray shows ([`View`]),
//! its menu as plain data ([`Entry`]), what the items do ([`Action`]) and
//! the icon's pixels. A backend puts that on screen:
//!
//! - `sni` (Linux, BSDs): a StatusNotifierItem over D-Bus, for Waybar's
//!   tray, the Omarchy shell, KDE and other SNI hosts.
//! - `macos`: an item in the menu bar.
//!
//! The icon is a small clock face, plain when idle and in the running
//! activity's color otherwise. Its menu stops running activities, starts
//! recent ones, and opens the window. Backends re-read the database once a
//! second, so starts and stops made anywhere else (CLI, TUI, window) show up
//! here too.
//!
//! The tray also notices time away from the computer (see `away`): after a
//! stretch away while something was tracked, it offers to take that time
//! out, in the menu and, on Linux, in a notification.

mod away;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(not(target_os = "macos"))]
mod notify;
#[cfg(not(target_os = "macos"))]
mod sni;
#[cfg(not(target_os = "macos"))]
mod wayland;

use anyhow::Result;
use away::Away;
use enoch::db::Db;
use enoch::palette;
use enoch::timefmt::{fmt_range, fmt_short, now_ms};

/// How many not-running activities the Start submenu offers.
const RECENT: usize = 12;

/// Run the tray. After `idle_after_ms` without input (0: never), time away
/// counts as away and the tray offers to drop it.
pub fn run(db: Db, idle_after_ms: i64) -> Result<()> {
    #[cfg(target_os = "macos")]
    return macos::run(db, idle_after_ms);
    #[cfg(not(target_os = "macos"))]
    return sni::run(db, idle_after_ms);
}

/// What the tray shows. Rebuilt every second; elapsed times are whole
/// minutes, so the icon and menu change at most once a minute while idle
/// of other changes.
#[derive(Clone, PartialEq, Default, Debug)]
struct View {
    /// Running activities with whole minutes elapsed, longest-running first.
    running: Vec<(String, i64)>,
    /// Activities not running, most recently started first.
    recent: Vec<String>,
}

impl View {
    fn load(db: &Db) -> Result<Self> {
        let now = now_ms();
        let running = db.running()?;
        let mut acts = db.activities()?;
        acts.sort_by_key(|a| std::cmp::Reverse(a.last));
        let recent = acts
            .into_iter()
            .map(|a| a.name)
            .filter(|n| !running.iter().any(|r| r.name.eq_ignore_ascii_case(n)))
            .take(RECENT)
            .collect();
        Ok(Self {
            running: running
                .into_iter()
                .map(|r| (r.name, (now - r.started_at).div_euclid(60_000)))
                .collect(),
            recent,
        })
    }
}

/// What choosing a menu item does.
#[derive(Clone, Debug, PartialEq)]
enum Action {
    Start(String),
    Stop(String),
    StopAll,
    #[cfg(feature = "gui")]
    OpenWindow,
    Quit,
    /// Take the offered stretch away out of what was tracked; what was
    /// still running carries on.
    DropAway,
    /// Take it out and stop what was running then, as of leaving.
    DropAwayAndStop,
    /// Count it after all.
    KeepAway,
}

/// A stretch away while something was tracked, waiting for the user to
/// keep or drop it.
#[derive(Clone, Debug, PartialEq)]
struct Offer {
    away: Away,
    /// What was running when the user left.
    names: Vec<String>,
}

impl Offer {
    /// `23m`.
    fn length(&self) -> String {
        fmt_short(self.away.to - self.away.from)
    }

    /// Notification title and body.
    #[cfg(not(target_os = "macos"))]
    fn text(&self) -> (String, String) {
        (
            format!("Away for {}", self.length()),
            format!(
                "{} kept running {}. Take that time out?",
                self.names.join(", "),
                fmt_range(self.away.from, self.away.to)
            ),
        )
    }
}

/// One menu entry, before a backend turns it into its own kind of item.
#[derive(Clone, Debug, PartialEq)]
enum Entry {
    /// A clickable item, or a grayed-out label when `action` is None.
    /// `icon` is a freedesktop icon name, for backends that show one.
    Item {
        label: String,
        action: Option<Action>,
        icon: Option<&'static str>,
    },
    Submenu {
        label: String,
        icon: Option<&'static str>,
        items: Vec<Entry>,
    },
    Separator,
}

impl Entry {
    fn item(label: impl Into<String>, action: Action) -> Self {
        Self::Item {
            label: label.into(),
            action: Some(action),
            icon: None,
        }
    }

    fn label(label: impl Into<String>) -> Self {
        Self::Item {
            label: label.into(),
            action: None,
            icon: None,
        }
    }

    fn with_icon(mut self, name: &'static str) -> Self {
        match &mut self {
            Self::Item { icon, .. } | Self::Submenu { icon, .. } => *icon = Some(name),
            Self::Separator => {}
        }
        self
    }
}

/// The tray's state and behavior, shared by the backends.
struct Tray {
    db: Db,
    view: View,
    /// The window this tray opened, so a second click doesn't open another.
    #[cfg(feature = "gui")]
    window: Option<std::process::Child>,
    error: Option<String>,
    /// Set by the Quit item; the backend's loop ends when it sees it.
    quit: bool,
    /// The latest stretch away not yet kept or dropped. A newer one
    /// replaces it, keeping the older stretch's time.
    offer: Option<Offer>,
}

impl Tray {
    fn new(db: Db) -> Self {
        let mut t = Self {
            db,
            view: View::default(),
            #[cfg(feature = "gui")]
            window: None,
            error: None,
            quit: false,
            offer: None,
        };
        t.reload();
        t
    }

    /// Re-read the database; true if what the tray shows changed.
    fn reload(&mut self) -> bool {
        let next = View::load(&self.db);
        self.show(next)
    }

    /// Show a freshly loaded view, or the error loading it; true if that
    /// changed anything.
    fn show(&mut self, next: Result<View>) -> bool {
        match next {
            Ok(v) => {
                let changed = v != self.view || self.error.is_some();
                self.view = v;
                self.error = None;
                changed
            }
            Err(e) => {
                let msg = format!("{e:#}");
                let changed = self.error.as_ref() != Some(&msg);
                self.error = Some(msg);
                changed
            }
        }
    }

    fn apply(&mut self, action: &Action) {
        match action {
            Action::Start(name) => {
                let r = self.db.start(name).map(drop);
                self.after(r);
            }
            Action::Stop(name) => {
                let r = self.db.stop(std::slice::from_ref(name)).map(drop);
                self.after(r);
            }
            Action::StopAll => {
                let r = self.db.stop(&[]).map(drop);
                self.after(r);
            }
            #[cfg(feature = "gui")]
            Action::OpenWindow => self.open_window(),
            Action::Quit => self.quit = true,
            Action::DropAway | Action::DropAwayAndStop => {
                let Some(o) = self.offer.take() else { return };
                let resume = *action == Action::DropAway;
                let r = self.db.cut_away(o.away.from, o.away.to, resume).map(drop);
                self.after(r);
            }
            Action::KeepAway => self.offer = None,
        }
    }

    /// Offer to drop `away` if anything was tracked when it began; true if
    /// it was offered.
    fn offer(&mut self, away: Away) -> bool {
        match self.db.tracked_at(away.from) {
            Ok(names) if !names.is_empty() => {
                self.offer = Some(Offer { away, names });
                true
            }
            Ok(_) => false,
            Err(e) => {
                self.error = Some(format!("{e:#}"));
                false
            }
        }
    }

    /// Reload after a change, or show why it failed.
    fn after(&mut self, r: Result<()>) {
        match r {
            Ok(()) => {
                self.reload();
            }
            Err(e) => self.error = Some(format!("{e:#}")),
        }
    }

    /// Launch `enoch gui`, unless the one we launched is still open.
    #[cfg(feature = "gui")]
    fn open_window(&mut self) {
        if let Some(child) = &mut self.window
            && matches!(child.try_wait(), Ok(None))
        {
            return;
        }
        let spawned = std::env::current_exe()
            .and_then(|exe| std::process::Command::new(exe).arg("gui").spawn());
        match spawned {
            Ok(child) => self.window = Some(child),
            Err(e) => self.error = Some(format!("opening the window: {e}")),
        }
    }

    /// The running activity's color, or None when nothing runs.
    fn icon_rgb(&self) -> Option<[u8; 3]> {
        self.view
            .running
            .first()
            .map(|(n, _)| palette::activity_rgb(n))
    }

    fn menu(&self) -> Vec<Entry> {
        let mut m = Vec::new();
        if let Some(e) = &self.error {
            m.push(Entry::label(format!("error: {e}")));
            m.push(Entry::Separator);
        }

        if let Some(o) = &self.offer {
            m.push(Entry::label(format!(
                "Away {} ({})",
                o.length(),
                fmt_range(o.away.from, o.away.to)
            )));
            m.push(Entry::item("Drop it, keep tracking", Action::DropAway));
            m.push(Entry::item("Drop it and stop", Action::DropAwayAndStop));
            m.push(Entry::item("Keep it", Action::KeepAway));
            m.push(Entry::Separator);
        }

        if self.view.running.is_empty() {
            m.push(Entry::label("Nothing running"));
        }
        for (name, mins) in &self.view.running {
            m.push(
                Entry::item(
                    format!("Stop {name} ({})", elapsed(*mins)),
                    Action::Stop(name.clone()),
                )
                .with_icon("media-playback-stop"),
            );
        }
        if self.view.running.len() > 1 {
            m.push(Entry::item("Stop all", Action::StopAll));
        }

        if !self.view.recent.is_empty() {
            m.push(Entry::Separator);
            m.push(Entry::Submenu {
                label: "Start".into(),
                icon: Some("media-playback-start"),
                items: self
                    .view
                    .recent
                    .iter()
                    .map(|n| Entry::item(n.clone(), Action::Start(n.clone())))
                    .collect(),
            });
        }

        m.push(Entry::Separator);
        #[cfg(feature = "gui")]
        m.push(Entry::item("Open window", Action::OpenWindow));
        m.push(Entry::item("Quit tray", Action::Quit));
        m
    }
}

/// `1h 5m`, `45m`, or `<1m`.
fn elapsed(mins: i64) -> String {
    if mins < 1 {
        "<1m".into()
    } else {
        fmt_short(mins * 60_000)
    }
}

/// Tooltip body: one line per running activity, or "Nothing running".
fn summary(view: &View) -> String {
    if view.running.is_empty() {
        return "Nothing running".into();
    }
    view.running
        .iter()
        .map(|(n, m)| format!("{n} · {}", elapsed(*m)))
        .collect::<Vec<_>>()
        .join("\n")
}

/// A `size`-px clock face (ring and two hands) in `rgb`, as straight
/// (non-premultiplied) RGBA rows. `filled` adds a faint fill inside the
/// ring, used while something runs.
fn clock_rgba(size: u32, rgb: [u8; 3], filled: bool) -> Vec<u8> {
    let s = size as f32;
    let c = s / 2.0;
    let r_out = s * 0.45;
    let ring = s * 0.11;
    let hand = s * 0.10;
    let [r, g, b] = rgb;
    let fill = if filled { 0.30 } else { 0.0 };
    // Hands at 12 and 2 o'clock, from the center.
    let hands = [(c, c - s * 0.26), (c + s * 0.17, c - s * 0.10)];

    let mut data = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            let d = ((px - c).powi(2) + (py - c).powi(2)).sqrt();
            // Coverage = how far inside each shape the pixel center is, in
            // pixels, clamped to [0, 1]: cheap anti-aliasing.
            let ring_a = (ring / 2.0 + 0.5 - (d - (r_out - ring / 2.0)).abs()).clamp(0.0, 1.0);
            let fill_a = (r_out - ring + 0.5 - d).clamp(0.0, 1.0) * fill;
            let hand_a = hands
                .iter()
                .map(|&(hx, hy)| {
                    (hand / 2.0 + 0.5 - seg_dist(px, py, c, c, hx, hy)).clamp(0.0, 1.0)
                })
                .fold(0.0f32, f32::max);
            let a = ring_a.max(fill_a).max(hand_a);
            data.extend_from_slice(&[r, g, b, (a * 255.0).round() as u8]);
        }
    }
    data
}

/// Distance from (px, py) to the segment (ax, ay)-(bx, by).
fn seg_dist(px: f32, py: f32, ax: f32, ay: f32, bx: f32, by: f32) -> f32 {
    let (dx, dy) = (bx - ax, by - ay);
    let t = (((px - ax) * dx + (py - ay) * dy) / (dx * dx + dy * dy)).clamp(0.0, 1.0);
    ((px - ax - t * dx).powi(2) + (py - ay - t * dy).powi(2)).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn alpha(rgba: &[u8], size: u32, x: u32, y: u32) -> u8 {
        rgba[((y * size + x) * 4 + 3) as usize]
    }

    fn tray(view: View) -> Tray {
        let mut t = Tray::new(Db::open_at(std::path::Path::new(":memory:")).unwrap());
        t.view = view;
        t
    }

    fn actions(entries: &[Entry]) -> Vec<Action> {
        entries
            .iter()
            .flat_map(|e| match e {
                Entry::Item { action, .. } => action.iter().cloned().collect(),
                Entry::Submenu { items, .. } => actions(items),
                Entry::Separator => vec![],
            })
            .collect()
    }

    #[test]
    fn icon_is_rgba_of_the_right_size() {
        let i = clock_rgba(32, [1, 2, 3], true);
        assert_eq!(i.len(), 32 * 32 * 4);
        assert_eq!(&i[0..3], &[1, 2, 3]);
    }

    #[test]
    fn icon_has_a_ring_and_transparent_corners() {
        let i = clock_rgba(32, [200; 3], false);
        assert_eq!(alpha(&i, 32, 0, 0), 0);
        // On the ring, straight left of center.
        assert!(alpha(&i, 32, 2, 16) > 200);
        // Unfilled: nothing between ring and hands.
        assert_eq!(alpha(&i, 32, 9, 22), 0);
        // Filled tints that area.
        assert!(alpha(&clock_rgba(32, [255, 0, 0], true), 32, 9, 22) > 0);
    }

    #[test]
    fn summary_lists_running() {
        let v = View {
            running: vec![("coding".into(), 65), ("tea".into(), 0)],
            recent: vec![],
        };
        assert_eq!(summary(&v), "coding · 1h 5m\ntea · <1m");
        assert_eq!(summary(&View::default()), "Nothing running");
    }

    #[test]
    fn idle_menu_says_so_and_offers_recent() {
        let t = tray(View {
            running: vec![],
            recent: vec!["coding".into(), "tea".into()],
        });
        let m = t.menu();
        assert_eq!(m[0], Entry::label("Nothing running"));
        let a = actions(&m);
        assert!(a.contains(&Action::Start("coding".into())));
        assert!(a.contains(&Action::Start("tea".into())));
        assert!(!a.contains(&Action::StopAll));
        assert_eq!(a.last(), Some(&Action::Quit));
    }

    #[test]
    fn running_menu_stops_each_and_all() {
        let t = tray(View {
            running: vec![("coding".into(), 65), ("tea".into(), 0)],
            recent: vec![],
        });
        let m = t.menu();
        assert!(matches!(&m[0], Entry::Item { label, .. } if label == "Stop coding (1h 5m)"));
        let a = actions(&m);
        assert!(a.contains(&Action::Stop("coding".into())));
        assert!(a.contains(&Action::Stop("tea".into())));
        assert!(a.contains(&Action::StopAll));
        // No Start submenu when nothing else exists.
        assert!(!m.iter().any(|e| matches!(e, Entry::Submenu { .. })));
    }

    #[test]
    fn away_is_offered_only_when_tracking() {
        let mut t = tray(View::default());
        assert!(!t.offer(Away { from: 1_000, to: 2_000 }));
        assert_eq!(t.offer, None);

        t.db.start("coding").unwrap();
        let started = t.db.running().unwrap()[0].started_at;
        let away = Away {
            from: started + 10_000,
            to: started + 20_000,
        };
        assert!(t.offer(away));
        assert_eq!(t.offer.as_ref().unwrap().names, vec!["coding".to_string()]);
        let m = t.menu();
        assert!(matches!(&m[0], Entry::Item { label, action: None, .. } if label.starts_with("Away 10s")));
        let a = actions(&m);
        assert_eq!(&a[..3], &[Action::DropAway, Action::DropAwayAndStop, Action::KeepAway]);

        t.apply(&Action::KeepAway);
        assert_eq!(t.offer, None);
        assert!(!actions(&t.menu()).contains(&Action::DropAway));
    }

    #[test]
    fn dropping_away_cuts_the_session() {
        let mut t = tray(View::default());
        t.db.start("coding").unwrap();
        let started = t.db.running().unwrap()[0].started_at;
        let away = Away {
            from: started + 1_000,
            to: started + 2_000,
        };
        assert!(t.offer(away));
        t.apply(&Action::DropAway);
        assert_eq!(t.offer, None);
        let r = t.db.running().unwrap();
        assert_eq!(r[0].started_at, away.to);

        assert!(t.offer(Away {
            from: away.to + 1_000,
            to: away.to + 2_000,
        }));
        t.apply(&Action::DropAwayAndStop);
        assert!(t.db.running().unwrap().is_empty());
        // Nothing offered: dropping is a no-op.
        t.apply(&Action::DropAway);
    }

    #[test]
    fn actions_change_the_database() {
        let mut t = tray(View::default());
        t.apply(&Action::Start("coding".into()));
        assert_eq!(t.view.running.len(), 1);
        assert_eq!(t.icon_rgb(), Some(palette::activity_rgb("coding")));
        t.apply(&Action::StopAll);
        assert!(t.view.running.is_empty());
        assert_eq!(t.view.recent, vec!["coding".to_string()]);
        assert_eq!(t.icon_rgb(), None);
        t.apply(&Action::Quit);
        assert!(t.quit);
    }
}
