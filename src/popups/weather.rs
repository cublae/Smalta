//! The weather: now, the next hours, the next days.

use crate::canvas::Rect;
use crate::i18n::{self, tr};
use crate::services::weather;
use crate::ui::{Align, Ui};

use super::Ctx;

pub fn draw(ui: &mut Ui, area: Rect, cx: &mut Ctx) -> i32 {
    let t = ui.theme;
    let line = ui.line();
    let w = &cx.services.weather;
    let Some(now) = &w.current else {
        let (row, rest) = area.take_top(line * 3, 0);
        let text = if !w.configured {
            tr("Not configured").to_owned()
        } else {
            w.error.clone().unwrap_or_else(|| "…".into())
        };
        ui.text(row, &text, t.dim, Align::Center);
        return rest.y - area.y;
    };
    let (head, rest) = area.take_top(line * 2 + 4, 10);
    let (icon, head) = head.take_left(line * 2, 10);
    ui.frame(icon, t.border);
    ui.sprite(icon, weather::icon(now.code), t.fg);
    let (top, bottom) = head.take_top(line, 4);
    ui.text(top, &format!("{}°  {}", now.temperature.round(), weather::describe(now.code)), t.fg, Align::Left);
    ui.text(bottom, &w.place, t.dim, Align::Left);

    let (details, rest) = rest.take_top(line, 10);
    let text = format!(
        "{} {}°   {} {}%   {} {}",
        tr("Feels like").to_lowercase(),
        now.feels.round(),
        tr("Humidity").to_lowercase(),
        now.humidity.round(),
        tr("Wind").to_lowercase(),
        now.wind.round()
    );
    ui.text(details, &text, t.dim, Align::Left);

    // Hours as a row of columns: time, icon, temperature.
    let hours: Vec<_> = w.hours.iter().step_by(2).take(6).collect();
    let (strip, rest) = rest.take_top(line * 2 + 12, 10);
    for (cell, (hour, temp, code)) in strip.columns(hours.len().max(1) as i32, 2).into_iter().zip(hours) {
        ui.frame(cell, t.faint);
        let (a, b) = cell.take_top(line, 0);
        ui.text(a, &format!("{hour:02}"), t.dim, Align::Center);
        let (b, c) = b.take_top(10, 0);
        ui.sprite(b, weather::icon(*code), t.fg);
        ui.text(c, &format!("{}°", temp.round()), t.fg, Align::Center);
    }

    let mut y = rest;
    for day in &w.days {
        let (row, next) = y.take_top(line + 4, 0);
        y = next;
        let (name, row) = row.take_left(110, 6);
        ui.text(name, &i18n::weekday_long(day.weekday).to_lowercase(), t.fg, Align::Left);
        let (icon, row) = row.take_left(12, 8);
        ui.sprite(icon, weather::icon(day.code), t.fg);
        ui.text(row, &format!("{}° / {}°", day.low.round(), day.high.round()), t.dim, Align::Right);
    }
    y.y - area.y
}
