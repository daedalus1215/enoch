//! The desktop's light or dark preference, where winit can't tell.
//!
//! On macOS winit reports the system theme itself. On Linux and the BSDs
//! it reports none, so egui's "follow the system" would always mean dark;
//! there the preference comes from the XDG desktop portal's
//! `org.freedesktop.appearance` `color-scheme` setting, which GNOME, KDE
//! and the gtk portal (under Hyprland, Sway, ...) all serve. The window
//! follows it as it changes.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use eframe::egui::{Context, Theme};

/// The system theme as last heard, followed in the background where that
/// needs doing.
pub struct SystemTheme {
    /// A portal `color-scheme` value; stays 0 where there is no portal.
    scheme: Arc<AtomicU32>,
}

impl SystemTheme {
    /// Start following the system theme, repainting `ctx` when it changes.
    /// Waits briefly for the first answer so the window doesn't open in
    /// the wrong theme and then flip.
    pub fn watch(ctx: &Context) -> Self {
        #[cfg(all(unix, not(target_os = "macos")))]
        let scheme = portal::watch(ctx);
        #[cfg(not(all(unix, not(target_os = "macos"))))]
        let scheme = {
            let _ = ctx;
            Arc::new(AtomicU32::new(0))
        };
        Self { scheme }
    }

    /// The system theme, if known here. `None` leaves it to winit.
    pub fn get(&self) -> Option<Theme> {
        from_scheme(self.scheme.load(Ordering::Relaxed))
    }
}

/// A portal `color-scheme` value as a theme: 1 prefers dark, 2 prefers
/// light, and 0 (no preference) or anything newer says nothing.
fn from_scheme(value: u32) -> Option<Theme> {
    match value {
        1 => Some(Theme::Dark),
        2 => Some(Theme::Light),
        _ => None,
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
mod portal {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::mpsc;
    use std::time::Duration;

    use eframe::egui::Context;
    use zbus::blocking::{Connection, Proxy};
    use zbus::zvariant::{OwnedValue, Value};

    const NAMESPACE: &str = "org.freedesktop.appearance";
    const KEY: &str = "color-scheme";
    /// How long opening the window waits for the portal's first answer.
    const FIRST_ANSWER: Duration = Duration::from_millis(300);

    /// Read the color scheme and keep it up to date from a thread; the
    /// value stays 0 (no preference) without a session bus or portal.
    pub fn watch(ctx: &Context) -> Arc<AtomicU32> {
        let scheme = Arc::new(AtomicU32::new(0));
        let (answered, first) = mpsc::channel();
        let (cell, ctx) = (scheme.clone(), ctx.clone());
        let spawned = std::thread::Builder::new()
            .name("enoch-theme".into())
            .spawn(move || {
                // Without a portal, "System" falls back to dark, as before.
                let _ = follow(&cell, &ctx, answered);
            });
        if spawned.is_ok() {
            let _ = first.recv_timeout(FIRST_ANSWER);
        }
        scheme
    }

    fn follow(cell: &AtomicU32, ctx: &Context, answered: mpsc::Sender<()>) -> zbus::Result<()> {
        let conn = Connection::session()?;
        let proxy = Proxy::new(
            &conn,
            "org.freedesktop.portal.Desktop",
            "/org/freedesktop/portal/desktop",
            "org.freedesktop.portal.Settings",
        )?;
        // Subscribe before reading, so a change in between isn't missed.
        let changes = proxy.receive_signal("SettingChanged")?;
        if let Some(v) = read(&proxy) {
            cell.store(v, Ordering::Relaxed);
            // In case the window stopped waiting for this answer.
            ctx.request_repaint();
        }
        let _ = answered.send(());
        for msg in changes {
            let Ok((ns, key, value)) = msg.body().deserialize::<(String, String, OwnedValue)>()
            else {
                continue;
            };
            if ns == NAMESPACE
                && key == KEY
                && let Some(v) = scheme(&value)
            {
                cell.store(v, Ordering::Relaxed);
                ctx.request_repaint();
            }
        }
        Ok(())
    }

    /// The current color scheme. `ReadOne` (portal version 2) answers
    /// with the value; the older `Read` wraps it in one more variant.
    fn read(proxy: &Proxy) -> Option<u32> {
        proxy
            .call::<_, _, OwnedValue>("ReadOne", &(NAMESPACE, KEY))
            .or_else(|_| proxy.call::<_, _, OwnedValue>("Read", &(NAMESPACE, KEY)))
            .ok()
            .and_then(|v| scheme(&v))
    }

    /// A `color-scheme` value, unwrapping any variants around it.
    pub(super) fn scheme(v: &Value) -> Option<u32> {
        match v {
            Value::U32(n) => Some(*n),
            Value::Value(inner) => scheme(inner),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn color_scheme_values() {
        assert_eq!(from_scheme(0), None);
        assert_eq!(from_scheme(1), Some(Theme::Dark));
        assert_eq!(from_scheme(2), Some(Theme::Light));
        assert_eq!(from_scheme(3), None);
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn read_and_read_one_answers() {
        use zbus::zvariant::Value;
        // ReadOne: the value itself; Read: the value in another variant.
        assert_eq!(portal::scheme(&Value::U32(2)), Some(2));
        let wrapped = Value::Value(Box::new(Value::U32(1)));
        assert_eq!(portal::scheme(&wrapped), Some(1));
        assert_eq!(portal::scheme(&Value::from("dark")), None);
    }
}
