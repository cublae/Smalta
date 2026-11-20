//! Screenshots: freeze the screens, drag out a region, mark it up, copy it
//! and keep it.
//!
//! Every output is frozen at once through wlr-screencopy, as for the colour
//! picker, and the region is cut from that copy: what was on screen at the
//! moment of asking, not whatever moved since. On the region go boxes,
//! arrows, lines drawn by hand and pixelated patches, in whole shell pixels
//! like everything else; the same drawing code paints them once more, at the
//! screen's own resolution, into the picture that is saved.

use std::path::PathBuf;

use crate::canvas::{Canvas, Color, Rect};
use crate::font::Face;
use crate::i18n::tr;
use crate::icons::{self, Icon};
use crate::picker::Capture;
use crate::theme::Theme;
use crate::ui::{Input, Key, Look, Ui};

#[derive(Clone, Copy, PartialEq)]
pub enum Tool {
    Box,
    Arrow,
    Pen,
    Pixelate,
}

const TOOLS: [(Tool, Icon); 4] =
    [(Tool::Box, icons::BOX), (Tool::Arrow, icons::ARROW), (Tool::Pen, icons::PEN), (Tool::Pixelate, icons::PIXELS)];

/// Ink to mark with; the first is the theme's accent.
const INKS: [Option<u32>; 4] = [None, Some(0xffe0_4848), Some(0xffff_d23f), Some(0xffff_ffff)];

/// A pixelated patch's cells, in shell pixels.
const PATCH: i32 = 8;
const STROKE: i32 = 2;
const BUTTON: i32 = 22;

#[derive(Clone)]
enum Mark {
    Box(Rect, Color),
    Arrow((i32, i32), (i32, i32), Color),
    Pen(Vec<(i32, i32)>, Color),
    Pixelate(Rect),
}

#[derive(Clone, Copy)]
enum Drag {
    /// Pulling out the region from where the press was.
    Select((i32, i32)),
    /// Drawing the mark at the end of the list, from where the press was.
    Draw((i32, i32)),
}

/// What finishing does with the picture.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Finish {
    Copy,
    Save,
    Cancel,
}

#[derive(Default)]
pub struct Shot {
    pub captures: Vec<Capture>,
    /// The output the region is on, and the region, in its shell pixels.
    output: Option<String>,
    region: Option<Rect>,
    drag: Option<Drag>,
    tool: Option<Tool>,
    ink: usize,
    marks: Vec<Mark>,
    /// Where the toolbar was last drawn, to tell its clicks from drawing.
    pub toolbar: Option<(String, Rect)>,
}

fn normalized(a: (i32, i32), b: (i32, i32)) -> Rect {
    Rect::new(a.0.min(b.0), a.1.min(b.1), (a.0 - b.0).abs(), (a.1 - b.1).abs())
}

impl Shot {
    pub fn active(&self) -> bool {
        !self.captures.is_empty()
    }

    pub fn clear(&mut self) {
        *self = Shot::default();
    }

    fn ink(&self, theme: &Theme) -> Color {
        INKS[self.ink].map(Color).unwrap_or(theme.accent)
    }

    /// A press on `output` at `x, y`, not on the toolbar.
    pub fn press(&mut self, output: &str, x: i32, y: i32, theme: &Theme) {
        let inside =
            self.output.as_deref() == Some(output) && self.region.is_some_and(|r| r.contains(x as f64, y as f64));
        match self.tool {
            // With a tool, inside the region draws.
            Some(tool) if inside => {
                let ink = self.ink(theme);
                self.marks.push(match tool {
                    Tool::Box => Mark::Box(Rect::new(x, y, 0, 0), ink),
                    Tool::Arrow => Mark::Arrow((x, y), (x, y), ink),
                    Tool::Pen => Mark::Pen(vec![(x, y)], ink),
                    Tool::Pixelate => Mark::Pixelate(Rect::new(x, y, 0, 0)),
                });
                self.drag = Some(Drag::Draw((x, y)));
            }
            // Anywhere else, a new region.
            _ => {
                self.output = Some(output.to_owned());
                self.region = Some(Rect::new(x, y, 0, 0));
                self.marks.clear();
                self.drag = Some(Drag::Select((x, y)));
            }
        }
    }

    pub fn motion(&mut self, output: &str, x: i32, y: i32) {
        if self.output.as_deref() != Some(output) {
            return;
        }
        match self.drag {
            Some(Drag::Select(from)) => self.region = Some(normalized(from, (x, y))),
            Some(Drag::Draw(from)) => {
                // A mark stays inside the region.
                let Some(r) = self.region else { return };
                let (x, y) = (x.clamp(r.x, r.x + r.w - 1), y.clamp(r.y, r.y + r.h - 1));
                match self.marks.last_mut() {
                    Some(Mark::Box(rect, _) | Mark::Pixelate(rect)) => *rect = normalized(from, (x, y)),
                    Some(Mark::Arrow(_, to, _)) => *to = (x, y),
                    Some(Mark::Pen(points, _)) if points.last() != Some(&(x, y)) => points.push((x, y)),
                    Some(_) => {}
                    None => {}
                }
            }
            None => {}
        }
    }

    /// The button let go. A region that is only a click is the whole output.
    pub fn release(&mut self, size: (i32, i32)) {
        if let Some(Drag::Select(_)) = self.drag
            && self.region.is_some_and(|r| r.w < 4 || r.h < 4)
        {
            self.region = Some(Rect::new(0, 0, size.0, size.1));
        }
        if self.drag.is_some() && self.tool.is_none() {
            self.tool = Some(Tool::Box);
        }
        self.drag = None;
    }

    pub fn key(&mut self, key: &Key) -> Option<Finish> {
        match key {
            Key::Escape => return Some(Finish::Cancel),
            Key::Enter | Key::Char('c') if self.region.is_some() => return Some(Finish::Copy),
            Key::Char('s') if self.region.is_some() => return Some(Finish::Save),
            Key::Backspace | Key::Delete | Key::Char('z') => {
                self.marks.pop();
            }
            Key::Char(c @ '1'..='4') => self.tool = Some(TOOLS[*c as usize - '1' as usize].0),
            Key::Tab => self.ink = (self.ink + 1) % INKS.len(),
            _ => {}
        }
        None
    }

    /// The frozen `capture` with the region, its marks and the toolbar over
    /// it. `ratio` is the capture's pixels to a shell pixel.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &mut self,
        canvas: &mut Canvas,
        input: &mut Input,
        capture: &Capture,
        size: (i32, i32),
        ratio: f32,
        theme: &Theme,
        face: &Face,
    ) -> (Vec<Rect>, Option<Finish>) {
        canvas.blit(capture.width, capture.height, &capture.pixels);
        let here = self.output.as_deref() == Some(capture.output.as_str());
        let region = self.region.filter(|_| here);
        let shade = Color(0xff00_0000);
        let screen = Rect::new(0, 0, size.0, size.1);
        let Some(r) = region else {
            canvas.dither(screen, shade);
            if self.region.is_none() {
                hint(canvas, theme, face, size);
            }
            return (Vec::new(), None);
        };
        // Outside the region, half the pixels black.
        canvas.dither(Rect::new(0, 0, size.0, r.y), shade);
        canvas.dither(Rect::new(0, r.bottom(), size.0, size.1 - r.bottom()), shade);
        canvas.dither(Rect::new(0, r.y, r.x, r.h), shade);
        canvas.dither(Rect::new(r.x + r.w, r.y, size.0 - r.x - r.w, r.h), shade);
        canvas.clip = Some(r);
        draw_marks(canvas, &self.marks, capture, ratio);
        canvas.clip = None;
        canvas.frame(Rect::new(r.x - 1, r.y - 1, r.w + 2, r.h + 2), theme.accent);

        // The size in the picture's own pixels, over the region's corner.
        let label = format!("{}×{}", (r.w as f32 * ratio).round(), (r.h as f32 * ratio).round());
        let width = crate::font::text_width(face, &label) + 8;
        let line = face.line_height();
        let above = if r.y >= line + 4 { r.y - line - 4 } else { r.y + 2 };
        canvas.fill(Rect::new(r.x - 1, above, width, line + 2), theme.bg);
        canvas.text(face, r.x + 3, above + 1, &label, theme.fg);

        if matches!(self.drag, Some(Drag::Select(_))) {
            return (Vec::new(), None);
        }
        self.toolbar(canvas, input, r, size, theme, face, &capture.output)
    }

    #[allow(clippy::too_many_arguments)]
    fn toolbar(
        &mut self,
        canvas: &mut Canvas,
        input: &mut Input,
        r: Rect,
        size: (i32, i32),
        theme: &Theme,
        face: &Face,
        output: &str,
    ) -> (Vec<Rect>, Option<Finish>) {
        let gap = 4;
        let count = TOOLS.len() + INKS.len() + 4;
        let w = count as i32 * (BUTTON + gap) + gap * 3;
        let h = BUTTON + gap * 2;
        // Under the region, or over it when there is no room, or inside it.
        let y = if r.bottom() + 6 + h <= size.1 {
            r.bottom() + 6
        } else if r.y - 6 - h >= 0 {
            r.y - 6 - h
        } else {
            r.bottom() - h - 6
        };
        let x = (r.x + r.w - w).clamp(0, (size.0 - w).max(0));
        let bar = Rect::new(x, y, w, h);
        self.toolbar = Some((output.to_owned(), bar));
        canvas.fill(Rect::new(bar.x + 3, bar.y + 3, bar.w, bar.h), Color(0x9900_0000));
        canvas.fill(bar, theme.bg);
        canvas.frame(bar, theme.border);

        let mut ui = Ui { canvas, face, theme, input, hot: Vec::new() };
        let mut at = bar.x + gap;
        let mut next = |extra: i32| {
            let cell = Rect::new(at, bar.y + gap, BUTTON, BUTTON);
            at += BUTTON + gap + extra;
            cell
        };
        let mut finish = None;
        for (i, (tool, icon)) in TOOLS.iter().enumerate() {
            let look = if self.tool == Some(*tool) { Look::On } else { Look::Flat };
            if ui.icon_button(next(if i == TOOLS.len() - 1 { gap * 2 } else { 0 }), icon, look) {
                self.tool = Some(*tool);
            }
        }
        for (i, ink) in INKS.iter().enumerate() {
            let cell = next(if i == INKS.len() - 1 { gap * 2 } else { 0 });
            let ink = ink.map(Color).unwrap_or(theme.accent);
            if i == self.ink {
                ui.frame(cell, theme.fg);
            }
            ui.fill(cell.inset(5), ink);
            if ui.clicked(cell) {
                self.ink = i;
            }
        }
        if ui.icon_button(next(0), icons::UNDO, Look::Flat) {
            self.marks.pop();
        }
        if ui.icon_button(next(0), icons::SAVE, Look::Flat) {
            finish = Some(Finish::Save);
        }
        if ui.icon_button(next(0), icons::CLIPBOARD, Look::Flat) {
            finish = Some(Finish::Copy);
        }
        if ui.icon_button(next(0), icons::CROSS, Look::Flat) {
            finish = Some(Finish::Cancel);
        }
        let hot = std::mem::take(&mut ui.hot);
        input.click = None;
        (hot, finish)
    }

    /// The region with its marks, as an 8-bit RGBA picture at the screen's
    /// resolution: width, height and pixels.
    pub fn render(&self, ratio_of: impl Fn(&Capture) -> f32) -> Option<(u32, u32, Vec<u8>)> {
        let output = self.output.as_deref()?;
        let capture = self.captures.iter().find(|c| c.output == output)?;
        let r = self.region?;
        let ratio = ratio_of(capture);
        let device = |v: i32| (v as f32 * ratio).floor() as i32;
        let (x0, y0) = (device(r.x).clamp(0, capture.width), device(r.y).clamp(0, capture.height));
        let (x1, y1) = (device(r.x + r.w).clamp(0, capture.width), device(r.y + r.h).clamp(0, capture.height));
        let (w, h) = (x1 - x0, y1 - y0);
        if w <= 0 || h <= 0 {
            return None;
        }
        let mut bytes = Vec::with_capacity((w * h * 4) as usize);
        for y in y0..y1 {
            let row = (y * capture.width) as usize;
            for p in &capture.pixels[row + x0 as usize..row + x1 as usize] {
                bytes.extend_from_slice(&p.to_le_bytes());
            }
        }
        // The marks drawn again, at this resolution, as if the region were the screen.
        let mut canvas = Canvas::new(&mut bytes, w, h, ratio);
        canvas.offset = (-r.x, -r.y);
        draw_marks(&mut canvas, &self.marks, capture, ratio);
        // Premultiplied BGRA in memory to plain RGBA; the screen is opaque.
        for pixel in bytes.as_chunks_mut::<4>().0 {
            pixel.swap(0, 2);
            pixel[3] = 255;
        }
        Some((w as u32, h as u32, bytes))
    }
}

/// What to do, before there is a region.
fn hint(canvas: &mut Canvas, theme: &Theme, face: &Face, size: (i32, i32)) {
    let text = tr("Drag to select · click for the whole screen · Esc cancels");
    let w = crate::font::text_width(face, text) + 24;
    let h = face.line_height() + 12;
    let panel = Rect::new((size.0 - w) / 2, 48, w, h);
    canvas.fill(Rect::new(panel.x + 3, panel.y + 3, w, h), Color(0x9900_0000));
    canvas.fill(panel, theme.bg);
    canvas.frame(panel, theme.border);
    canvas.text(face, panel.x + 12, panel.y + 6, text, theme.fg);
}

fn draw_marks(canvas: &mut Canvas, marks: &[Mark], capture: &Capture, ratio: f32) {
    for mark in marks {
        match mark {
            Mark::Box(rect, ink) => {
                for i in 0..STROKE {
                    canvas.frame(Rect::new(rect.x + i, rect.y + i, rect.w - i * 2, rect.h - i * 2), *ink);
                }
            }
            Mark::Arrow(from, to, ink) => arrow(canvas, *from, *to, *ink),
            Mark::Pen(points, ink) => {
                for pair in points.windows(2) {
                    canvas.line(pair[0], pair[1], STROKE, *ink);
                }
                if let [only] = points.as_slice() {
                    canvas.line(*only, *only, STROKE, *ink);
                }
            }
            Mark::Pixelate(rect) => pixelate(canvas, *rect, capture, ratio),
        }
    }
}

fn arrow(canvas: &mut Canvas, from: (i32, i32), to: (i32, i32), ink: Color) {
    canvas.line(from, to, STROKE, ink);
    let (dx, dy) = ((to.0 - from.0) as f32, (to.1 - from.1) as f32);
    let length = (dx * dx + dy * dy).sqrt();
    if length < 2.0 {
        return;
    }
    // Two barbs back from the tip, at thirty degrees either side.
    let head = 12.0f32.min(length / 2.0);
    let angle = dy.atan2(dx);
    for side in [-1.0f32, 1.0] {
        let a = angle + std::f32::consts::PI - side * std::f32::consts::FRAC_PI_6;
        let end = (to.0 + (a.cos() * head).round() as i32, to.1 + (a.sin() * head).round() as i32);
        canvas.line(to, end, STROKE, ink);
    }
}

/// `rect` in cells of [`PATCH`] shell pixels, each the average of what the
/// screen showed under it.
fn pixelate(canvas: &mut Canvas, rect: Rect, capture: &Capture, ratio: f32) {
    let mut y = rect.y;
    while y < rect.bottom() {
        let mut x = rect.x;
        let ch = PATCH.min(rect.bottom() - y);
        while x < rect.x + rect.w {
            let cw = PATCH.min(rect.x + rect.w - x);
            let device = |v: i32| (v as f32 * ratio) as i32;
            let mut sum = [0u64; 3];
            let mut count = 0u64;
            for sy in device(y)..device(y + ch) {
                for sx in device(x)..device(x + cw) {
                    if let Some(p) = crate::picker::at(capture, sx, sy) {
                        sum[0] += ((p >> 16) & 0xff) as u64;
                        sum[1] += ((p >> 8) & 0xff) as u64;
                        sum[2] += (p & 0xff) as u64;
                        count += 1;
                    }
                }
            }
            if count > 0 {
                let [r, g, b] = sum.map(|s| (s / count) as u32);
                canvas.fill(Rect::new(x, y, cw, ch), Color(0xff00_0000 | (r << 16) | (g << 8) | b));
            }
            x += PATCH;
        }
        y += PATCH;
    }
}

/// `~/Pictures/Screenshots`, or wherever `XDG_PICTURES_DIR` says pictures go.
fn folder() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    let pictures = std::env::var("XDG_PICTURES_DIR").unwrap_or_else(|_| format!("{home}/Pictures"));
    PathBuf::from(pictures).join("Screenshots")
}

/// The picture as a PNG.
pub fn encode(width: u32, height: u32, rgba: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().ok()?;
    writer.write_image_data(rgba).ok()?;
    writer.finish().ok()?;
    Some(out)
}

/// Encodes, copies and, when asked, keeps the picture, off the event loop:
/// a large one takes a moment. Returns where it will be kept.
pub fn deliver(width: u32, height: u32, rgba: Vec<u8>, keep: bool) -> Option<PathBuf> {
    let name = chrono::Local::now().format("Screenshot %Y-%m-%d %H-%M-%S.png").to_string();
    let path = keep.then(|| folder().join(name));
    let target = path.clone();
    std::thread::spawn(move || {
        let Some(png) = encode(width, height, &rgba) else {
            log::error!("could not encode the screenshot");
            return;
        };
        crate::clipboard::copy_typed(&png, "image/png");
        if let Some(path) = target {
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            if let Err(error) = std::fs::write(&path, &png) {
                log::error!("{}: {error}", path.display());
            }
        }
    });
    path
}

/// Text to show for a finished screenshot.
pub fn summary(path: Option<&PathBuf>) -> String {
    match path {
        Some(path) => format!("{} {}", tr("Saved to"), path.display()),
        None => tr("Screenshot copied").to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_the_region_with_its_marks() {
        // A 40x20 screen of grey at two device pixels a shell pixel.
        let capture = Capture { output: "A".into(), width: 40, height: 20, pixels: vec![0xff80_8080; 800] };
        let mut shot = Shot { captures: vec![capture], ..Shot::default() };
        let theme = Theme::new(&crate::config::Theme::default());
        shot.press("A", 2, 2, &theme);
        shot.motion("A", 12, 8);
        shot.release((20, 10));
        // A red box over the whole region.
        shot.ink = 1;
        shot.press("A", 2, 2, &theme);
        shot.motion("A", 11, 7);
        shot.release((20, 10));
        let (w, h, rgba) = shot.render(|_| 2.0).unwrap();
        assert_eq!((w, h), (20, 12));
        // The box's corner is red, the middle grey still.
        assert_eq!(&rgba[0..4], &[0xe0, 0x48, 0x48, 255]);
        let middle = ((4 * w + 10) * 4) as usize;
        assert_eq!(&rgba[middle..middle + 4], &[0x80, 0x80, 0x80, 255]);
        let png = encode(w, h, &rgba).unwrap();
        let back = crate::image::decode(&png).unwrap();
        assert_eq!((back.width, back.height), (20, 12));
    }
}
