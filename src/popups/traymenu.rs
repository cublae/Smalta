//! A tray item's menu, as one flat list with submenus indented.

use crate::canvas::Rect;
use crate::i18n::tr;
use crate::services::tray;
use crate::ui::{Align, Ui};

use super::{Ctx, Request};

#[derive(Default)]
pub struct TrayMenu {
    pub key: String,
}

pub fn draw(menu: &TrayMenu, ui: &mut Ui, area: Rect, cx: &mut Ctx) -> i32 {
    let t = ui.theme;
    let line = ui.line();
    let entries = match &cx.services.tray_menu {
        Some((key, entries)) if *key == menu.key => entries,
        _ => {
            let (row, rest) = area.take_top(line + 6, 0);
            ui.text(row, "…", t.dim, Align::Left);
            return rest.y - area.y;
        }
    };
    if entries.is_empty() {
        let (row, rest) = area.take_top(line + 6, 0);
        ui.text(row, tr("Empty"), t.dim, Align::Left);
        return rest.y - area.y;
    }
    let mut y = area;
    for entry in entries {
        if entry.separator {
            let (row, rest) = y.take_top(7, 0);
            y = rest;
            ui.fill(Rect::new(row.x, row.y + 3, row.w, 1), t.border);
            continue;
        }
        let (row, rest) = y.take_top(line + 6, 0);
        y = rest;
        let indent = entry.depth as i32 * 12;
        let row = Rect::new(row.x + indent, row.y, row.w - indent, row.h);
        let mark = match entry.checked {
            Some(true) => "[x] ",
            Some(false) => "[ ] ",
            None => "",
        };
        let label = format!("{mark}{}", entry.label);
        if !entry.enabled {
            ui.text(Rect::new(row.x + 8, row.y, row.w - 16, row.h), &label, t.border, Align::Left);
            continue;
        }
        if ui.row(row, None, &label, "", false) {
            cx.requests.push(Request::Tray(tray::Command::Click(menu.key.clone(), entry.id)));
            cx.requests.push(Request::Close);
        }
    }
    y.y - area.y
}
