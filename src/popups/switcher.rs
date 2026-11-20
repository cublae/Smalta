//! The window switcher: every open window as a card, the most recently used
//! first, with what it shows in big dithered pixels.
//!
//! A keybinding runs `smalta switch`: the first press freezes the screens,
//! cuts each window on them out into a preview and opens on the window used
//! before this one; each further press moves on. Let go of the modifier held
//! with it -- Alt, Super or Ctrl -- and the chosen window comes forward, the
//! way Alt+Tab does. Opened without one held, Enter or a click chooses.
//!
//! Only windows on a workspace that is on screen can be cut out of a frozen
//! screen; the rest show their icon.

use std::collections::HashMap;

use crate::appicons;
use crate::canvas::Rect;
use crate::compositor::Window;
use crate::i18n::tr;
use crate::picker::Capture;
use crate::services::tray::Pixmap;
use crate::ui::{Align, Key, Ui};

use super::{Ctx, Outcome, Request};

const COLUMNS: i32 = 4;
const GAP: i32 = 10;
const CARD_W: i32 = 184;
const PREVIEW_H: i32 = 104;
/// Shell pixels to one pixel of a preview.
const CELL: i32 = 2;
/// Shades per channel in a preview.
const LEVELS: u32 = 6;
const ROWS: i32 = 3;
const ICON: i32 = 16;
const BIG_ICON: i32 = 32;

pub const WIDTH: i32 = COLUMNS * CARD_W + (COLUMNS - 1) * GAP;

#[derive(Default)]
pub struct Switcher {
    windows: Vec<Window>,
    selected: usize,
    /// The first row on screen.
    scroll: usize,
    /// Previews by window id, made when the screens were frozen.
    previews: HashMap<String, Pixmap>,
    icons: HashMap<(String, i32), Option<Pixmap>>,
    /// Opened with a modifier held, whose release chooses.
    pub armed: bool,
    /// The modifier has been seen held since opening.
    held: bool,
    /// The modifiers have been heard of since opening: the compositor tells
    /// them as the keyboard comes to the switcher.
    told: bool,
}

impl Switcher {
    /// A fresh start on `windows`, one step from the current window: forward
    /// is the one used before it, back the one used longest ago.
    pub fn prepare(&mut self, windows: Vec<Window>, step: i32, armed: bool) {
        self.windows = windows;
        self.previews.clear();
        self.scroll = 0;
        self.armed = armed;
        self.held = false;
        self.told = false;
        let count = self.windows.len();
        self.selected = match (count, step < 0) {
            (0 | 1, _) => 0,
            (_, false) => 1,
            (_, true) => count - 1,
        };
        self.reveal();
    }

    pub fn step(&mut self, step: i32) {
        let count = self.windows.len() as i32;
        if count > 0 {
            self.selected = (self.selected as i32 + step).rem_euclid(count) as usize;
            self.reveal();
        }
    }

    fn reveal(&mut self) {
        let row = self.selected / COLUMNS as usize;
        if row < self.scroll {
            self.scroll = row;
        } else if row >= self.scroll + ROWS as usize {
            self.scroll = row + 1 - ROWS as usize;
        }
    }

    /// Outputs with a window on them worth freezing for a preview.
    pub fn outputs(&self) -> Vec<String> {
        let mut outputs: Vec<String> =
            self.windows.iter().filter(|w| w.rect.is_some()).map(|w| w.output.clone()).collect();
        outputs.sort();
        outputs.dedup();
        outputs
    }

    /// Previews of the windows on `capture`'s output, whose logical size is
    /// `logical`; the capture itself is let go after.
    pub fn take(&mut self, capture: Capture, logical: (i32, i32)) {
        let (sx, sy) =
            (capture.width as f32 / logical.0.max(1) as f32, capture.height as f32 / logical.1.max(1) as f32);
        let screen = Pixmap { width: capture.width, height: capture.height, pixels: capture.pixels };
        for window in self.windows.iter().filter(|w| w.output == capture.output) {
            let Some((x, y, w, h)) = window.rect else { continue };
            // Only what is on the screen: a window scrolled half off it shows its half.
            let (x0, y0) = ((x as f32 * sx).max(0.0), (y as f32 * sy).max(0.0));
            let x1 = ((x + w) as f32 * sx).min(screen.width as f32);
            let y1 = ((y + h) as f32 * sy).min(screen.height as f32);
            if x1 - x0 < 4.0 || y1 - y0 < 4.0 {
                continue;
            }
            let (pw, ph) = fit((x1 - x0) / sx, (y1 - y0) / sy, CARD_W - 2, PREVIEW_H - 2);
            let mut preview =
                crate::image::resample(&screen, (x0, y0, x1 - x0, y1 - y0), (pw / CELL).max(1), (ph / CELL).max(1));
            crate::wallpaper::posterize(&mut preview, LEVELS);
            self.previews.insert(window.id.clone(), preview);
        }
    }

    fn icon(&mut self, app: &str, size: i32) -> Option<&Pixmap> {
        self.icons
            .entry((app.to_owned(), size))
            .or_insert_with(|| appicons::load(app, size).or_else(|| appicons::load(&app.to_lowercase(), size)))
            .as_ref()
    }

    fn choose(&self, cx: &mut Ctx) {
        if let Some(window) = self.windows.get(self.selected) {
            cx.requests.push(Request::FocusWindow(window.id.clone()));
        }
        cx.requests.push(Request::Close);
    }

    /// The modifiers now held: let go of the one the switcher opened with, and
    /// the chosen window comes forward.
    pub fn modifiers(&mut self, held: bool, cx: &mut Ctx) {
        if !self.armed {
            return;
        }
        if held {
            self.held = true;
        } else if self.held || !self.told {
            // Let go already, or let go before the switcher was even up: a
            // quick Alt+Tab, straight to the window before.
            self.armed = false;
            self.choose(cx);
        }
        self.told = true;
    }

    pub fn key(&mut self, key: &Key, cx: &mut Ctx) -> Outcome {
        let columns = COLUMNS as usize;
        let count = self.windows.len();
        match key {
            Key::Escape => return Outcome::Close,
            Key::Enter | Key::Char(' ') => self.choose(cx),
            Key::Tab | Key::Right | Key::Char('l') => self.step(1),
            Key::BackTab | Key::Left | Key::Char('h') => self.step(-1),
            Key::Down | Key::Char('j') if self.selected + columns < count => self.step(COLUMNS),
            Key::Up | Key::Char('k') if self.selected >= columns => self.step(-COLUMNS),
            _ => return Outcome::Ignored,
        }
        Outcome::Redraw
    }

    pub fn draw(&mut self, ui: &mut Ui, area: Rect, cx: &mut Ctx) -> i32 {
        let t = ui.theme;
        let line = ui.line();
        if self.windows.is_empty() {
            ui.text(Rect::new(area.x, area.y, area.w, line * 2), tr("No windows"), t.dim, Align::Center);
            return line * 2;
        }
        let card_h = PREVIEW_H + 6 + line;
        let rows = (self.windows.len() as i32 + COLUMNS - 1) / COLUMNS;
        let steps = ui.scrolled(Rect::new(area.x, area.y, area.w, card_h * ROWS));
        if steps != 0 {
            let max = (rows - ROWS).max(0);
            self.scroll = (self.scroll as i32 + steps).clamp(0, max) as usize;
        }
        let first = self.scroll * COLUMNS as usize;
        let last = (first + (COLUMNS * ROWS) as usize).min(self.windows.len());
        let mut chosen = None;
        let mut bottom = area.y;
        for index in first..last {
            let slot = (index - first) as i32;
            let (column, row) = (slot % COLUMNS, slot / COLUMNS);
            let card = Rect::new(area.x + column * (CARD_W + GAP), area.y + row * (card_h + GAP), CARD_W, card_h);
            bottom = card.bottom();
            let current = index == self.selected;
            let window = self.windows[index].clone();

            let preview = Rect::new(card.x, card.y, CARD_W, PREVIEW_H);
            ui.fill(preview, t.bg);
            match self.previews.get(&window.id) {
                Some(image) => {
                    // Centred, its own shape, in whole cells.
                    let (w, h) = (image.width * CELL, image.height * CELL);
                    let at = Rect::new(preview.x + (preview.w - w) / 2, preview.y + (preview.h - h) / 2, w, h);
                    ui.canvas.image_in(at, image);
                }
                None => {
                    // Off screen: a half-tone ground and the application's icon, large.
                    ui.canvas.dither(preview.inset(1), t.faint);
                    let at = (preview.x + (preview.w - BIG_ICON) / 2, preview.y + (preview.h - BIG_ICON) / 2);
                    match self.icon(&window.app, BIG_ICON) {
                        Some(icon) => ui.canvas.image(at.0, at.1, BIG_ICON, &icon.clone()),
                        None => ui.text(preview, &window.app, t.dim, Align::Center),
                    }
                }
            }
            let hovered = ui.hovered(card);
            ui.frame(
                preview,
                if current {
                    t.accent
                } else if hovered {
                    t.fg
                } else {
                    t.border
                },
            );
            if current {
                ui.frame(preview.inset(1), t.accent);
            }

            let label = Rect::new(card.x, preview.bottom() + 6, CARD_W, line);
            let (badge, rest) = label.take_left(ICON, 6);
            if let Some(icon) = self.icon(&window.app, ICON).cloned() {
                ui.canvas.image(badge.x, badge.y + (badge.h - ICON) / 2, ICON, &icon);
            }
            let (place, title) = rest.take_right(ui.text_width(&window.workspace) + 2, 6);
            ui.text(place, &window.workspace, t.dim, Align::Right);
            let name = if window.title.is_empty() { &window.app } else { &window.title };
            ui.text(title, name, if current { t.accent } else { t.fg }, Align::Left);
            if ui.clicked(card) {
                chosen = Some(index);
            }
        }
        if let Some(index) = chosen {
            self.selected = index;
            self.choose(cx);
        }
        bottom - area.y
    }
}

/// `w` x `h` scaled to fit inside `max_w` x `max_h`, its shape kept.
fn fit(w: f32, h: f32, max_w: i32, max_h: i32) -> (i32, i32) {
    let scale = (max_w as f32 / w).min(max_h as f32 / h);
    (((w * scale) as i32).max(1), ((h * scale) as i32).max(1))
}
