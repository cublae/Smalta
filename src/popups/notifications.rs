//! The notification center: everything not yet dismissed, newest first.
//! It stands on its own, or as the right-hand column of the calendar.

use crate::canvas::Rect;
use crate::i18n::tr;
use crate::icons;
use crate::notifications::{self, CardAction};
use crate::ui::{Align, Look, Ui};

use super::{Ctx, Request};

pub fn draw(ui: &mut Ui, area: Rect, cx: &mut Ctx) -> i32 {
    let t = ui.theme;
    let line = ui.line();
    // One row: "do not disturb" as the same check box the control center
    // has, and the clear button at the other end when there is anything
    // to clear.
    let (top, mut rest) = area.take_top(line + 6, 8);
    let dnd = tr("Do not disturb").to_lowercase();
    let (toggle, top) = top.take_left(ui.text_width(&dnd) + 34, 8);
    if ui.toggle(toggle, cx.dnd, &dnd, "") {
        cx.requests.push(Request::ToggleDnd);
    }
    if !cx.notifications.list.is_empty() {
        let label = tr("Clear all").to_lowercase();
        let (clear, _) = top.take_right(ui.text_width(&label) + 16, 0);
        if ui.button(clear, &label, Look::Framed) {
            cx.requests.push(Request::ClearNotifications);
        }
    }

    if cx.notifications.list.is_empty() {
        let (empty, after) = rest.take_top(line * 3, 0);
        ui.text(empty, tr("No notifications"), t.dim, Align::Center);
        return after.y - area.y;
    }
    let mut actions = Vec::new();
    for n in cx.notifications.list.iter().take(8) {
        let (height, action) = notifications::card(ui, rest, n);
        if let Some(action) = action {
            actions.push((n.data.id, action));
        }
        rest = Rect::new(rest.x, rest.y + height + 6, rest.w, rest.h);
    }
    for (id, action) in actions {
        cx.requests.push(match action {
            CardAction::Dismiss => Request::Dismiss(id),
            CardAction::Invoke(key) => Request::Invoke(id, key),
        });
    }
    rest.y - area.y
}

/// The column beside the calendar, `area.h` tall: a heading with the
/// do-not-disturb and clear buttons, a rule, then the cards, cut at the
/// bottom when there are more than fit.
pub fn column(ui: &mut Ui, area: Rect, cx: &mut Ctx) {
    let t = ui.theme;
    let line = ui.line();
    let (head, rest) = area.take_top(line + 6, 6);
    let (bell, head) = head.take_left(10, 6);
    ui.sprite(bell, if cx.dnd { icons::BELL_OFF } else { icons::BELL }, t.fg);
    let clear_label = tr("Clear all").to_lowercase();
    let (clear, head) = head.take_right(ui.text_width(&clear_label) + 12, 4);
    let dnd_label = tr("Do not disturb").to_lowercase();
    let (dnd, head) = head.take_right(ui.text_width(&dnd_label) + 12, 8);
    ui.text(head, &tr("Notifications").to_lowercase(), t.fg, Align::Left);
    if ui.button(dnd, &dnd_label, if cx.dnd { Look::On } else { Look::Framed }) {
        cx.requests.push(Request::ToggleDnd);
    }
    let empty = cx.notifications.list.is_empty();
    if empty {
        // Nothing to clear: the label stays, greyed, so the heading keeps its shape.
        ui.text(clear, &clear_label, t.border, Align::Center);
    } else if ui.button(clear, &clear_label, Look::Flat) {
        cx.requests.push(Request::ClearNotifications);
    }
    ui.fill(Rect::new(rest.x, rest.y, rest.w, 1), t.border);
    let mut rest = Rect::new(rest.x, rest.y + 8, rest.w, rest.h - 8);

    if empty {
        let (icon, after) = rest.take_top(line + 8, 4);
        // The bell at twice its size, each of its pixels a 2x2 block.
        let w = icons::BELL[0].len() as i32 * 2;
        let (x0, y0) = (icon.x + (icon.w - w) / 2, icon.y + 16);
        for (row, cells) in icons::BELL.iter().enumerate() {
            for (column, cell) in cells.chars().enumerate() {
                if cell == '#' {
                    ui.fill(Rect::new(x0 + column as i32 * 2, y0 + row as i32 * 2, 2, 2), t.dim);
                }
            }
        }
        let message = Rect::new(after.x, after.y + 24, after.w, line);
        ui.text(message, tr("All quiet, nothing new"), t.dim, Align::Center);
        return;
    }
    // Cards past the bottom are cut there, not drawn over the frame.
    let bottom = area.bottom();
    ui.canvas.clip = Some(Rect::new(area.x, rest.y, area.w, bottom - rest.y));
    let mut actions = Vec::new();
    for n in &cx.notifications.list {
        if rest.y >= bottom {
            break;
        }
        let (height, action) = notifications::card(ui, rest, n);
        if let Some(action) = action {
            actions.push((n.data.id, action));
        }
        rest = Rect::new(rest.x, rest.y + height + 6, rest.w, rest.h);
    }
    ui.canvas.clip = None;
    for (id, action) in actions {
        cx.requests.push(match action {
            CardAction::Dismiss => Request::Dismiss(id),
            CardAction::Invoke(key) => Request::Invoke(id, key),
        });
    }
}
