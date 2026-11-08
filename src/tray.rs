//! Tray icon (`enoch tray`): a StatusNotifierItem for Waybar's tray, the
//! Omarchy shell, KDE and other SNI hosts.
//!
//! The icon is a small clock face, gray when idle and in the running
//! activity's color otherwise. Its tooltip lists what is running; its menu
//! stops running activities, starts recent ones, and opens the window.
//!
//! The process polls the database once a second, so starts and stops made
//! anywhere else (CLI, TUI, window) show up here too.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{Context, Result};
use enoch::db::Db;
use enoch::palette;
use enoch::timefmt::{fmt_short, now_ms};
use ksni::blocking::TrayMethods;
use ksni::menu::{StandardItem, SubMenu};
use ksni::{Icon, MenuItem, ToolTip};

/// How many not-running activities the Start submenu offers.
const RECENT: usize = 12;

/// Idle icon color.
const IDLE_RGB: [u8; 3] = [200, 200, 200];

pub fn run(db: Db) -> Result<()> {
    let quit = Arc::new(AtomicBool::new(false));
    let mut tray = EnochTray {
        // Menu actions run on the tray's own thread, with their own connection.
        db: Db::open()?,
        view: View::default(),
        #[cfg(feature = "gui")]
        window: None,
        quit: quit.clone(),
        error: None,
    };
    tray.reload();
    // Keep running if no tray host is up yet (e.g. started before the bar
    // at login); the icon appears once one registers.
    let handle = tray
        .assume_sni_available(true)
        .spawn()
        .context("starting the tray icon over D-Bus")?;

    while !quit.load(Ordering::Relaxed) && !handle.is_closed() {
        std::thread::sleep(Duration::from_secs(1));
        let next = View::load(&db);
        handle.update(|t| match next {
            Ok(v) if v != t.view => {
                t.view = v;
                t.error = None;
            }
            Ok(_) => {}
            Err(e) => t.error = Some(format!("{e:#}")),
        });
    }
    handle.shutdown().wait();
    Ok(())
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

struct EnochTray {
    db: Db,
    view: View,
    /// The window this tray opened, so a second click doesn't open another.
    #[cfg(feature = "gui")]
    window: Option<std::process::Child>,
    quit: Arc<AtomicBool>,
    error: Option<String>,
}

impl EnochTray {
    fn reload(&mut self) {
        match View::load(&self.db) {
            Ok(v) => {
                self.view = v;
                self.error = None;
            }
            Err(e) => self.error = Some(format!("{e:#}")),
        }
    }

    fn start(&mut self, name: &str) {
        if let Err(e) = self.db.start(name) {
            self.error = Some(format!("{e:#}"));
            return;
        }
        self.reload();
    }

    /// Stop these activities; empty stops everything.
    fn stop(&mut self, names: &[String]) {
        if let Err(e) = self.db.stop(names) {
            self.error = Some(format!("{e:#}"));
            return;
        }
        self.reload();
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
}

impl ksni::Tray for EnochTray {
    fn id(&self) -> String {
        "enoch".into()
    }

    fn title(&self) -> String {
        "enoch".into()
    }

    fn icon_pixmap(&self) -> Vec<Icon> {
        let rgb = self
            .view
            .running
            .first()
            .map(|(n, _)| palette::activity_rgb(n));
        [22, 32, 48].iter().map(|&s| clock_icon(s, rgb)).collect()
    }

    fn tool_tip(&self) -> ToolTip {
        ToolTip {
            title: "enoch".into(),
            description: summary(&self.view),
            ..Default::default()
        }
    }

    /// Left click opens the window.
    fn activate(&mut self, _x: i32, _y: i32) {
        #[cfg(feature = "gui")]
        self.open_window();
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        let mut m: Vec<MenuItem<Self>> = Vec::new();
        if let Some(e) = &self.error {
            m.push(label_item(format!("error: {e}")));
            m.push(MenuItem::Separator);
        }

        if self.view.running.is_empty() {
            m.push(label_item("Nothing running".into()));
        }
        for (name, mins) in &self.view.running {
            let n = name.clone();
            m.push(
                StandardItem {
                    label: format!("Stop {} ({})", menu_escape(name), elapsed(*mins)),
                    icon_name: "media-playback-stop".into(),
                    activate: Box::new(move |t: &mut Self| t.stop(std::slice::from_ref(&n))),
                    ..Default::default()
                }
                .into(),
            );
        }
        if self.view.running.len() > 1 {
            m.push(
                StandardItem {
                    label: "Stop all".into(),
                    activate: Box::new(|t: &mut Self| t.stop(&[])),
                    ..Default::default()
                }
                .into(),
            );
        }

        if !self.view.recent.is_empty() {
            m.push(MenuItem::Separator);
            let items = self
                .view
                .recent
                .iter()
                .map(|name| {
                    let n = name.clone();
                    StandardItem {
                        label: menu_escape(name),
                        activate: Box::new(move |t: &mut Self| t.start(&n)),
                        ..Default::default()
                    }
                    .into()
                })
                .collect();
            m.push(
                SubMenu {
                    label: "Start".into(),
                    icon_name: "media-playback-start".into(),
                    submenu: items,
                    ..Default::default()
                }
                .into(),
            );
        }

        m.push(MenuItem::Separator);
        #[cfg(feature = "gui")]
        m.push(
            StandardItem {
                label: "Open window".into(),
                activate: Box::new(|t: &mut Self| t.open_window()),
                ..Default::default()
            }
            .into(),
        );
        m.push(
            StandardItem {
                label: "Quit tray".into(),
                activate: Box::new(|t: &mut Self| t.quit.store(true, Ordering::Relaxed)),
                ..Default::default()
            }
            .into(),
        );
        m
    }

    /// The host went away (e.g. the bar restarted): keep running so the
    /// icon comes back when it does.
    fn watcher_offline(&self, _reason: ksni::OfflineReason) -> bool {
        true
    }
}

fn label_item(label: String) -> MenuItem<EnochTray> {
    StandardItem {
        label: menu_escape(&label),
        enabled: false,
        ..Default::default()
    }
    .into()
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

/// dbusmenu labels treat `_` as a mnemonic marker; double it to show one.
fn menu_escape(s: &str) -> String {
    s.replace('_', "__")
}

/// A `size`-px clock face (ring and two hands) as ARGB32 in network byte
/// order, as StatusNotifierItem wants. Gray when `rgb` is None; otherwise
/// in that color, with a faint fill.
fn clock_icon(size: i32, rgb: Option<[u8; 3]>) -> Icon {
    let s = size as f32;
    let c = s / 2.0;
    let r_out = s * 0.45;
    let ring = s * 0.11;
    let hand = s * 0.10;
    let [r, g, b] = rgb.unwrap_or(IDLE_RGB);
    let fill = if rgb.is_some() { 0.30 } else { 0.0 };
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
            data.extend_from_slice(&[(a * 255.0).round() as u8, r, g, b]);
        }
    }
    Icon {
        width: size,
        height: size,
        data,
    }
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

    fn alpha(icon: &Icon, x: i32, y: i32) -> u8 {
        icon.data[((y * icon.width + x) * 4) as usize]
    }

    #[test]
    fn icon_is_argb_of_the_right_size() {
        let i = clock_icon(32, Some([1, 2, 3]));
        assert_eq!((i.width, i.height, i.data.len()), (32, 32, 32 * 32 * 4));
        // Color bytes follow alpha, in network order.
        assert_eq!(&i.data[1..4], &[1, 2, 3]);
    }

    #[test]
    fn icon_has_a_ring_and_transparent_corners() {
        let i = clock_icon(32, None);
        assert_eq!(alpha(&i, 0, 0), 0);
        // On the ring, straight left of center.
        assert!(alpha(&i, 2, 16) > 200);
        // Idle has no fill between ring and hands.
        assert_eq!(alpha(&i, 9, 22), 0);
        // Running tints that area.
        assert!(alpha(&clock_icon(32, Some([255, 0, 0])), 9, 22) > 0);
    }

    #[test]
    fn menu_labels_keep_underscores() {
        assert_eq!(menu_escape("deep_work"), "deep__work");
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
}
