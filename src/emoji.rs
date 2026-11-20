//! Colour emoji, from Noto Color Emoji's PNGs and its flags' SVGs, brought
//! down to the font's size.
//!
//! Noto keeps one 128-pixel PNG per emoji, sequences included, named by
//! code points: `emoji_u1f44d_1f3fb.png` is a thumbs-up with a skin tone.
//! Each is shrunk to [`SIZE`] the first time it is drawn and cut to whole
//! pixels, the way app icons are, so it sits in a line of the bitmap font.
//! Without the PNGs, or for an emoji they lack, Unifont's black-and-white
//! glyph is drawn instead.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use crate::services::tray::Pixmap;

/// Square, in shell pixels: Unifont's height.
pub const SIZE: i32 = 16;

const ZWJ: char = '\u{200d}';
const TEXT_STYLE: char = '\u{fe0e}';
const EMOJI_STYLE: char = '\u{fe0f}';

fn dir() -> Option<&'static PathBuf> {
    static DIR: OnceLock<Option<PathBuf>> = OnceLock::new();
    DIR.get_or_init(|| {
        option_env!("SMALTA_EMOJI")
            .map(str::to_owned)
            .or_else(|| std::env::var("SMALTA_EMOJI").ok())
            .map(PathBuf::from)
            .filter(|d| d.is_dir())
    })
    .as_ref()
}

/// Shrunk images by file name; `None` for a sequence with no file.
fn cache() -> &'static Mutex<HashMap<String, Option<Arc<Pixmap>>>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Option<Arc<Pixmap>>>>> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

fn load(codes: &[char]) -> Option<Arc<Pixmap>> {
    // Noto leaves the emoji-style selector out of its names.
    let name =
        codes.iter().filter(|&&c| c != EMOJI_STYLE).map(|&c| format!("{:x}", c as u32)).collect::<Vec<_>>().join("_");
    let mut cache = cache().lock().ok()?;
    cache
        .entry(name)
        .or_insert_with_key(|name| {
            let file = dir()?.join(format!("emoji_u{name}.png"));
            let small = match std::fs::read(&file) {
                Ok(data) => {
                    let image = crate::image::decode_png(&data)?;
                    let side = image.width.max(image.height) as f32;
                    let (x, y) = ((image.width as f32 - side) / 2.0, (image.height as f32 - side) / 2.0);
                    crate::image::resample(&image, (x, y, side, side), SIZE, SIZE)
                }
                // Flags come as SVG.
                Err(_) => crate::appicons::render_svg(&std::fs::read(file.with_extension("svg")).ok()?, SIZE)?,
            };
            Some(Arc::new(crate::image::crisp(small)))
        })
        .clone()
}

/// Whether `c` starts an emoji drawn in colour by default. Below U+2600
/// the emoji-looking characters are arrows, ©, ™ and the like, which read
/// as text unless an emoji selector asks otherwise.
fn emoji_by_default(c: char) -> bool {
    matches!(c as u32, 0x2600..=0x27bf | 0x2b00..=0x2bff | 0x1f000..=0x1faff)
}

fn modifier(c: char) -> bool {
    matches!(c as u32, 0x1f3fb..=0x1f3ff) || c == EMOJI_STYLE || matches!(c as u32, 0xe0020..=0xe007f)
}

fn regional(c: char) -> bool {
    matches!(c as u32, 0x1f1e6..=0x1f1ff)
}

/// The emoji starting at `chars[0]`, if it is one there is a picture for:
/// how many characters it spans, and the picture.
pub fn at(chars: &[char]) -> Option<(usize, Arc<Pixmap>)> {
    dir()?;
    let first = *chars.first()?;
    let selected = chars.get(1) == Some(&EMOJI_STYLE);
    if chars.get(1) == Some(&TEXT_STYLE) || !(emoji_by_default(first) || selected) {
        return None;
    }
    // The longest sequence the characters could make: a flag's two
    // letters, or the base with its modifiers and anything joined on.
    let mut end = 1;
    if regional(first) && chars.get(1).is_some_and(|&c| regional(c)) {
        end = 2;
    } else {
        loop {
            while chars.get(end).is_some_and(|&c| modifier(c)) {
                end += 1;
            }
            if chars.get(end) == Some(&ZWJ) && chars.get(end + 1).is_some() {
                end += 2;
            } else {
                break;
            }
        }
    }
    // Not every sequence has a picture of its own: try shorter ones down to
    // the base alone, which takes the modifiers it cannot show along with it.
    for len in (1..=end).rev() {
        if let Some(image) = load(&chars[..len]) {
            return Some((if len == 1 { end.min(1 + count_modifiers(&chars[1..])) } else { len }, image));
        }
    }
    None
}

fn count_modifiers(chars: &[char]) -> usize {
    chars.iter().take_while(|&&c| modifier(c)).count()
}
