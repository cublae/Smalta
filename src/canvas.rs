//! Drawing into a shared-memory buffer, in whole pixels.
//!
//! Everything is placed in shell pixels and multiplied by `scale` on the way
//! in. A shell pixel covers the device pixels from `floor(x * scale)` up to
//! `floor((x + 1) * scale)`: at a whole scale every one is the same square,
//! at 1.5 they alternate between one and two device pixels wide, and either
//! way an edge falls on a whole device pixel. Nothing here antialiases.

use crate::font::{self, Face};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Color(pub u32);

impl Color {
    pub fn rgb(rgb: u32) -> Self {
        Color(0xff00_0000 | rgb)
    }

    pub fn parse(text: &str) -> Option<Self> {
        let hex = text.strip_prefix('#')?;
        (hex.len() == 6).then(|| u32::from_str_radix(hex, 16).ok().map(Color::rgb)).flatten()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub fn contains(&self, x: f64, y: f64) -> bool {
        x >= self.x as f64 && y >= self.y as f64 && x < (self.x + self.w) as f64 && y < (self.y + self.h) as f64
    }
}

pub struct Canvas<'a> {
    pixels: &'a mut [u8],
    /// Device pixels.
    width: i32,
    height: i32,
    scale: f32,
    /// Drawing outside this, in shell pixels, is dropped: a list that
    /// scrolls cuts its rows at its edges.
    pub clip: Option<Rect>,
    /// Where shell pixel 0, 0 is: a window drawn somewhere inside a
    /// larger surface draws as if it were the whole of it.
    pub offset: (i32, i32),
    /// When set, [`Canvas::image`] recolours what it draws along these
    /// three: darkest, middle, lightest. Icons then take the theme's tones.
    pub tint: Option<[Color; 3]>,
}

impl<'a> Canvas<'a> {
    pub fn new(pixels: &'a mut [u8], width: i32, height: i32, scale: f32) -> Self {
        Self { pixels, width, height, scale, clip: None, offset: (0, 0), tint: None }
    }

    /// A shell coordinate in device pixels.
    fn device(&self, v: i32) -> i32 {
        (v as f32 * self.scale).floor() as i32
    }

    /// Where drawing may land, in device pixels: `x0, y0, x1, y1`.
    fn bounds(&self) -> (i32, i32, i32, i32) {
        match self.clip {
            Some(c) => {
                let (x, y) = (c.x + self.offset.0, c.y + self.offset.1);
                (
                    self.device(x).max(0),
                    self.device(y).max(0),
                    self.device(x + c.w).min(self.width),
                    self.device(y + c.h).min(self.height),
                )
            }
            None => (0, 0, self.width, self.height),
        }
    }

    pub fn clear(&mut self, color: Color) {
        for pixel in self.pixels.as_chunks_mut::<4>().0 {
            pixel.copy_from_slice(&color.0.to_le_bytes());
        }
    }

    pub fn fill(&mut self, rect: Rect, color: Color) {
        let rect = Rect { x: rect.x + self.offset.0, y: rect.y + self.offset.1, ..rect };
        let (bx0, by0, bx1, by1) = self.bounds();
        let x0 = self.device(rect.x).clamp(bx0, bx1.max(bx0));
        let x1 = self.device(rect.x + rect.w).clamp(bx0, bx1.max(bx0));
        let y0 = self.device(rect.y).clamp(by0, by1.max(by0));
        let y1 = self.device(rect.y + rect.h).clamp(by0, by1.max(by0));
        if x0 >= x1 || y0 >= y1 {
            return;
        }
        let bytes = color.0.to_le_bytes();
        for y in y0..y1 {
            let start = ((y * self.width + x0) * 4) as usize;
            let end = ((y * self.width + x1) * 4) as usize;
            for pixel in self.pixels[start..end].as_chunks_mut::<4>().0 {
                *pixel = bytes;
            }
        }
    }

    /// A one-pixel frame just inside `rect`.
    pub fn frame(&mut self, rect: Rect, color: Color) {
        let Rect { x, y, w, h } = rect;
        self.fill(Rect { x, y, w, h: 1 }, color);
        self.fill(Rect { x, y: y + h - 1, w, h: 1 }, color);
        self.fill(Rect { x, y, w: 1, h }, color);
        self.fill(Rect { x: x + w - 1, y, w: 1, h }, color);
    }

    /// A sprite with each of its pixels drawn `mag` x `mag`.
    pub fn sprite_scaled(&mut self, x: i32, y: i32, rows: &[&str], color: Color, mag: i32) {
        for (dy, row) in rows.iter().enumerate() {
            for (dx, cell) in row.chars().enumerate() {
                if cell == '#' {
                    self.fill(Rect { x: x + dx as i32 * mag, y: y + dy as i32 * mag, w: mag, h: mag }, color);
                }
            }
        }
    }

    /// A bitmap sprite: rows of `#` for ink, anything else for nothing.
    pub fn sprite(&mut self, x: i32, y: i32, rows: &[&str], color: Color) {
        for (dy, row) in rows.iter().enumerate() {
            for (dx, cell) in row.chars().enumerate() {
                if cell == '#' {
                    self.fill(Rect { x: x + dx as i32, y: y + dy as i32, w: 1, h: 1 }, color);
                }
            }
        }
    }

    /// Text with its top-left at `x, y`; returns the advance.
    pub fn text(&mut self, face: &Face, x: i32, y: i32, text: &str, color: Color) -> i32 {
        let baseline = y + face.ascent;
        let mut pen = x;
        for piece in font::pieces(face, text) {
            let glyph = match piece {
                font::Piece::Glyph(glyph) => glyph,
                font::Piece::Emoji(image) => {
                    self.picture(pen, baseline - font::EMOJI_TOP, &image);
                    pen += crate::emoji::SIZE;
                    continue;
                }
            };
            let left = pen + glyph.left;
            let top = baseline - glyph.top;
            for row in 0..glyph.height {
                for column in 0..glyph.width {
                    if glyph.bits[(row * glyph.width + column) as usize] {
                        self.fill(Rect { x: left + column, y: top + row, w: 1, h: 1 }, color);
                    }
                }
            }
            pen += glyph.advance;
        }
        pen - x
    }
}

impl Canvas<'_> {
    /// A premultiplied ARGB image scaled to `size` x `size` shell pixels by
    /// nearest neighbour: whatever it was, it comes out as pixel art.
    pub fn image(&mut self, x: i32, y: i32, size: i32, image: &crate::services::tray::Pixmap) {
        if image.width <= 0 || image.height <= 0 || size <= 0 {
            return;
        }
        // Sample once per shell pixel, not per device pixel, so the image is
        // as coarse as the font.
        for py in 0..size {
            for px in 0..size {
                let sx = (px * image.width / size).min(image.width - 1);
                let sy = (py * image.height / size).min(image.height - 1);
                let src = image.pixels[(sy * image.width + sx) as usize];
                let alpha = src >> 24;
                if alpha == 0 {
                    continue;
                }
                let src = match self.tint {
                    Some(ramp) => tinted(src, ramp),
                    None => src,
                };
                self.blend_cell(x + px, y + py, src);
            }
        }
    }

    /// A premultiplied ARGB image at its own size, one image pixel to a
    /// shell pixel.
    pub fn picture(&mut self, x: i32, y: i32, image: &crate::services::tray::Pixmap) {
        for py in 0..image.height {
            for px in 0..image.width {
                let src = image.pixels[(py * image.width + px) as usize];
                if src >> 24 == 0 {
                    continue;
                }
                self.blend_cell(x + px, y + py, src);
            }
        }
    }

    /// `src` over every device pixel of the shell pixel at `x, y`.
    fn blend_cell(&mut self, x: i32, y: i32, src: u32) {
        let (x, y) = (x + self.offset.0, y + self.offset.1);
        for dy in self.device(y)..self.device(y + 1) {
            for dx in self.device(x)..self.device(x + 1) {
                self.blend(dx, dy, src);
            }
        }
    }

    /// `src` over the pixel at `x, y`, both premultiplied.
    fn blend(&mut self, x: i32, y: i32, src: u32) {
        let (x0, y0, x1, y1) = self.bounds();
        if x < x0 || y < y0 || x >= x1 || y >= y1 {
            return;
        }
        let at = ((y * self.width + x) * 4) as usize;
        let dst = u32::from_le_bytes(self.pixels[at..at + 4].try_into().unwrap());
        let inverse = 255 - (src >> 24);
        let channel = |shift: u32| {
            let s = (src >> shift) & 0xff;
            let d = (dst >> shift) & 0xff;
            (s + d * inverse / 255).min(255) << shift
        };
        let out = channel(24) | channel(16) | channel(8) | channel(0);
        self.pixels[at..at + 4].copy_from_slice(&out.to_le_bytes());
    }
}

impl Canvas<'_> {
    /// Text with every font pixel drawn as a `mag` x `mag` block: a big
    /// clock that is still the same pixel font, only closer.
    pub fn text_big(&mut self, face: &crate::font::Face, x: i32, y: i32, text: &str, color: Color, mag: i32) -> i32 {
        let baseline = face.ascent;
        let mut pen = 0;
        for piece in font::pieces(face, text) {
            let glyph = match piece {
                font::Piece::Glyph(glyph) => glyph,
                font::Piece::Emoji(image) => {
                    let size = crate::emoji::SIZE;
                    self.image(x + pen * mag, y + (baseline - font::EMOJI_TOP) * mag, size * mag, &image);
                    pen += size;
                    continue;
                }
            };
            let top = baseline - glyph.top;
            for row in 0..glyph.height {
                for column in 0..glyph.width {
                    if glyph.bits[(row * glyph.width + column) as usize] {
                        let px = x + (pen + glyph.left + column) * mag;
                        let py = y + (top + row) * mag;
                        self.fill(Rect { x: px, y: py, w: mag, h: mag }, color);
                    }
                }
            }
            pen += glyph.advance;
        }
        pen * mag
    }

    /// Every other pixel of `rect`, in a checkerboard: a half-tone fill that
    /// stays crisp where a translucent one would turn to mush.
    pub fn dither(&mut self, rect: Rect, color: Color) {
        for y in rect.y..rect.y + rect.h {
            let start = rect.x + (rect.x + y).rem_euclid(2);
            let mut x = start;
            while x < rect.x + rect.w {
                self.fill(Rect { x, y, w: 1, h: 1 }, color);
                x += 2;
            }
        }
    }
}

impl Canvas<'_> {
    /// Device pixels straight into the buffer, top-left aligned and clipped.
    pub fn blit(&mut self, width: i32, height: i32, pixels: &[u32]) {
        let w = width.min(self.width);
        for y in 0..height.min(self.height) {
            let src = &pixels[(y * width) as usize..(y * width + w) as usize];
            let start = ((y * self.width) * 4) as usize;
            for (dst, p) in self.pixels[start..start + w as usize * 4].as_chunks_mut::<4>().0.iter_mut().zip(src) {
                *dst = p.to_le_bytes();
            }
        }
    }
}

/// A premultiplied pixel's brightness, put on a ramp of three colours: its
/// alpha kept, its hue the ramp's.
fn tinted(src: u32, ramp: [Color; 3]) -> u32 {
    let [a, r, g, b] = src.to_be_bytes();
    let a32 = a as u32;
    // Back from premultiplied, then the eye's weighting of the three.
    let plain = |c: u8| (c as u32 * 255 / a32.max(1)).min(255);
    let light = (plain(r) * 299 + plain(g) * 587 + plain(b) * 114) / 1000;
    let (from, to, t) = if light < 128 { (ramp[0], ramp[1], light * 2) } else { (ramp[1], ramp[2], (light - 128) * 2) };
    let mix = |shift: u32| {
        let (f, e) = ((from.0 >> shift) & 0xff, (to.0 >> shift) & 0xff);
        let c = (f * (255 - t.min(255)) + e * t.min(255)) / 255;
        c * a32 / 255
    };
    (a32 << 24) | (mix(16) << 16) | (mix(8) << 8) | mix(0)
}

impl Canvas<'_> {
    /// An image laid over the whole buffer, each of its pixels a `cell` x
    /// `cell` square of device pixels: a wallpaper in big pixels.
    pub fn blocks(&mut self, image: &crate::services::tray::Pixmap, cell: i32) {
        if image.width <= 0 || image.height <= 0 || cell <= 0 {
            return;
        }
        for y in 0..self.height {
            let row = (y / cell).min(image.height - 1) * image.width;
            let start = (y * self.width * 4) as usize;
            let line = &mut self.pixels[start..start + self.width as usize * 4];
            for (x, pixel) in line.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                let column = (x as i32 / cell).min(image.width - 1);
                *pixel = image.pixels[(row + column) as usize].to_le_bytes();
            }
        }
    }
}

impl Canvas<'_> {
    /// An opaque image stretched over `rect` by nearest neighbour, one
    /// sample per shell pixel: a small picture comes out in big pixels.
    pub fn image_in(&mut self, rect: Rect, image: &crate::services::tray::Pixmap) {
        if image.width <= 0 || image.height <= 0 || rect.w <= 0 || rect.h <= 0 {
            return;
        }
        for py in 0..rect.h {
            let sy = (py * image.height / rect.h).min(image.height - 1);
            for px in 0..rect.w {
                let sx = (px * image.width / rect.w).min(image.width - 1);
                let color = Color(image.pixels[(sy * image.width + sx) as usize] | 0xff00_0000);
                self.fill(Rect { x: rect.x + px, y: rect.y + py, w: 1, h: 1 }, color);
            }
        }
    }
}

impl Canvas<'_> {
    /// A line `width` shell pixels thick from one point to the other: a
    /// square brush stepped along it, every step a whole pixel.
    pub fn line(&mut self, from: (i32, i32), to: (i32, i32), width: i32, color: Color) {
        let (mut x, mut y) = from;
        let (dx, dy) = ((to.0 - x).abs(), -(to.1 - y).abs());
        let (sx, sy) = (if x < to.0 { 1 } else { -1 }, if y < to.1 { 1 } else { -1 });
        let mut error = dx + dy;
        let half = width / 2;
        loop {
            self.fill(Rect { x: x - half, y: y - half, w: width, h: width }, color);
            if (x, y) == to {
                break;
            }
            let twice = error * 2;
            if twice >= dy {
                error += dy;
                x += sx;
            }
            if twice <= dx {
                error += dx;
                y += sy;
            }
        }
    }
}
