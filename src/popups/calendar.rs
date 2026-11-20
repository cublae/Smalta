//! A month at a time; scroll or the arrows to move, the title to come back.

use chrono::{Datelike, Local, NaiveDate};

use crate::canvas::Rect;
use crate::i18n;
use crate::icons;
use crate::ui::{Align, Look, Ui};

#[derive(Default)]
pub struct Calendar {
    /// Months away from the current one.
    pub offset: i32,
}

pub const WIDTH: i32 = 7 * 30 + 32;

impl Calendar {
    pub fn draw(&mut self, ui: &mut Ui, area: Rect) -> i32 {
        let t = ui.theme;
        let today = Local::now().date_naive();
        let line = ui.line();

        // The time large, in the same font twice the size, and the date under it.
        let (time, rest) = area.take_top(line * 2, 4);
        let now = Local::now().format("%H:%M").to_string();
        ui.canvas.text_big(ui.face, time.x, time.y, &now, t.fg, 2);
        let (date, rest) = rest.take_top(line, 10);
        let headline = format!(
            "{}, {} {}",
            i18n::weekday_long(today.weekday().num_days_from_monday()),
            today.day(),
            i18n::month_of(today.month0())
        );
        ui.text(date, &headline, t.dim, Align::Left);

        let (nav, rest) = rest.take_top(line + 6, 6);
        let shown = shift(today, self.offset);
        let (prev, nav) = nav.take_left(line + 6, 4);
        let (next, nav) = nav.take_right(line + 6, 4);
        if ui.icon_button(prev, icons::CHEVRON_LEFT, Look::Flat) {
            self.offset -= 1;
        }
        if ui.icon_button(next, icons::CHEVRON, Look::Flat) {
            self.offset += 1;
        }
        let label = format!("{} {}", i18n::month(shown.month0()), shown.year());
        if ui.button(nav, &label, Look::Flat) {
            self.offset = 0;
        }

        let cell = 30;
        let (heads, rest) = rest.take_top(line + 4, 2);
        for (i, column) in heads.columns(7, 0).into_iter().enumerate() {
            ui.text(column, i18n::weekday_short(i as u32), if i >= 5 { t.dim } else { t.fg }, Align::Center);
        }

        let grid = Rect::new(rest.x, rest.y, rest.w, cell * 6);
        let first = shown.with_day(1).unwrap_or(shown);
        let lead = first.weekday().num_days_from_monday() as i64;
        for index in 0..42i64 {
            let date = first - chrono::Duration::days(lead) + chrono::Duration::days(index);
            let column = (index % 7) as i32;
            let row = (index / 7) as i32;
            let x = grid.x + column * (grid.w / 7);
            let rect = Rect::new(x + (grid.w / 7 - cell) / 2, grid.y + row * cell, cell, cell - 2);
            let inside = date.month() == shown.month();
            let ink = if date == today {
                ui.fill(rect, t.accent);
                t.on_accent
            } else if !inside {
                t.border
            } else if column >= 5 {
                t.dim
            } else {
                t.fg
            };
            ui.text(rect, &date.day().to_string(), ink, Align::Center);
        }
        let steps = ui.scrolled(grid);
        self.offset += steps;
        grid.bottom() - area.y
    }
}

fn shift(date: NaiveDate, months: i32) -> NaiveDate {
    let total = date.year() * 12 + date.month0() as i32 + months;
    NaiveDate::from_ymd_opt(total.div_euclid(12), total.rem_euclid(12) as u32 + 1, 1).unwrap_or(date)
}
