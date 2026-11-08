//! Desktop notifications with action buttons, over
//! org.freedesktop.Notifications (mako, dunst, swaync, the Omarchy shell,
//! KDE, GNOME, ...).

use std::collections::HashMap;

use anyhow::{Context, Result};
use zbus::blocking::{Connection, Proxy};
use zbus::zvariant::Value;

pub struct Notifier {
    proxy: Proxy<'static>,
}

impl Notifier {
    pub fn new() -> Result<Self> {
        let conn = Connection::session().context("connecting to the session bus")?;
        let proxy = Proxy::new(
            &conn,
            "org.freedesktop.Notifications",
            "/org/freedesktop/Notifications",
            "org.freedesktop.Notifications",
        )
        .context("reaching the notification server")?;
        Ok(Self { proxy })
    }

    /// Show a notification that stays until acted on or dismissed,
    /// replacing notification `replaces` if not 0. `actions` are
    /// (key, button label) pairs. Returns its id.
    pub fn show(
        &self,
        replaces: u32,
        summary: &str,
        body: &str,
        actions: &[(&str, &str)],
    ) -> Result<u32> {
        let actions: Vec<&str> = actions.iter().flat_map(|(k, l)| [*k, *l]).collect();
        let hints: HashMap<&str, Value> = HashMap::from([("urgency", Value::U8(1))]);
        let expire_never = 0i32;
        self.proxy
            .call(
                "Notify",
                &("enoch", replaces, "", summary, body, actions, hints, expire_never),
            )
            .context("showing a notification")
    }

    pub fn close(&self, id: u32) {
        let _: zbus::Result<()> = self.proxy.call("CloseNotification", &(id,));
    }

    /// Call `f(id, action key)` on a thread of its own each time someone
    /// picks an action on any notification.
    pub fn on_action(&self, f: impl Fn(u32, String) + Send + 'static) -> Result<()> {
        let signals = self
            .proxy
            .receive_signal("ActionInvoked")
            .context("listening for notification actions")?;
        std::thread::Builder::new()
            .name("enoch-notify".into())
            .spawn(move || {
                for msg in signals {
                    if let Ok((id, key)) = msg.body().deserialize::<(u32, String)>() {
                        f(id, key);
                    }
                }
            })
            .context("starting the notification listener")?;
        Ok(())
    }
}
