//! macOS backend: an item in the menu bar (an NSStatusItem, via tray-icon)
//! on a winit event loop. Clicking it opens the menu, as is usual on macOS.
//! While something runs, the item shows the longest-running activity's
//! elapsed time next to the clock. No Dock icon.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use enoch::db::Db;
use tray_icon::menu::{IsMenuItem, Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem, Submenu};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};
use winit::application::ApplicationHandler;
use winit::event::{StartCause, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS};
use winit::window::WindowId;

use super::{Action, Entry, Tray, clock_rgba, elapsed, summary};

/// Icon size in pixels. The menu bar draws it 18 points tall, so this is
/// sharp on Retina displays.
const ICON_PX: u32 = 36;

pub fn run(db: Db) -> Result<()> {
    let mut builder = EventLoop::<MenuEvent>::with_user_event();
    builder
        // A menu bar item only: no Dock icon or app menu.
        .with_activation_policy(ActivationPolicy::Accessory)
        .with_default_menu(false);
    let event_loop = builder.build().context("starting the macOS event loop")?;

    // Menu clicks arrive on tray-icon's handler; forward them to the loop.
    let proxy = Mutex::new(event_loop.create_proxy());
    MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
        if let Ok(p) = proxy.lock() {
            let _ = p.send_event(e);
        }
    }));

    let mut app = App {
        tray: Tray::new(db),
        item: None,
        actions: HashMap::new(),
        failed: None,
    };
    event_loop
        .run_app(&mut app)
        .context("running the menu bar item")?;
    match app.failed {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

struct App {
    tray: Tray,
    item: Option<TrayIcon>,
    /// What each item of the current menu does, by its id.
    actions: HashMap<MenuId, Action>,
    /// Why the item couldn't be added, returned from `run`.
    failed: Option<anyhow::Error>,
}

impl App {
    fn create(&mut self) -> Result<()> {
        let item = TrayIconBuilder::new()
            .with_id("enoch")
            .with_tooltip("enoch")
            .build()
            .context("adding the menu bar item")?;
        self.item = Some(item);
        self.refresh();
        Ok(())
    }

    /// Redraw the item and rebuild its menu from the tray's state.
    fn refresh(&mut self) {
        let Some(item) = &self.item else { return };
        let mut actions = HashMap::new();
        let menu = Menu::new();
        for e in self.tray.menu() {
            let _ = menu.append(to_muda(&e, &mut actions).as_ref());
        }
        item.set_menu(Some(Box::new(menu)));
        self.actions = actions;

        // Idle: a template image, which macOS tints to suit a light or dark
        // menu bar. Running: the activity's color, drawn as is.
        let _ = match self.tray.icon_rgb() {
            Some(rgb) => item.set_icon(icon(rgb, true)),
            None => item.set_icon_templated(icon([0, 0, 0], false)),
        };
        item.set_title(self.tray.view.running.first().map(|(_, m)| elapsed(*m)));
        let _ = item.set_tooltip(Some(summary(&self.tray.view)));
    }
}

impl ApplicationHandler<MenuEvent> for App {
    fn new_events(&mut self, event_loop: &ActiveEventLoop, cause: StartCause) {
        let tick = match cause {
            // tray-icon wants the item added once the loop is running.
            StartCause::Init => {
                if let Err(e) = self.create() {
                    self.failed = Some(e);
                    event_loop.exit();
                    return;
                }
                true
            }
            StartCause::ResumeTimeReached { .. } => {
                if self.tray.reload() {
                    self.refresh();
                }
                true
            }
            // A menu click woke the loop early; the next tick stays due.
            _ => false,
        };
        if tick {
            event_loop.set_control_flow(ControlFlow::WaitUntil(
                Instant::now() + Duration::from_secs(1),
            ));
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, e: MenuEvent) {
        let Some(action) = self.actions.get(&e.id).cloned() else {
            return;
        };
        self.tray.apply(&action);
        if self.tray.quit {
            event_loop.exit();
            return;
        }
        self.refresh();
    }

    fn resumed(&mut self, _: &ActiveEventLoop) {}

    fn window_event(&mut self, _: &ActiveEventLoop, _: WindowId, _: WindowEvent) {}
}

fn icon(rgb: [u8; 3], filled: bool) -> Option<Icon> {
    Icon::from_rgba(clock_rgba(ICON_PX, rgb, filled), ICON_PX, ICON_PX).ok()
}

/// Build a muda item for `e`, recording what clickable items do.
fn to_muda(e: &Entry, actions: &mut HashMap<MenuId, Action>) -> Box<dyn IsMenuItem> {
    match e {
        Entry::Item { label, action, .. } => {
            let item = MenuItem::new(menu_escape(label), action.is_some(), None);
            if let Some(a) = action {
                actions.insert(item.id().clone(), a.clone());
            }
            Box::new(item)
        }
        Entry::Submenu { label, items, .. } => {
            let sub = Submenu::new(menu_escape(label), true);
            for child in items {
                let _ = sub.append(to_muda(child, actions).as_ref());
            }
            Box::new(sub)
        }
        Entry::Separator => Box::new(PredefinedMenuItem::separator()),
    }
}

/// muda reads `&` as a mnemonic marker and drops it; `&&` shows one.
fn menu_escape(s: &str) -> String {
    s.replace('&', "&&")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_labels_keep_ampersands() {
        assert_eq!(menu_escape("R&D"), "R&&D");
    }
}
