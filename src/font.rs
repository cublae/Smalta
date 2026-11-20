//! Bitmap fonts: a BDF face, with GNU Unifont behind it for everything else.
//!
//! Both formats are text and small enough to read by hand, which keeps the
//! shell free of a font stack. Glyphs are kept as bit rows and drawn as whole
//! pixels: there is no antialiasing to lose by scaling them.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, OnceLock};

use crate::services::tray::Pixmap;

pub struct Glyph {
    pub advance: i32,
    pub width: i32,
    pub height: i32,
    /// Offset of the bitmap's left edge from the pen position.
    pub left: i32,
    /// Offset of the bitmap's top edge above the baseline.
    pub top: i32,
    /// One bit per pixel, row-major, `width` bits to a row.
    pub bits: Vec<bool>,
}

pub struct Face {
    pub ascent: i32,
    pub descent: i32,
    glyphs: HashMap<u32, Glyph>,
}

impl Face {
    pub fn line_height(&self) -> i32 {
        self.ascent + self.descent
    }

    pub fn load_bdf(path: &Path) -> std::io::Result<Face> {
        let text = std::fs::read_to_string(path)?;
        Ok(parse_bdf(&text))
    }
}

fn parse_bdf(text: &str) -> Face {
    let mut face = Face { ascent: 0, descent: 0, glyphs: HashMap::new() };
    let mut lines = text.lines();
    let mut bounding = (0, 0, 0, 0);

    while let Some(line) = lines.next() {
        let mut words = line.split_whitespace();
        match words.next() {
            Some("FONTBOUNDINGBOX") => {
                let n: Vec<i32> = words.filter_map(|w| w.parse().ok()).collect();
                if n.len() == 4 {
                    bounding = (n[0], n[1], n[2], n[3]);
                }
            }
            Some("FONT_ASCENT") => face.ascent = words.next().and_then(|w| w.parse().ok()).unwrap_or(0),
            Some("FONT_DESCENT") => face.descent = words.next().and_then(|w| w.parse().ok()).unwrap_or(0),
            Some("STARTCHAR") => {
                let mut code = None;
                let mut advance = bounding.0;
                let (mut width, mut height, mut x, mut y) = bounding;
                let mut bits = Vec::new();
                while let Some(line) = lines.next() {
                    let mut words = line.split_whitespace();
                    match words.next() {
                        Some("ENCODING") => code = words.next().and_then(|w| w.parse::<i64>().ok()),
                        Some("DWIDTH") => advance = words.next().and_then(|w| w.parse().ok()).unwrap_or(advance),
                        Some("BBX") => {
                            let n: Vec<i32> = words.filter_map(|w| w.parse().ok()).collect();
                            if n.len() == 4 {
                                (width, height, x, y) = (n[0], n[1], n[2], n[3]);
                            }
                        }
                        Some("BITMAP") => {
                            for row in lines.by_ref() {
                                if row.starts_with("ENDCHAR") {
                                    break;
                                }
                                let value = u64::from_str_radix(row.trim(), 16).unwrap_or(0);
                                let span = row.trim().len() as i32 * 4;
                                for column in 0..width {
                                    bits.push(value >> (span - 1 - column) & 1 == 1);
                                }
                            }
                            break;
                        }
                        _ => {}
                    }
                }
                if let Some(code) = code.filter(|code| *code >= 0) {
                    let glyph = Glyph { advance, width, height, left: x, top: y + height, bits };
                    face.glyphs.insert(code as u32, glyph);
                }
            }
            _ => {}
        }
    }
    if face.ascent == 0 {
        face.ascent = bounding.1 + bounding.3;
        face.descent = -bounding.3;
    }
    face
}

/// Unifont's `.hex`: `CODEPOINT:BITS`, 32 hex digits for an 8x16 glyph and 64
/// for a 16x16 one. Loaded the first time a face lacks a character.
fn parse_hex(text: &str) -> HashMap<u32, Glyph> {
    let mut glyphs = HashMap::new();
    for line in text.lines() {
        let Some((code, hex)) = line.split_once(':') else {
            continue;
        };
        let Ok(code) = u32::from_str_radix(code, 16) else {
            continue;
        };
        let width = if hex.len() == 64 { 16 } else { 8 };
        let mut bits = Vec::with_capacity(width * 16);
        for row in hex.as_bytes().chunks(width / 4) {
            let value = u32::from_str_radix(std::str::from_utf8(row).unwrap_or("0"), 16).unwrap_or(0);
            for column in 0..width {
                bits.push(value >> (width - 1 - column) & 1 == 1);
            }
        }
        // Unifont sits two pixels of its sixteen below the baseline.
        let glyph = Glyph { advance: width as i32, width: width as i32, height: 16, left: 0, top: 14, bits };
        glyphs.insert(code, glyph);
    }
    glyphs
}

static UNIFONT: OnceLock<HashMap<u32, Glyph>> = OnceLock::new();

fn unifont() -> &'static HashMap<u32, Glyph> {
    UNIFONT.get_or_init(|| {
        // The Basic Multilingual Plane, then the planes above it, where the
        // emoji are.
        let files = [
            option_env!("SMALTA_UNIFONT").map(str::to_owned).or_else(|| std::env::var("SMALTA_UNIFONT").ok()),
            option_env!("SMALTA_UNIFONT_UPPER")
                .map(str::to_owned)
                .or_else(|| std::env::var("SMALTA_UNIFONT_UPPER").ok()),
        ];
        if files[0].is_none() {
            log::warn!("no Unifont known; characters outside the main font will be boxes");
        }
        let mut glyphs = HashMap::new();
        for path in files.into_iter().flatten() {
            match std::fs::read_to_string(&path) {
                Ok(text) => glyphs.extend(parse_hex(&text)),
                Err(error) => log::warn!("{path}: {error}"),
            }
        }
        glyphs
    })
}

/// The glyph for `c`: the face's own, Unifont's, or the face's replacement box.
pub fn glyph(face: &Face, c: char) -> Option<&Glyph> {
    if invisible(c) {
        return None;
    }
    face.glyphs
        .get(&(c as u32))
        .or_else(|| unifont().get(&(c as u32)))
        .or_else(|| face.glyphs.get(&0xfffd))
        .or_else(|| face.glyphs.get(&('?' as u32)))
}

/// A piece of text as it is drawn: a glyph, or a colour emoji that may
/// stand for several characters.
pub enum Piece<'a> {
    Glyph(&'a Glyph),
    Emoji(Arc<Pixmap>),
}

impl Piece<'_> {
    pub fn advance(&self) -> i32 {
        match self {
            Piece::Glyph(glyph) => glyph.advance,
            Piece::Emoji(_) => crate::emoji::SIZE,
        }
    }
}

/// Unifont's glyphs, and so the emoji drawn in their place, stand this
/// far above the baseline.
pub const EMOJI_TOP: i32 = 14;

pub fn pieces<'a>(face: &'a Face, text: &str) -> Vec<Piece<'a>> {
    let chars: Vec<char> = text.chars().collect();
    let mut pieces = Vec::with_capacity(chars.len());
    let mut i = 0;
    while i < chars.len() {
        if let Some((len, image)) = crate::emoji::at(&chars[i..]) {
            pieces.push(Piece::Emoji(image));
            i += len;
            continue;
        }
        if let Some(glyph) = glyph(face, chars[i]) {
            pieces.push(Piece::Glyph(glyph));
        }
        i += 1;
    }
    pieces
}

pub fn text_width(face: &Face, text: &str) -> i32 {
    pieces(face, text).iter().map(Piece::advance).sum()
}

/// Default face: what the config names, else the Spleen baked in at build time.
pub fn load_default(configured: Option<&str>) -> Face {
    let candidates = [
        configured.map(str::to_owned),
        std::env::var("SMALTA_FONT").ok(),
        option_env!("SMALTA_FONT").map(str::to_owned),
    ];
    for path in candidates.into_iter().flatten() {
        match Face::load_bdf(Path::new(&path)) {
            Ok(face) if !face.glyphs.is_empty() => return face,
            Ok(_) => log::error!("{path}: no glyphs"),
            Err(error) => log::error!("{path}: {error}"),
        }
    }
    log::error!("no BDF font; text will not be drawn");
    Face { ascent: 12, descent: 4, glyphs: HashMap::new() }
}

/// Characters that only steer the ones around them: the marks and isolates
/// of text direction that apps wrap names and links in, the soft hyphen,
/// the variation selectors after an emoji, the joiner inside one, skin
/// tones, tags. Unifont draws them as dashed boxes so a font editor can see
/// them; text should not.
fn invisible(c: char) -> bool {
    matches!(
        c as u32,
        0x00ad | 0x061c | 0x180e | 0x200b..=0x200f | 0x202a..=0x202e | 0x2060..=0x206f | 0xfe00..=0xfe0f | 0xfeff | 0x1f3fb..=0x1f3ff | 0xe0000..=0xe007f | 0xe0100..=0xe01ef
    )
}
