//! Immediate-mode widgets for popups.
//!
//! A popup is drawn from scratch whenever something changes, and a widget
//! reports a click by checking the pending click against its own rectangle as
//! it is drawn. There is no retained tree to keep in step with the state: the
//! state is whatever the popup's draw function reads.

use crate::canvas::{Canvas, Color, Rect};
use crate::font::{self, Face};
use crate::theme::Theme;

/// Pointer and keyboard input for one draw, in the popup's own pixel units.
#[derive(Default)]
pub struct Input {
    pub pointer: Option<(f64, f64)>,
    /// A press waiting to be claimed by whichever widget lies under it.
    pub click: Option<(f64, f64)>,
    /// True while the button is held, for dragging sliders.
    pub held: bool,
    /// Scroll steps at the pointer: positive is down.
    pub scroll: i32,
    /// The same scrolling in pixels, for lists that move smoothly: a wheel
    /// notch counts as several lines, a touchpad as far as the fingers went.
    pub pixels: f64,
}

#[derive(Clone, Copy, PartialEq)]
pub enum Align {
    Left,
    Center,
    Right,
}

pub struct Ui<'a, 'b> {
    pub canvas: &'a mut Canvas<'b>,
    pub face: &'a Face,
    pub theme: &'a Theme,
    pub input: &'a mut Input,
    /// Clickable rectangles, for the pointer shape.
    pub hot: Vec<Rect>,
}

impl Rect {
    pub fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        Rect { x, y, w, h }
    }

    pub fn inset(self, by: i32) -> Rect {
        Rect::new(self.x + by, self.y + by, (self.w - by * 2).max(0), (self.h - by * 2).max(0))
    }

    /// The left `w` pixels, and what is left after a `gap`.
    pub fn take_left(self, w: i32, gap: i32) -> (Rect, Rect) {
        let w = w.min(self.w);
        (Rect::new(self.x, self.y, w, self.h), Rect::new(self.x + w + gap, self.y, (self.w - w - gap).max(0), self.h))
    }

    pub fn take_right(self, w: i32, gap: i32) -> (Rect, Rect) {
        let w = w.min(self.w);
        (
            Rect::new(self.x + self.w - w, self.y, w, self.h),
            Rect::new(self.x, self.y, (self.w - w - gap).max(0), self.h),
        )
    }

    pub fn take_top(self, h: i32, gap: i32) -> (Rect, Rect) {
        let h = h.min(self.h);
        (Rect::new(self.x, self.y, self.w, h), Rect::new(self.x, self.y + h + gap, self.w, (self.h - h - gap).max(0)))
    }

    /// `n` equal columns with `gap` between them.
    pub fn columns(self, n: i32, gap: i32) -> Vec<Rect> {
        let n = n.max(1);
        let w = (self.w - gap * (n - 1)) / n;
        (0..n).map(|i| Rect::new(self.x + i * (w + gap), self.y, w, self.h)).collect()
    }

    pub fn bottom(&self) -> i32 {
        self.y + self.h
    }
}

/// How a button looks at rest.
#[derive(Clone, Copy, PartialEq)]
pub enum Look {
    /// Nothing until hovered.
    Flat,
    /// A one-pixel frame.
    Framed,
    /// Filled with the accent.
    On,
}

impl Ui<'_, '_> {
    pub fn hovered(&self, rect: Rect) -> bool {
        self.input.pointer.is_some_and(|(x, y)| rect.contains(x, y))
    }

    /// Claims the pending click if it fell inside `rect`.
    pub fn clicked(&mut self, rect: Rect) -> bool {
        self.hot.push(rect);
        if self.input.click.is_some_and(|(x, y)| rect.contains(x, y)) {
            self.input.click = None;
            return true;
        }
        false
    }

    /// Scroll steps over `rect`, consumed.
    pub fn scrolled(&mut self, rect: Rect) -> i32 {
        if self.hovered(rect) {
            return std::mem::take(&mut self.input.scroll);
        }
        0
    }

    /// Scrolling over `rect` in pixels, consumed.
    pub fn scrolled_pixels(&mut self, rect: Rect) -> i32 {
        if self.hovered(rect) {
            self.input.scroll = 0;
            return std::mem::take(&mut self.input.pixels).round() as i32;
        }
        0
    }

    pub fn line(&self) -> i32 {
        self.face.line_height()
    }

    pub fn text_width(&self, text: &str) -> i32 {
        font::text_width(self.face, text)
    }

    pub fn fill(&mut self, rect: Rect, color: Color) {
        self.canvas.fill(rect, color);
    }

    pub fn frame(&mut self, rect: Rect, color: Color) {
        self.canvas.frame(rect, color);
    }

    /// One line of text, vertically centred in `rect` and cut to fit it.
    pub fn text(&mut self, rect: Rect, text: &str, color: Color, align: Align) {
        let text = self.fit(text, rect.w);
        let width = self.text_width(&text);
        let x = match align {
            Align::Left => rect.x,
            Align::Center => rect.x + (rect.w - width) / 2,
            Align::Right => rect.x + rect.w - width,
        };
        let y = rect.y + (rect.h - self.line()) / 2;
        self.canvas.text(self.face, x, y, &text, color);
    }

    /// `text`, shortened with an ellipsis until it fits `width`.
    pub fn fit(&self, text: &str, width: i32) -> String {
        if self.text_width(text) <= width {
            return text.to_owned();
        }
        let mut chars: Vec<char> = text.chars().collect();
        while !chars.is_empty() {
            chars.pop();
            let candidate: String = chars.iter().collect::<String>() + "…";
            if self.text_width(&candidate) <= width {
                return candidate;
            }
        }
        String::new()
    }

    pub fn sprite(&mut self, rect: Rect, rows: &[&str], color: Color) {
        let w = rows.iter().map(|r| r.chars().count() as i32).max().unwrap_or(0);
        let h = rows.len() as i32;
        self.canvas.sprite(rect.x + (rect.w - w) / 2, rect.y + (rect.h - h) / 2, rows, color);
    }

    /// The colours a button is drawn in: fill (if any), frame (if any), ink.
    fn button_colors(&self, rect: Rect, look: Look) -> (Option<Color>, Option<Color>, Color) {
        let t = self.theme;
        let hovered = self.hovered(rect);
        match look {
            Look::On => (Some(t.accent), Some(t.accent), t.on_accent),
            Look::Framed if hovered => (Some(t.hover), Some(t.fg), t.fg),
            Look::Framed => (None, Some(t.border), t.fg),
            Look::Flat if hovered => (Some(t.hover), Some(t.border), t.fg),
            Look::Flat => (None, None, t.fg),
        }
    }

    fn button_frame(&mut self, rect: Rect, look: Look) -> Color {
        let (fill, edge, ink) = self.button_colors(rect, look);
        if let Some(fill) = fill {
            self.fill(rect, fill);
        }
        if let Some(edge) = edge {
            self.frame(rect, edge);
        }
        ink
    }

    pub fn button(&mut self, rect: Rect, label: &str, look: Look) -> bool {
        let ink = self.button_frame(rect, look);
        self.text(rect.inset(4), label, ink, Align::Center);
        self.clicked(rect)
    }

    pub fn icon_button(&mut self, rect: Rect, icon: &[&str], look: Look) -> bool {
        let ink = self.button_frame(rect, look);
        self.sprite(rect, icon, ink);
        self.clicked(rect)
    }

    /// A row the whole width of `rect`: icon, title, and optional detail on the right.
    pub fn row(&mut self, rect: Rect, icon: Option<&[&str]>, title: &str, detail: &str, current: bool) -> bool {
        let t = self.theme;
        let hovered = self.hovered(rect);
        if current {
            self.fill(rect, t.hover);
            self.fill(Rect::new(rect.x, rect.y, 2, rect.h), t.accent);
        } else if hovered {
            self.fill(rect, t.hover);
        }
        let mut body = rect.inset(0);
        body.x += 8;
        body.w -= 16;
        if let Some(icon) = icon {
            let (cell, rest) = body.take_left(12, 8);
            self.sprite(cell, icon, if current { t.accent } else { t.fg });
            body = rest;
        }
        if !detail.is_empty() {
            let width = self.text_width(detail).min(body.w / 2);
            let (right, rest) = body.take_right(width, 8);
            self.text(right, detail, t.dim, Align::Right);
            body = rest;
        }
        self.text(body, title, if current { t.accent } else { t.fg }, Align::Left);
        self.clicked(rect)
    }

    /// `[x] label` in pixels: a box that fills when on.
    pub fn toggle(&mut self, rect: Rect, on: bool, label: &str, detail: &str) -> bool {
        let t = self.theme;
        if self.hovered(rect) {
            self.fill(rect, t.hover);
        }
        let inner = Rect::new(rect.x + 8, rect.y, rect.w - 16, rect.h);
        let (cell, rest) = inner.take_left(10, 8);
        let mark = Rect::new(cell.x, cell.y + (cell.h - 10) / 2, 10, 10);
        self.frame(mark, if on { t.accent } else { t.border });
        if on {
            self.fill(mark.inset(2), t.accent);
        }
        let (title, rest) = rest.take_left(self.text_width(label), 8);
        self.text(title, label, t.fg, Align::Left);
        self.text(rest, detail, t.dim, Align::Right);
        self.clicked(rect)
    }

    /// Segments lit up to `value`, 0..1.
    pub fn cells(&mut self, rect: Rect, value: f32, cell: i32, gap: i32) {
        let t = self.theme;
        let count = ((rect.w + gap) / (cell + gap)).max(1);
        let lit = (value.clamp(0.0, 1.0) * count as f32).round() as i32;
        for i in 0..count {
            let color = if i < lit { t.fg } else { t.faint };
            self.fill(Rect::new(rect.x + i * (cell + gap), rect.y, cell, rect.h), color);
        }
    }

    /// A segmented slider: click or drag to set, scroll to step. Returns the
    /// new value when the user moved it.
    pub fn slider(&mut self, rect: Rect, value: f32) -> Option<f32> {
        let track = Rect::new(rect.x, rect.y + (rect.h - 8) / 2, rect.w, 8);
        self.cells(track, value, 3, 1);
        self.hot.push(rect);
        let at = |x: f64| ((x - rect.x as f64) / rect.w as f64).clamp(0.0, 1.0) as f32;
        if let Some((x, y)) = self.input.click
            && rect.contains(x, y)
        {
            self.input.click = None;
            return Some(at(x));
        }
        if self.input.held
            && let Some((x, y)) = self.input.pointer
            && rect.contains(x, y)
        {
            return Some(at(x));
        }
        let steps = self.scrolled(rect);
        (steps != 0).then(|| (value - steps as f32 * 0.05).clamp(0.0, 1.0))
    }

    /// A one-line entry. Editing happens in [`Field::key`]; this only draws it.
    pub fn field(&mut self, rect: Rect, field: &Field, placeholder: &str, focused: bool, password: bool) {
        let t = self.theme;
        self.fill(rect, t.bg);
        self.frame(rect, if focused { t.fg } else { t.border });
        let inner = rect.inset(1);
        let body = Rect::new(inner.x + 6, inner.y, inner.w - 12, inner.h);
        if field.text.is_empty() {
            self.text(body, placeholder, t.dim, Align::Left);
            if focused {
                self.caret(body, 0);
            }
            return;
        }
        let shown: String = if password { "•".repeat(field.text.chars().count()) } else { field.text.clone() };
        // Keep the caret in view: drop characters from the left until it fits.
        let before: String = shown.chars().take(field.cursor).collect();
        let mut skip = 0;
        while self.text_width(&before.chars().skip(skip).collect::<String>()) > body.w - 2 {
            skip += 1;
        }
        let visible: String = shown.chars().skip(skip).collect();
        self.text(body, &visible, t.fg, Align::Left);
        if focused {
            let x = self.text_width(&before.chars().skip(skip).collect::<String>());
            self.caret(body, x);
        }
    }

    fn caret(&mut self, body: Rect, x: i32) {
        let h = self.line() - 2;
        let caret = Rect::new(body.x + x, body.y + (body.h - h) / 2, 1, h);
        self.fill(caret, self.theme.fg);
    }

    /// A titled panel: the frame with its title cut into the top edge.
    pub fn panel(&mut self, rect: Rect, title: &str) {
        let t = self.theme;
        self.frame(rect, t.border);
        if !title.is_empty() {
            let w = self.text_width(title) + 8;
            let label = Rect::new(rect.x + 8, rect.y - self.line() / 2, w, self.line());
            self.fill(label, t.bg);
            self.text(label, title, t.dim, Align::Center);
        }
    }
}

/// A text entry's contents and caret. The caret counts characters, not bytes.
#[derive(Clone, Default)]
pub struct Field {
    pub text: String,
    pub cursor: usize,
}

/// Keys the shell reacts to, already translated through the keymap.
#[derive(Clone, Debug, PartialEq)]
pub enum Key {
    Char(char),
    Backspace,
    /// Ctrl+Backspace: the word before the caret.
    BackspaceWord,
    Delete,
    Enter,
    Escape,
    Tab,
    BackTab,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
}

impl Field {
    fn byte(&self, at: usize) -> usize {
        self.text.char_indices().nth(at).map_or(self.text.len(), |(i, _)| i)
    }

    /// Applies an editing key; true if the text changed.
    pub fn key(&mut self, key: &Key) -> bool {
        let len = self.text.chars().count();
        match key {
            Key::Char(c) => {
                let at = self.byte(self.cursor);
                self.text.insert(at, *c);
                self.cursor += 1;
                true
            }
            Key::Backspace if self.cursor > 0 => {
                let at = self.byte(self.cursor - 1);
                self.text.remove(at);
                self.cursor -= 1;
                true
            }
            Key::BackspaceWord if self.cursor > 0 => {
                let chars: Vec<char> = self.text.chars().collect();
                let mut start = self.cursor;
                while start > 0 && chars[start - 1].is_whitespace() {
                    start -= 1;
                }
                while start > 0 && !chars[start - 1].is_whitespace() {
                    start -= 1;
                }
                let (from, to) = (self.byte(start), self.byte(self.cursor));
                self.text.replace_range(from..to, "");
                self.cursor = start;
                true
            }
            Key::Delete if self.cursor < len => {
                let at = self.byte(self.cursor);
                self.text.remove(at);
                true
            }
            Key::Left => {
                self.cursor = self.cursor.saturating_sub(1);
                false
            }
            Key::Right => {
                self.cursor = (self.cursor + 1).min(len);
                false
            }
            Key::Home => {
                self.cursor = 0;
                false
            }
            Key::End => {
                self.cursor = len;
                false
            }
            _ => false,
        }
    }
}

impl Ui<'_, '_> {
    /// `text` broken into lines no wider than `width`, at spaces where it can
    /// be and anywhere where a word is too long on its own.
    pub fn wrap(&self, text: &str, width: i32) -> Vec<String> {
        let mut lines = Vec::new();
        for paragraph in text.lines() {
            let mut current = String::new();
            for word in paragraph.split(' ') {
                let candidate = if current.is_empty() { word.to_owned() } else { format!("{current} {word}") };
                if self.text_width(&candidate) <= width {
                    current = candidate;
                    continue;
                }
                if !current.is_empty() {
                    lines.push(std::mem::take(&mut current));
                }
                let mut piece = String::new();
                for c in word.chars() {
                    piece.push(c);
                    if self.text_width(&piece) > width {
                        piece.pop();
                        lines.push(std::mem::take(&mut piece));
                        piece.push(c);
                    }
                }
                current = piece;
            }
            lines.push(current);
        }
        while lines.last().is_some_and(String::is_empty) {
            lines.pop();
        }
        lines
    }
}
