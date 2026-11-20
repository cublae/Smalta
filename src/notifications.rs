//! What the notification server received, and how a notification is drawn.

use std::time::{Duration, Instant};

use zbus::blocking::Connection;

use crate::canvas::Rect;
use crate::config::NotificationSettings;
use crate::i18n::tr;
use crate::icons;
use crate::services::notifications::{self as server, Incoming};
use crate::ui::{Align, Look, Ui};

pub struct Notification {
    pub data: Incoming,
    pub arrived: Instant,
    /// Still shown as a toast.
    pub fresh: bool,
    /// How long it has been up as a toast, not counting time under the pointer.
    pub shown: Duration,
}

#[derive(Default)]
pub struct Notifications {
    pub list: Vec<Notification>,
    pub connection: Option<Connection>,
    /// Arrived since the center was last opened.
    pub unread: bool,
}

/// What a click on a card asks for.
pub enum CardAction {
    Dismiss,
    Invoke(String),
}

impl Notifications {
    /// A new notification, as a toast unless do-not-disturb or a shared
    /// screen keeps it quiet; critical ones show through do-not-disturb.
    pub fn add(&mut self, data: Incoming, quiet: bool, settings: &NotificationSettings) {
        let fresh = !quiet || data.urgency >= 2;
        let notification = Notification { arrived: Instant::now(), fresh, shown: Duration::ZERO, data };
        match self.list.iter_mut().find(|n| n.data.id == notification.data.id) {
            Some(existing) => *existing = notification,
            None => self.list.insert(0, notification),
        }
        self.unread = true;
        self.list.truncate(settings.history.max(1) as usize);
    }

    pub fn close(&mut self, id: u32, reason: u32) {
        let before = self.list.len();
        self.list.retain(|n| n.data.id != id);
        if self.list.len() != before
            && let Some(connection) = &self.connection
        {
            server::closed(connection, id, reason);
        }
    }

    pub fn clear(&mut self) {
        let ids: Vec<u32> = self.list.iter().map(|n| n.data.id).collect();
        for id in ids {
            self.close(id, 2);
        }
    }

    pub fn invoke(&mut self, id: u32, key: &str) {
        if let Some(connection) = &self.connection {
            server::invoked(connection, id, key);
        }
        self.close(id, 2);
    }

    /// Toasts that have been up for `step` more; those past their time stop
    /// being toasts and stay in the center. A `step` of zero holds them, for
    /// a pointer resting on them. Returns whether anything changed.
    pub fn expire(&mut self, step: Duration, settings: &NotificationSettings) -> bool {
        let mut changed = false;
        for n in &mut self.list {
            if !n.fresh {
                continue;
            }
            n.shown += step;
            if lifetime(n, settings).is_some_and(|life| n.shown >= life) {
                n.fresh = false;
                changed = true;
            }
        }
        changed
    }

    pub fn toasts(&self, settings: &NotificationSettings) -> impl Iterator<Item = &Notification> {
        self.list.iter().filter(|n| n.fresh).take(settings.max_toasts.max(1) as usize)
    }
}

/// How long `n` stays a toast, or `None` for until it is closed.
fn lifetime(n: &Notification, settings: &NotificationSettings) -> Option<Duration> {
    if n.data.urgency >= 2 && settings.critical_sticky {
        return None;
    }
    if settings.app_timeout {
        match n.data.timeout {
            0 => return None,
            // An app asking for longer than a minute is asking for a toast
            // that never leaves.
            t if t > 0 => return Some(Duration::from_millis(t as u64).min(Duration::from_secs(60))),
            _ => {}
        }
    }
    Some(Duration::from_secs(settings.timeout.max(1) as u64))
}

fn age(arrived: Instant) -> String {
    let secs = arrived.elapsed().as_secs();
    let russian = crate::i18n::russian();
    match secs {
        0..60 => tr("just now").to_owned(),
        60..3600 => format!("{} {}", secs / 60, if russian { "мин" } else { "min" }),
        _ => format!("{} {}", secs / 3600, if russian { "ч" } else { "h" }),
    }
}

/// One card: app and age on the frame, summary, wrapped body, action buttons.
/// Returns its height and what was clicked.
pub fn card(ui: &mut Ui, area: Rect, n: &Notification) -> (i32, Option<CardAction>) {
    let t = ui.theme;
    let line = ui.line();
    let inner_w = area.w - 16;
    let body_lines = ui.wrap(&n.data.body, inner_w).len().min(5) as i32;
    let actions: Vec<&(String, String)> =
        n.data.actions.iter().filter(|(key, label)| key != "default" && !label.is_empty()).collect();
    let height = 8 + line + 4 + body_lines * line + if actions.is_empty() { 0 } else { line + 14 } + 10;
    let rect = Rect::new(area.x, area.y + line / 2, area.w, height);

    ui.fill(rect, t.bg);
    let critical = n.data.urgency >= 2;
    ui.frame(rect, if critical { t.urgent } else { t.border });
    let app = if n.data.app.is_empty() { tr("Notifications").to_owned() } else { n.data.app.to_lowercase() };
    ui.panel(rect, &app);
    if critical {
        ui.frame(rect, t.urgent);
    }
    let mut action = None;

    let inner = Rect::new(rect.x + 8, rect.y + 8, inner_w, height - 16);
    let (top, rest) = inner.take_top(line, 4);
    let (close, top) = top.take_right(line, 4);
    if ui.icon_button(close, icons::CROSS, Look::Flat) {
        action = Some(CardAction::Dismiss);
    }
    let (when, top) = top.take_right(ui.text_width(&age(n.arrived)), 8);
    ui.text(when, &age(n.arrived), t.dim, Align::Right);
    ui.text(top, &n.data.summary, t.fg, Align::Left);

    let mut y = rest.y;
    for text in ui.wrap(&n.data.body, inner_w).into_iter().take(5) {
        ui.text(Rect::new(inner.x, y, inner.w, line), &text, t.dim, Align::Left);
        y += line;
    }
    if !actions.is_empty() {
        let row = Rect::new(inner.x, y + 6, inner.w, line + 6);
        for (cell, (key, label)) in row.columns(actions.len() as i32, 4).into_iter().zip(&actions) {
            if ui.button(cell, &label.to_lowercase(), Look::Framed) {
                action = Some(CardAction::Invoke(key.clone()));
            }
        }
    }
    // The rest of the card: its default action, or nothing.
    if action.is_none() && ui.clicked(rect) {
        let default = n.data.actions.iter().any(|(key, _)| key == "default");
        action = Some(if default { CardAction::Invoke("default".into()) } else { CardAction::Dismiss });
    }
    (height + line / 2, action)
}
