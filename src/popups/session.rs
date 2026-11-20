//! The session menu over the whole screen: lock, log out, suspend, restart,
//! power off, as big tiles under a big clock.
//!
//! `smalta toggle session` opens it from a keybinding. Arrows, Tab or the
//! digits choose, Enter does it, Esc or a click past the tiles goes back.
//! It opens on Lock, so a stray Enter costs nothing.

use crate::canvas::{Color, Rect};
use crate::i18n::tr;
use crate::icons::{self, Icon};
use crate::ui::{Align, Key, Ui};

use super::{Ctx, Outcome, Request};

const TILE: i32 = 120;
const GAP: i32 = 12;
/// Each icon pixel drawn this many shell pixels across.
const ICON_MAG: i32 = 4;
const CLOCK_MAG: i32 = 4;

#[derive(Default)]
pub struct Session {
    selected: usize,
    /// Where the pointer was at the last draw: only a pointer that moves
    /// takes the choice away from the keys.
    pointer: Option<(f64, f64)>,
}

fn actions() -> [(Icon, &'static str, Request); 5] {
    [
        (icons::LOCK, tr("Lock"), Request::Lock),
        (icons::LOGOUT, tr("Log out"), Request::LogOut),
        (icons::MOON, tr("Suspend"), Request::Suspend),
        (icons::RESTART, tr("Restart"), Request::Reboot),
        (icons::POWER, tr("Power off"), Request::PowerOff),
    ]
}

/// `user@host · up 3 h 05 min`.
fn who() -> String {
    let user = std::env::var("USER").unwrap_or_default();
    let host = std::fs::read_to_string("/proc/sys/kernel/hostname").unwrap_or_default();
    let mut line = format!("{user}@{}", host.trim());
    let uptime = std::fs::read_to_string("/proc/uptime").ok();
    if let Some(seconds) = uptime.as_deref().and_then(|u| u.split_whitespace().next()?.parse::<f64>().ok()) {
        let minutes = seconds as u64 / 60;
        let up = match minutes / 60 {
            0 => format!("{} {}", minutes, tr("min")),
            h => format!("{h} {} {:02} {}", tr("h"), minutes % 60, tr("min")),
        };
        line = format!("{line} · {} {up}", tr("up"));
    }
    line
}

impl Session {
    pub fn opened(&mut self) {
        self.selected = 0;
        self.pointer = None;
    }

    fn act(&self, cx: &mut Ctx) {
        let (_, _, request) = actions().into_iter().nth(self.selected).expect("five actions");
        cx.requests.push(Request::Close);
        cx.requests.push(request);
    }

    /// The whole screen is `area`.
    pub fn draw(&mut self, ui: &mut Ui, area: Rect, cx: &mut Ctx) -> i32 {
        let t = ui.theme;
        // The screen behind, dimmed and screened by a checkerboard of the
        // panel colour: darker, and still there.
        ui.fill(area, Color(0xb000_0000));
        ui.canvas.dither(area, t.bg);

        let actions = actions();
        let count = actions.len() as i32;
        let row_w = count * TILE + (count - 1) * GAP;
        let clock = chrono::Local::now().format("%H:%M").to_string();
        let clock_w = ui.text_width(&clock) * CLOCK_MAG;
        let clock_h = ui.line() * CLOCK_MAG;
        let block_h = clock_h + 8 + ui.line() + 28 + TILE + 20 + ui.line();
        let mut y = area.y + (area.h - block_h) / 2;
        let x = area.x + (area.w - row_w) / 2;

        let backing = |w: i32, h: i32, y: i32| Rect::new(area.x + (area.w - w) / 2 - 8, y - 4, w + 16, h + 8);
        ui.fill(backing(clock_w, clock_h, y), t.bg);
        ui.canvas.text_big(ui.face, area.x + (area.w - clock_w) / 2, y, &clock, t.fg, CLOCK_MAG);
        y += clock_h + 8;
        let who = who();
        let who_w = ui.text_width(&who);
        ui.fill(backing(who_w, ui.line(), y), t.bg);
        ui.text(Rect::new(area.x, y, area.w, ui.line()), &who, t.dim, Align::Center);
        y += ui.line() + 28;

        let moved = ui.input.pointer != self.pointer;
        self.pointer = ui.input.pointer;
        let mut chosen = None;
        for (i, (icon, label, _)) in actions.iter().enumerate() {
            let tile = Rect::new(x + i as i32 * (TILE + GAP), y, TILE, TILE);
            if moved && ui.hovered(tile) {
                self.selected = i;
            }
            let on = i == self.selected;
            // A hard shadow, like every panel's.
            ui.fill(Rect::new(tile.x + 3, tile.y + 3, tile.w, tile.h), Color(0x9900_0000));
            ui.fill(tile, if on { t.accent } else { t.bg });
            ui.frame(tile, if on { t.accent } else { t.border });
            let ink = if on { t.on_accent } else { t.fg };
            let w = icon.iter().map(|r| r.chars().count() as i32).max().unwrap_or(0) * ICON_MAG;
            let h = icon.len() as i32 * ICON_MAG;
            ui.canvas.sprite_scaled(tile.x + (TILE - w) / 2, tile.y + (TILE - h) / 2 - 8, icon, ink, ICON_MAG);
            let under = Rect::new(tile.x + 4, tile.bottom() - ui.line() - 8, TILE - 8, ui.line());
            ui.text(under, &label.to_lowercase(), ink, Align::Center);
            let digit = Rect::new(tile.x + 5, tile.y + 3, 10, ui.line());
            ui.text(digit, &(i + 1).to_string(), if on { t.on_accent } else { t.faint }, Align::Left);
            if ui.clicked(tile) {
                chosen = Some(i);
            }
        }
        y += TILE + 20;
        let hint = format!("← → · Enter · Esc {}", tr("back"));
        let hint_w = ui.text_width(&hint);
        ui.fill(backing(hint_w, ui.line(), y), t.bg);
        ui.text(Rect::new(area.x, y, area.w, ui.line()), &hint, t.dim, Align::Center);

        if let Some(i) = chosen {
            self.selected = i;
            self.act(cx);
        } else if ui.input.click.take().is_some() {
            // Past the tiles: nothing chosen, back to where it was.
            cx.requests.push(Request::Close);
        }
        area.h
    }

    pub fn key(&mut self, key: &Key, cx: &mut Ctx) -> Outcome {
        let count = actions().len();
        match key {
            Key::Escape => return Outcome::Close,
            Key::Enter => {
                self.act(cx);
                return Outcome::Ignored;
            }
            Key::Left | Key::Up | Key::BackTab => self.selected = (self.selected + count - 1) % count,
            Key::Right | Key::Down | Key::Tab => self.selected = (self.selected + 1) % count,
            Key::Home => self.selected = 0,
            Key::End => self.selected = count - 1,
            Key::Char(c) => match c.to_digit(10).map(|d| d as usize) {
                Some(d @ 1..=5) => {
                    self.selected = d - 1;
                    self.act(cx);
                    return Outcome::Ignored;
                }
                _ => return Outcome::Ignored,
            },
            _ => return Outcome::Ignored,
        }
        Outcome::Redraw
    }
}
