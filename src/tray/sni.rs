//! StatusNotifierItem backend (Linux, BSDs): the tray over D-Bus, for
//! Waybar's tray, the Omarchy shell, KDE and other SNI hosts. Left click
//! opens the window; the menu is the shared one.

use std::time::Duration;

use anyhow::{Context, Result};
use enoch::db::Db;
use ksni::blocking::TrayMethods;
use ksni::menu::{StandardItem, SubMenu};
use ksni::{Icon, MenuItem, ToolTip};

use super::{Entry, Tray, View, clock_rgba, summary};

/// Idle icon color.
const IDLE_RGB: [u8; 3] = [200, 200, 200];

pub fn run(db: Db) -> Result<()> {
    // Menu actions run on the tray's own thread, with their own connection.
    let tray = SniTray(Tray::new(Db::open()?));
    // Keep running if no tray host is up yet (e.g. started before the bar
    // at login); the icon appears once one registers.
    let handle = tray
        .assume_sni_available(true)
        .spawn()
        .context("starting the tray icon over D-Bus")?;

    loop {
        std::thread::sleep(Duration::from_secs(1));
        let next = View::load(&db);
        // ksni diffs the icon, tooltip and menu itself before signalling.
        match handle.update(|t| {
            t.0.show(next);
            t.0.quit
        }) {
            Some(false) => {}
            // Quit chosen, or the service ended.
            Some(true) | None => break,
        }
    }
    handle.shutdown().wait();
    Ok(())
}

struct SniTray(Tray);

impl ksni::Tray for SniTray {
    fn id(&self) -> String {
        "enoch".into()
    }

    fn title(&self) -> String {
        "enoch".into()
    }

    fn icon_pixmap(&self) -> Vec<Icon> {
        let rgb = self.0.icon_rgb();
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
            description: summary(&self.0.view),
            ..Default::default()
        }
    }

    /// Left click opens the window.
    fn activate(&mut self, _x: i32, _y: i32) {
        #[cfg(feature = "gui")]
        self.0.apply(&super::Action::OpenWindow);
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        self.0.menu().iter().map(to_ksni).collect()
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
                item.activate = Box::new(move |t: &mut SniTray| t.0.apply(&a));
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
