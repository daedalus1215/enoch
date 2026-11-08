//! Idle probe for Wayland compositors with ext-idle-notify-v1 (Hyprland,
//! Sway, KDE, ...). The compositor says when there has been no
//! input for the threshold, and when input comes back.
//!
//! The notification honors idle inhibitors, so a playing video or a
//! presentation that keeps the screen awake also keeps you "present".

use std::sync::mpsc::Sender;

use anyhow::{Context, Result};
use enoch::timefmt::now_ms;
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::{wl_registry, wl_seat};
use wayland_client::{Connection, Dispatch, QueueHandle};
use wayland_protocols::ext::idle_notify::v1::client::ext_idle_notification_v1::{
    self, ExtIdleNotificationV1,
};
use wayland_protocols::ext::idle_notify::v1::client::ext_idle_notifier_v1::ExtIdleNotifierV1;

/// What the probe saw, in unix ms.
pub enum Signal {
    /// No input since then.
    Idle(i64),
    /// Input at that moment.
    Active(i64),
}

/// Watch for idleness of `threshold_ms` on a thread of its own, sending
/// what it sees to `tx`. Errors if there is no Wayland display or the
/// compositor lacks the protocol.
pub fn watch(threshold_ms: i64, tx: Sender<Signal>) -> Result<()> {
    let conn = Connection::connect_to_env().context("connecting to the Wayland display")?;
    let (globals, mut queue) =
        registry_queue_init::<Probe>(&conn).context("listing Wayland globals")?;
    let qh = queue.handle();
    let seat: wl_seat::WlSeat = globals.bind(&qh, 1..=1, ()).context("no Wayland seat")?;
    let notifier: ExtIdleNotifierV1 = globals
        .bind(&qh, 1..=1, ())
        .context("the compositor doesn't offer ext-idle-notify-v1")?;
    let timeout = threshold_ms.clamp(1, u32::MAX as i64) as u32;
    let notification = notifier.get_idle_notification(timeout, &seat, &qh, ());
    let mut probe = Probe { tx, threshold_ms };
    std::thread::Builder::new()
        .name("enoch-idle".into())
        .spawn(move || {
            // Keep the objects for as long as the thread watches.
            let _keep = (seat, notifier, notification);
            while queue.blocking_dispatch(&mut probe).is_ok() {}
        })
        .context("starting the idle watcher")?;
    Ok(())
}

struct Probe {
    tx: Sender<Signal>,
    threshold_ms: i64,
}

impl Dispatch<ExtIdleNotificationV1, ()> for Probe {
    fn event(
        probe: &mut Self,
        _: &ExtIdleNotificationV1,
        event: ext_idle_notification_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let now = now_ms();
        let signal = match event {
            // Sent once the threshold has passed without input.
            ext_idle_notification_v1::Event::Idled => Signal::Idle(now - probe.threshold_ms),
            ext_idle_notification_v1::Event::Resumed => Signal::Active(now),
            _ => return,
        };
        let _ = probe.tx.send(signal);
    }
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for Probe {
    fn event(
        _: &mut Self,
        _: &wl_registry::WlRegistry,
        _: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_seat::WlSeat, ()> for Probe {
    fn event(
        _: &mut Self,
        _: &wl_seat::WlSeat,
        _: wl_seat::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ExtIdleNotifierV1, ()> for Probe {
    fn event(
        _: &mut Self,
        _: &ExtIdleNotifierV1,
        _: <ExtIdleNotifierV1 as wayland_client::Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}
