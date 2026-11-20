//! The notification server: `org.freedesktop.Notifications` on the session bus.
//!
//! Only one program can own the name. While another daemon has it, this
//! keeps asking every few seconds, so quitting that daemon hands the job over
//! without restarting the shell.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::thread;
use std::time::Duration;

use smithay_client_toolkit::reexports::calloop::channel::Sender;
use zbus::blocking::Connection;
use zbus::zvariant::OwnedValue;

pub const PATH: &str = "/org/freedesktop/Notifications";
pub const INTERFACE: &str = "org.freedesktop.Notifications";

#[derive(Clone, Debug)]
pub struct Incoming {
    pub id: u32,
    pub app: String,
    pub summary: String,
    pub body: String,
    /// (key, label) pairs; `default` is what clicking the card does.
    pub actions: Vec<(String, String)>,
    /// 0 low, 1 normal, 2 critical.
    pub urgency: u8,
    /// Milliseconds; -1 the server decides, 0 never.
    pub timeout: i32,
}

pub enum Event {
    Ready(Connection),
    Notify(Incoming),
    Close(u32),
}

struct Server {
    events: Sender<Event>,
    next: AtomicU32,
}

#[zbus::interface(name = "org.freedesktop.Notifications")]
impl Server {
    fn get_capabilities(&self) -> Vec<&'static str> {
        vec!["body", "actions", "persistence"]
    }

    #[allow(clippy::too_many_arguments)]
    fn notify(
        &self,
        app_name: String,
        replaces_id: u32,
        _app_icon: String,
        summary: String,
        body: String,
        actions: Vec<String>,
        hints: HashMap<String, OwnedValue>,
        expire_timeout: i32,
    ) -> u32 {
        let id = if replaces_id > 0 { replaces_id } else { self.next.fetch_add(1, Ordering::Relaxed) };
        let urgency = hints.get("urgency").and_then(|v| u8::try_from(v).ok()).unwrap_or(1);
        let pairs = actions.chunks(2).filter_map(|pair| Some((pair.first()?.clone(), pair.get(1)?.clone()))).collect();
        let _ = self.events.send(Event::Notify(Incoming {
            id,
            app: app_name,
            summary,
            body: strip_markup(&body),
            actions: pairs,
            urgency,
            timeout: expire_timeout,
        }));
        id
    }

    fn close_notification(&self, id: u32) {
        let _ = self.events.send(Event::Close(id));
    }

    fn get_server_information(&self) -> (&'static str, &'static str, &'static str, &'static str) {
        ("smalta", "smalta", env!("CARGO_PKG_VERSION"), "1.2")
    }
}

pub fn spawn(events: Sender<Event>) {
    thread::spawn(move || {
        let mut warned = false;
        loop {
            let server = Server { events: events.clone(), next: AtomicU32::new(1) };
            let built = zbus::blocking::connection::Builder::session()
                .and_then(|b| b.serve_at(PATH, server))
                .and_then(|b| b.name(INTERFACE))
                .and_then(|b| b.build());
            match built {
                Ok(connection) => {
                    log::info!("serving notifications");
                    if events.send(Event::Ready(connection.clone())).is_err() {
                        return;
                    }
                    // The connection serves on its own executor; this thread
                    // only has to keep it alive.
                    loop {
                        thread::sleep(Duration::from_secs(3600));
                    }
                }
                Err(error) => {
                    if !warned {
                        log::warn!("notifications: {error}; another daemon has the name, retrying");
                        warned = true;
                    }
                    thread::sleep(Duration::from_secs(5));
                }
            }
        }
    });
}

/// Notification bodies may carry a little HTML. Tags go; entities become characters.
pub fn strip_markup(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_tag = false;
    for c in text.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&#39;", "'")
        .replace("&amp;", "&")
}

/// Tell the sender what became of its notification: 1 expired, 2 dismissed, 3 closed by call.
pub fn closed(connection: &Connection, id: u32, reason: u32) {
    let _ = connection.emit_signal(None::<&str>, PATH, INTERFACE, "NotificationClosed", &(id, reason));
}

pub fn invoked(connection: &Connection, id: u32, key: &str) {
    let _ = connection.emit_signal(None::<&str>, PATH, INTERFACE, "ActionInvoked", &(id, key));
}
