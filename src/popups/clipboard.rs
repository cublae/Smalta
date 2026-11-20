//! Clipboard history as cards: what kind of thing, how long ago, a look at
//! it, and buttons to pin or forget it. Filter as you type; Enter or a click
//! copies back, Delete forgets.

use std::collections::HashMap;

use crate::canvas::Rect;
use crate::clipboard::{self, Entry};
use crate::i18n::{self, tr};
use crate::icons;
use crate::services::tray::Pixmap;
use crate::ui::{Align, Field, Key, Look, Ui};

use super::{Ctx, Outcome, Request};

/// Image previews, in shell pixels.
const PREVIEW: i32 = 64;
/// Lines of a text entry shown on its card.
const TEXT_LINES: usize = 2;
/// Between cards.
const GAP: i32 = 6;

#[derive(Default)]
pub struct History {
    field: Field,
    entries: Vec<Entry>,
    shown: Vec<usize>,
    selected: usize,
    /// How far the list is scrolled, in pixels.
    offset: i32,
    /// The selection moved by key and the list should bring it into view;
    /// the wheel moves the view and leaves the selection be.
    follow: bool,
    /// Each shown card's height with the gap after it; empty when stale.
    heights: Vec<i32>,
    pinned_only: bool,
    /// When the clear button was pressed once; a second press clears, if it
    /// is not the second half of a double click.
    confirm_wipe: Option<std::time::Instant>,
    /// Decoded previews by [`Entry::key`], kept across openings.
    previews: HashMap<String, Option<Pixmap>>,
}

enum Action {
    Copy(usize),
    Delete(usize),
    Pin(usize),
}

impl History {
    pub fn opened(&mut self) {
        self.field = Field::default();
        self.confirm_wipe = None;
        self.reload();
    }

    fn reload(&mut self) {
        self.entries = clipboard::list();
        let keys: Vec<&str> = self.entries.iter().map(Entry::key).collect();
        self.previews.retain(|key, _| keys.contains(&key.as_str()));
        self.filter();
    }

    fn filter(&mut self) {
        let query = self.field.text.to_lowercase();
        self.shown = (0..self.entries.len())
            .filter(|&i| {
                let entry = &self.entries[i];
                (!self.pinned_only || entry.pin.is_some()) && entry.preview.to_lowercase().contains(&query)
            })
            .collect();
        self.selected = 0;
        self.offset = 0;
        self.heights.clear();
    }

    /// Does `action` and keeps the selection near where it was.
    fn apply(&mut self, action: Action, cx: &mut Ctx) {
        let keep = self.selected;
        match action {
            Action::Copy(index) => {
                clipboard::copy_entry(&self.entries[index]);
                cx.requests.push(Request::Close);
                return;
            }
            Action::Delete(index) => clipboard::delete(&self.entries[index]),
            Action::Pin(index) => {
                let entry = &self.entries[index];
                if entry.pin.is_some() { clipboard::unpin(entry) } else { clipboard::pin(entry) }
            }
        }
        let offset = self.offset;
        self.reload();
        self.selected = keep.min(self.shown.len().saturating_sub(1));
        // Clamped to the shorter list when next drawn.
        self.offset = offset;
    }

    pub fn key(&mut self, key: &Key, cx: &mut Ctx) -> Outcome {
        let count = self.shown.len();
        if *key != Key::Escape {
            self.confirm_wipe = None;
        }
        match key {
            Key::Escape if self.confirm_wipe.is_some() => self.confirm_wipe = None,
            Key::Escape => return Outcome::Close,
            Key::Enter => {
                if let Some(&index) = self.shown.get(self.selected) {
                    self.apply(Action::Copy(index), cx);
                }
            }
            Key::Delete => {
                if let Some(&index) = self.shown.get(self.selected) {
                    self.apply(Action::Delete(index), cx);
                }
            }
            Key::Down => self.selected = (self.selected + 1).min(count.saturating_sub(1)),
            Key::Up => self.selected = self.selected.saturating_sub(1),
            Key::PageDown => self.selected = (self.selected + 3).min(count.saturating_sub(1)),
            Key::PageUp => self.selected = self.selected.saturating_sub(3),
            _ => {
                if self.field.key(key) {
                    self.filter();
                }
            }
        }
        self.follow = true;
        Outcome::Redraw
    }

    /// The text of a card, as it will be drawn.
    fn text_lines(ui: &Ui, entry: &Entry, width: i32) -> Vec<String> {
        let mut lines = ui.wrap(&entry.preview, width);
        if lines.len() > TEXT_LINES {
            let rest = lines[TEXT_LINES - 1..].join(" ");
            lines.truncate(TEXT_LINES - 1);
            lines.push(format!("{rest}{}", "…".repeat(8)));
        }
        lines
    }

    fn card_height(ui: &Ui, entry: &Entry, width: i32) -> i32 {
        let line = ui.line();
        let body = if entry.image { PREVIEW } else { Self::text_lines(ui, entry, width).len().max(1) as i32 * line };
        6 + line + 4 + 6 + body + 8
    }

    pub fn draw(&mut self, ui: &mut Ui, area: Rect, cx: &mut Ctx) -> i32 {
        let t = ui.theme;
        let line = ui.line();
        let (bar, rest) = area.take_top(line + 10, 8);
        // The panel runs the height of the screen; the cards fill it.
        let list = rest;
        let inner_w = list.w - 16;

        // The tool bar: search, pins only, clear.
        let wipe_w = if self.confirm_wipe.is_some() { ui.text_width(tr("Clear all?")) + 12 } else { line + 10 };
        let (wipe, bar) = bar.take_right(wipe_w, 4);
        let (pins, field) = bar.take_right(line + 10, 6);
        ui.field(field, &self.field, tr("Search…"), true, false);
        if let Some(asked) = self.confirm_wipe {
            if ui.button(wipe, tr("Clear all?"), Look::On) && asked.elapsed().as_millis() >= 500 {
                clipboard::wipe();
                self.confirm_wipe = None;
                self.reload();
            }
        } else if ui.icon_button(wipe, icons::TRASH, Look::Framed) {
            self.confirm_wipe = Some(std::time::Instant::now());
        }
        if ui.icon_button(pins, icons::PIN, if self.pinned_only { Look::On } else { Look::Framed }) {
            self.pinned_only = !self.pinned_only;
            self.filter();
        }

        if self.shown.is_empty() {
            let empty = if self.pinned_only { tr("Nothing pinned") } else { tr("Empty") };
            ui.text(Rect::new(list.x, list.y, list.w, line + 12), empty, t.dim, Align::Center);
            return list.bottom() - area.y;
        }

        // Cards differ in height, so scrolling means measuring them; text
        // is wrapped to measure, so the heights are kept until the list changes.
        if self.heights.len() != self.shown.len() {
            self.heights = self.shown.iter().map(|&i| Self::card_height(ui, &self.entries[i], inner_w) + GAP).collect();
        }
        let total: i32 = self.heights.iter().sum::<i32>() - GAP;
        let top_of = |p: usize| self.heights[..p].iter().sum::<i32>();

        self.offset += ui.scrolled_pixels(list);
        if self.follow {
            self.follow = false;
            let top = top_of(self.selected);
            let bottom = top + self.heights[self.selected] - GAP;
            if top < self.offset {
                self.offset = top;
            } else if bottom > self.offset + list.h {
                self.offset = bottom - list.h;
            }
        }
        self.offset = self.offset.clamp(0, (total - list.h).max(0));

        // Cards are cut at the list's edges; only what shows takes clicks.
        ui.canvas.clip = Some(list);
        let inside = |r: Rect| r.y >= list.y && r.y + r.h <= list.bottom();
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
        let mut action = None;
        let mut y = list.y - self.offset;
        for position in 0..self.shown.len() {
            let height = self.heights[position] - GAP;
            let card_y = y;
            y += height + GAP;
            if card_y + height <= list.y {
                continue;
            }
            if card_y >= list.bottom() {
                break;
            }
            let index = self.shown[position];
            let entry = &self.entries[index];
            let y = card_y;
            let visible = {
                let top = y.max(list.y);
                Rect::new(list.x, top, list.w, (y + height).min(list.bottom()) - top)
            };
            let card = Rect::new(list.x, y, list.w, height);
            let current = position == self.selected;
            if current || ui.hovered(visible) {
                ui.fill(card, t.hover);
            }
            ui.frame(card, if current { t.accent } else { t.border });

            // Header: kind, age, and the buttons.
            let header = Rect::new(card.x + 8, card.y + 6, inner_w, line + 4);
            let (pin, header) = header.take_right(line + 4, 2);
            let (delete, header) = header.take_right(line + 4, 6);
            let (kind, header) = header.take_left(8, 6);
            ui.sprite(kind, if entry.image { icons::IMAGE } else { icons::TEXT }, t.fg);
            let label = if entry.image { tr("Image") } else { tr("Text") };
            let (label_rect, header) = header.take_left(ui.text_width(label), 8);
            ui.text(label_rect, label, t.fg, Align::Left);
            let mut detail = entry.time.map(|time| i18n::ago(now - time)).unwrap_or_default();
            if let Some(size) = entry.dimensions() {
                detail = if detail.is_empty() { size.to_owned() } else { format!("{detail} · {size}") };
            }
            ui.text(header, &detail, t.dim, Align::Left);
            if ui.icon_button(pin, icons::PIN, if entry.pin.is_some() { Look::On } else { Look::Flat }) && inside(pin) {
                action = Some(Action::Pin(index));
            }
            if ui.icon_button(delete, icons::TRASH, Look::Flat) && inside(delete) {
                action = Some(Action::Delete(index));
            }

            // Body: the picture, or the first lines of the text.
            let body = Rect::new(card.x + 8, header.bottom() + 6, inner_w, height - (line + 4) - 6 - 6 - 8);
            if entry.image {
                let preview = self.previews.entry(entry.key().to_owned()).or_insert_with(|| {
                    let image = crate::image::decode(&clipboard::contents(entry)?)?;
                    Some(crate::image::cover(&image, inner_w, PREVIEW))
                });
                match preview {
                    Some(image) => {
                        ui.fill(body, t.bg);
                        let x = body.x + (body.w - image.width) / 2;
                        let y = body.y + (body.h - image.height) / 2;
                        ui.canvas.picture(x, y, image);
                    }
                    None => ui.text(body, &entry.preview, t.dim, Align::Center),
                }
            } else {
                for (row, text) in Self::text_lines(ui, entry, inner_w).iter().enumerate() {
                    let rect = Rect::new(body.x, body.y + row as i32 * line, body.w, line);
                    ui.text(rect, text, if current { t.fg } else { t.dim }, Align::Left);
                }
            }

            if action.is_none() && ui.clicked(visible) {
                action = Some(Action::Copy(index));
            }
        }
        ui.canvas.clip = None;
        if let Some(action) = action {
            self.apply(action, cx);
        }
        list.bottom() - area.y
    }
}
