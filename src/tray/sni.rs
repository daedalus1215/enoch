//! StatusNotifierItem backend (Linux, BSDs): the tray over D-Bus, for
//! Waybar's tray, the Omarchy shell, KDE and other SNI hosts. Left click
//! opens the window; the menu is the shared one.
//!
//! Time away comes from the Wayland idle probe and the tick's sleep check;
//! an offer to drop it also goes out as a notification, whose buttons act
//! like the menu's.

use std::sync::mpsc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use enoch::db::Db;
use enoch::timefmt::now_ms;
use ksni::blocking::TrayMethods;
use ksni::menu::{StandardItem, SubMenu};
use ksni::{Icon, MenuItem, ToolTip};

use super::away::Tracker;
use super::notify::Notifier;
use super::wayland::{self, Signal};
use super::{Action, Entry, Tray, View, clock_rgba, summary};

/// Idle icon color.
const IDLE_RGB: [u8; 3] = [200, 200, 200];

/// Notification buttons, as (action key, label).
const OFFER_BUTTONS: [(&str, &str); 3] = [
    ("drop", "Drop it"),
    ("stop", "Drop and stop"),
    ("keep", "Keep it"),
];

pub fn run(db: Db, idle_after_ms: i64) -> Result<()> {
    let mut tracker = Tracker::new(idle_after_ms);
    let (tx, idle) = mpsc::channel();
    if tracker.threshold() > 0
        && let Err(e) = wayland::watch(tracker.threshold(), tx)
    {
        eprintln!("enoch tray: no idle detection, only sleep: {e:#}");
    }
    let notifier = Notifier::new()
        .inspect_err(|e| eprintln!("enoch tray: no notifications: {e:#}"))
        .ok();

    // Menu actions run on the tray's own thread, with their own connection.
    let tray = SniTray {
        tray: Tray::new(Db::open()?),
        note: None,
    };
    // Keep running if no tray host is up yet (e.g. started before the bar
    // at login); the icon appears once one registers.
    let handle = tray
        .assume_sni_available(true)
        .spawn()
        .context("starting the tray icon over D-Bus")?;

    if let Some(n) = &notifier {
        let handle = handle.clone();
        let listening = n.on_action(move |id, key| {
            let action = match key.as_str() {
                "drop" => Action::DropAway,
                "stop" => Action::DropAwayAndStop,
                "keep" => Action::KeepAway,
                _ => return,
            };
            handle.update(|t| {
                if t.note == Some(id) {
                    // The server closes it on an action.
                    t.note = None;
                    t.tray.apply(&action);
                }
            });
        });
        if let Err(e) = listening {
            eprintln!("enoch tray: notification buttons won't work: {e:#}");
        }
    }

    let awake = Instant::now();
    loop {
        std::thread::sleep(Duration::from_secs(1));
        let mut stretches: Vec<_> = idle
            .try_iter()
            .filter_map(|s| match s {
                Signal::Idle(since) => {
                    tracker.idle(since);
                    None
                }
                Signal::Active(at) => tracker.active(at),
            })
            .collect();
        stretches.extend(tracker.tick(now_ms(), awake.elapsed().as_millis() as i64));
        let next = View::load(&db);

        // ksni diffs the icon, tooltip and menu itself before signalling.
        let Some(tick) = handle.update(|t| {
            t.tray.show(next);
            let offered = stretches.into_iter().fold(false, |o, a| t.tray.offer(a) || o);
            Tick {
                quit: t.tray.quit,
                note: t.note,
                // Resolved from the menu: the notification can go.
                stale: t.note.filter(|_| t.tray.offer.is_none()),
                offer: offered.then(|| t.tray.offer.as_ref().map(|o| o.text())).flatten(),
            }
        }) else {
            break; // The service ended.
        };
        if tick.quit {
            break;
        }
        if let Some(n) = &notifier {
            if let Some(id) = tick.stale {
                n.close(id);
                handle.update(|t| t.note = t.note.filter(|&n| n != id));
            }
            if let Some((title, body)) = tick.offer {
                match n.show(tick.note.unwrap_or(0), &title, &body, &OFFER_BUTTONS) {
                    Ok(id) => {
                        handle.update(|t| t.note = Some(id));
                    }
                    Err(e) => eprintln!("enoch tray: {e:#}"),
                }
            }
        }
    }
    if let (Some(n), Some(Some(id))) = (&notifier, handle.update(|t| t.note)) {
        n.close(id);
    }
    handle.shutdown().wait();
    Ok(())
}

/// What the loop learns from one update of the tray.
struct Tick {
    quit: bool,
    /// The notification showing an offer, if any.
    note: Option<u32>,
    /// A notification whose offer was resolved from the menu.
    stale: Option<u32>,
    /// A new offer to notify about, as title and body.
    offer: Option<(String, String)>,
}

struct SniTray {
    tray: Tray,
    /// The notification showing the current offer, if any.
    note: Option<u32>,
}

impl ksni::Tray for SniTray {
    fn id(&self) -> String {
        "enoch".into()
    }

    fn title(&self) -> String {
        "enoch".into()
    }

    fn icon_pixmap(&self) -> Vec<Icon> {
        let rgb = self.tray.icon_rgb();
        [22, 32, 48]
            .iter()
            .map(|&s| Icon {
                width: s,
                height: s,
                data: argb(clock_rgba(s as u32, rgb.unwrap_or(IDLE_RGB), rgb.is_some())),
            })
            .collect()
    }

    fn tool_tip(&self) -> ToolTip {
        ToolTip {
            title: "enoch".into(),
            description: summary(&self.tray.view),
            ..Default::default()
        }
    }

    /// Left click opens the window.
    fn activate(&mut self, _x: i32, _y: i32) {
        #[cfg(feature = "gui")]
        self.tray.apply(&Action::OpenWindow);
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        self.tray.menu().iter().map(to_ksni).collect()
    }

    /// The host went away (e.g. the bar restarted): keep running so the
    /// icon comes back when it does.
    fn watcher_offline(&self, _reason: ksni::OfflineReason) -> bool {
        true
    }
}

fn to_ksni(e: &Entry) -> MenuItem<SniTray> {
    match e {
        Entry::Item {
            label,
            action,
            icon,
        } => {
            let mut item = StandardItem {
                label: menu_escape(label),
                icon_name: icon.unwrap_or_default().into(),
                enabled: action.is_some(),
                ..Default::default()
            };
            if let Some(a) = action.clone() {
                item.activate = Box::new(move |t: &mut SniTray| t.tray.apply(&a));
            }
            item.into()
        }
        Entry::Submenu { label, icon, items } => SubMenu {
            label: menu_escape(label),
            icon_name: icon.unwrap_or_default().into(),
            submenu: items.iter().map(to_ksni).collect(),
            ..Default::default()
        }
        .into(),
        Entry::Separator => MenuItem::Separator,
    }
}

/// RGBA to the ARGB32 in network byte order that StatusNotifierItem wants.
fn argb(mut rgba: Vec<u8>) -> Vec<u8> {
    for px in rgba.chunks_exact_mut(4) {
        px.rotate_right(1);
    }
    rgba
}

/// dbusmenu labels treat `_` as a mnemonic marker; double it to show one.
fn menu_escape(s: &str) -> String {
    s.replace('_', "__")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pixels_are_argb() {
        assert_eq!(argb(vec![1, 2, 3, 4, 5, 6, 7, 8]), vec![4, 1, 2, 3, 8, 5, 6, 7]);
    }

    #[test]
    fn menu_labels_keep_underscores() {
        assert_eq!(menu_escape("deep_work"), "deep__work");
    }
}
