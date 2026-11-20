//! Widgets on the desktop: a big clock, the date and how much of the day is
//! gone, a calendar, what is playing, and graphs of the machine's load.
//!
//! They sit on the background layer and take no input; the space between them
//! stays transparent, so the wallpaper shows through.

use chrono::{Datelike, Local, Timelike};

use crate::canvas::{Canvas, Color, Rect};
use crate::config::Config;
use crate::font::Face;
use crate::i18n::{self, tr};
use crate::icons;
use crate::popups::calendar::Calendar;
use crate::resources::{HISTORY, Resources};
use crate::services::Services;
use crate::theme::Theme;
use crate::ui::{Align, Input, Ui};

pub struct Desktop<'a> {
    pub config: &'a Config,
    pub theme: &'a Theme,
    pub face: &'a Face,
    pub resources: &'a Resources,
    pub services: &'a Services,
}

const MARGIN: i32 = 40;
const SIDE: i32 = 280;

/// A panel in the desktop's own style: dark, framed, with a hard shadow.
fn card(canvas: &mut Canvas, theme: &Theme, rect: Rect) {
    canvas.fill(Rect::new(rect.x + 3, rect.y + 3, rect.w, rect.h), Color(0x9900_0000));
    canvas.fill(rect, theme.bg);
    canvas.frame(rect, theme.border);
}

pub fn draw(canvas: &mut Canvas, width: i32, height: i32, d: &Desktop) {
    canvas.clear(Color(0));
    let t = d.theme;
    let line = d.face.line_height();
    let now = Local::now();
    let cfg = &d.config.desktop;

    // The clock: the font itself at six times the size, with a shadow.
    let mag = 6;
    let clock = now.format(&cfg.clock_format).to_string();
    canvas.text_big(d.face, MARGIN + mag, MARGIN + mag, &clock, Color(0xcc00_0000), mag);
    let clock_w = canvas.text_big(d.face, MARGIN, MARGIN, &clock, t.fg, mag);
    let mut y = MARGIN + line * mag + 12;
    // Media sits at the bottom left; the calendar only draws if it ends above it.
    let media_top = height - MARGIN - line * 2 - 28;

    let mut input = Input::default();
    let mut ui = Ui { canvas, face: d.face, theme: t, input: &mut input, hot: Vec::new() };
    if cfg.show_date {
        let date = format!(
            "{}, {} {}",
            i18n::weekday_long(now.weekday().num_days_from_monday()),
            now.day(),
            i18n::month_of(now.month0())
        );
        let w = clock_w.max(SIDE);
        ui.text(Rect::new(MARGIN, y, w, line), &date.to_lowercase(), t.dim, Align::Left);
        y += line + 6;
        // How much of the day has gone, in twelve cells.
        let day = (now.num_seconds_from_midnight() as f32) / 86_400.0;
        ui.cells(Rect::new(MARGIN, y, w, 4), day, 14, 3);
        y += 4 + 24;
    }

    if cfg.show_calendar {
        let mut calendar = Calendar::default();
        let area = Rect::new(MARGIN + 12, y + 12, crate::popups::calendar::WIDTH, 1000);
        // Measured once, drawn once more on top of its card.
        let used = calendar.draw(&mut ui, Rect::new(-10_000, -10_000, area.w, 1000));
        let frame = Rect::new(MARGIN, y, area.w + 24, used + 24);
        // A screen too short for it goes without, rather than under the media card.
        if frame.bottom() + 16 < media_top || !cfg.show_media {
            card(ui.canvas, t, frame);
            calendar.draw(&mut ui, area);
            y = frame.bottom() + 16;
        }
    }

    // Graphs down the right side.
    if cfg.show_graphs {
        let r = d.resources;
        let mut gy = MARGIN;
        let x = width - MARGIN - SIDE;
        let graphs = [
            ("cpu", format!("{}%", (r.cpu * 100.0).round()), &r.cpu_history, icons::SPEED),
            ("memory", format!("{:.1}/{:.0} GB", r.memory_gib.0, r.memory_gib.1), &r.memory_history, icons::GRID),
            (
                "temperature",
                r.temperature.map(|t| format!("{}°C", t.round())).unwrap_or_default(),
                &r.temperature_history,
                icons::SUN,
            ),
        ];
        for (name, value, history, icon) in graphs {
            if history.is_empty() {
                continue;
            }
            let rect = Rect::new(x, gy, SIDE, line + 70);
            card(ui.canvas, t, rect);
            let inner = rect.inset(10);
            let (head, plot) = inner.take_top(line, 8);
            let (glyph, head) = head.take_left(10, 6);
            ui.sprite(glyph, icon, t.dim);
            ui.text(head, name, t.dim, Align::Left);
            ui.text(head, &value, t.fg, Align::Right);
            graph(ui.canvas, t, plot, history);
            gy = rect.bottom() + 16;
        }
    }

    // What is playing, bottom left.
    if cfg.show_media && media_top > y {
        let rect = Rect::new(MARGIN, media_top, 380, line * 2 + 28);
        card(ui.canvas, t, rect);
        let inner = rect.inset(10);
        let (head, rest) = inner.take_top(line, 4);
        let (glyph, head) = head.take_left(10, 6);
        ui.sprite(glyph, icons::MUSIC, t.dim);
        match d.services.media.current() {
            Some(player) if !player.title.is_empty() => {
                ui.text(head, &player.title, t.fg, Align::Left);
                let (artist, bar) = rest.take_top(line, 4);
                ui.text(artist, &player.artist, t.dim, Align::Left);
                if player.length > 0.0 {
                    ui.cells(Rect::new(bar.x, bar.y, bar.w, 3), (player.position / player.length) as f32, 2, 1);
                }
            }
            _ => ui.text(head, &tr("Nothing playing").to_lowercase(), t.dim, Align::Left),
        }
    }
}

/// A line along the values, the area under it half-toned.
fn graph(canvas: &mut Canvas, t: &Theme, plot: Rect, history: &std::collections::VecDeque<f32>) {
    let step = (plot.w / HISTORY as i32).max(1);
    let start = plot.x + plot.w - step * history.len() as i32;
    for (i, value) in history.iter().enumerate() {
        let h = ((value.clamp(0.0, 1.0) * plot.h as f32).round() as i32).max(1);
        let x = start + i as i32 * step;
        let top = plot.y + plot.h - h;
        canvas.dither(Rect::new(x, top + 1, step, h - 1), t.faint);
        canvas.fill(Rect::new(x, top, step, 1), t.fg);
    }
    canvas.fill(Rect::new(plot.x, plot.y + plot.h, plot.w, 1), t.border);
}
