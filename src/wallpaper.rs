//! The wallpaper, in big pixels.
//!
//! The picture is cut to the screen's shape, brought down to one sample per
//! `cell` screen pixels, darkened if asked, tinted into the theme's tones if
//! asked, and reduced to a few shades -- dithered between them by a 4x4
//! Bayer matrix, or in flat bands. Drawn back up, each sample is a square
//! block: the picture becomes pixel art of itself.
//!
//! The accent can come from the picture too: the colour its most vivid parts
//! share, brought to a brightness that reads on the dark theme.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use crate::canvas::Color;
use crate::config::Wallpaper;
use crate::services::tray::Pixmap;
use crate::theme::Theme;

/// Thresholds for ordered dithering, a 4x4 Bayer matrix in sixteenths.
const BAYER: [[u32; 4]; 4] = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];

/// The least a picture is kept at along its longest side: a wide screen at
/// one pixel a cell. A larger screen keeps it larger, up to as it is.
const KEPT: i32 = 2560;

/// How long the picture is kept after it was last asked for. The screen
/// keeps what was drawn from it; only a moving slider needs it again soon.
const KEEP_FOR: Duration = Duration::from_secs(10);

/// The path, its modification time, the side it was cut to, and the picture.
type Kept = Option<(String, Option<SystemTime>, i32, Arc<Pixmap>)>;
static KEPT_IMAGE: Mutex<Kept> = Mutex::new(None);
/// When the picture was last asked for, and whether a thread is waiting to let it go.
static LAST_USE: Mutex<(Option<Instant>, bool)> = Mutex::new((None, false));

/// The picture at `path`, decoded once and kept until the file changes or
/// nobody has asked for it for [`KEEP_FOR`], shrunk to [`KEPT`] or to
/// `longest`, whichever is more. Every setting moved would otherwise decode
/// it again -- fifty megabytes passing through for a large JPEG, twice, for
/// the preview and the screen. Kept for good, it would hold a screen-sized
/// picture nothing draws from.
fn load(path: &str, longest: i32) -> Option<Arc<Pixmap>> {
    used();
    let modified = std::fs::metadata(path).and_then(|m| m.modified()).ok();
    let limit = longest.max(KEPT);
    let mut kept = KEPT_IMAGE.lock().ok()?;
    // Kept large enough, or kept whole because it is smaller than it was allowed.
    if let Some((p, m, cut, image)) = kept.as_ref()
        && p == path
        && *m == modified
        && (*cut >= limit || image.width.max(image.height) < *cut)
    {
        return Some(image.clone());
    }
    let data = std::fs::read(path).map_err(|e| log::warn!("wallpaper {path}: {e}")).ok()?;
    let Some(image) = crate::image::decode(&data) else {
        log::warn!("wallpaper {path}: not a PNG, JPEG or BMP this can read");
        return None;
    };
    drop(data);
    let side = image.width.max(image.height);
    let image = if side > limit {
        let (w, h) = (image.width * limit / side, image.height * limit / side);
        let all = (0.0, 0.0, image.width as f32, image.height as f32);
        crate::image::resample(&image, all, w.max(1), h.max(1))
    } else {
        image
    };
    let image = Arc::new(image);
    *kept = Some((path.to_owned(), modified, limit, image.clone()));
    Some(image)
}

/// The picture cut to fill `width` x `height` and sampled down to it.
fn fill(image: &Pixmap, width: i32, height: i32) -> Pixmap {
    let (iw, ih) = (image.width as f32, image.height as f32);
    let scale = (width as f32 / iw).max(height as f32 / ih);
    let (w, h) = (width as f32 / scale, height as f32 / scale);
    crate::image::resample(image, ((iw - w) / 2.0, (ih - h) / 2.0, w, h), width, height)
}

/// The wallpaper for a screen of `width` x `height` device pixels, one
/// sample per cell: draw each as a `cell`-pixel square.
pub fn render(settings: &Wallpaper, theme: &Theme, file: &str, width: i32, height: i32) -> Option<Pixmap> {
    let cell = settings.cell() as i32;
    let (w, h) = ((width + cell - 1) / cell, (height + cell - 1) / cell);
    let mut image = fill(&*load(file, w.max(h))?, w.max(1), h.max(1));
    let levels = settings.levels.clamp(2, 32);
    let dim = settings.dim.clamp(0.0, 0.9);
    let ramp = match settings.tint.as_str() {
        "mono" => Some([theme.bg, mid(theme.bg, theme.fg), theme.fg]),
        "duotone" => Some([theme.bg, theme.accent, theme.fg]),
        _ => None,
    };
    for y in 0..image.height {
        for x in 0..image.width {
            let at = (y * image.width + x) as usize;
            let [_, r, g, b] = image.pixels[at].to_be_bytes();
            let darker = |c: u8| c as f32 * (1.0 - dim);
            let (r, g, b) = (darker(r), darker(g), darker(b));
            let shade = |v: f32| {
                // Not pixelated: every value kept, nothing to dither between.
                if !settings.pixelate {
                    return v.round().clamp(0.0, 255.0) as u32;
                }
                // Where between two shades a value falls, against this cell's threshold.
                let threshold =
                    if settings.dither { (BAYER[(y % 4) as usize][(x % 4) as usize] as f32 + 0.5) / 16.0 } else { 0.5 };
                let steps = (levels - 1) as f32;
                ((v / 255.0 * steps + threshold - 0.5).round().clamp(0.0, steps) / steps * 255.0) as u32
            };
            let out = match ramp {
                // Tinted: the brightness, in a few shades, along the ramp.
                Some(ramp) => along(ramp, shade(r * 0.299 + g * 0.587 + b * 0.114)),
                None => (shade(r) << 16) | (shade(g) << 8) | shade(b),
            };
            image.pixels[at] = 0xff00_0000 | out;
        }
    }
    Some(image)
}

fn mid(a: Color, b: Color) -> Color {
    let channel = |shift: u32| ((((a.0 >> shift) & 0xff) + ((b.0 >> shift) & 0xff)) / 2) << shift;
    Color(0xff00_0000 | channel(16) | channel(8) | channel(0))
}

/// A brightness, 0 to 255, as a colour on a ramp of three.
fn along(ramp: [Color; 3], light: u32) -> u32 {
    let (from, to, t) = if light < 128 { (ramp[0], ramp[1], light * 2) } else { (ramp[1], ramp[2], (light - 128) * 2) };
    let t = t.min(255);
    let mix = |shift: u32| ((((from.0 >> shift) & 0xff) * (255 - t) + ((to.0 >> shift) & 0xff) * t) / 255) << shift;
    mix(16) | mix(8) | mix(0)
}

/// The picture's own accent, `#rrggbb`: the hue its vivid parts share most,
/// at their average colour, lifted to read against the dark theme.
pub fn accent(file: &str) -> Option<String> {
    let image = fill(&*load(file, 0)?, 96, 54);
    // Hue in 24 slices; each pixel counts as much as it is vivid.
    let mut weight = [0f32; 24];
    let mut sum = [[0f32; 3]; 24];
    for pixel in &image.pixels {
        let [_, r, g, b] = pixel.to_be_bytes();
        let (r, g, b) = (r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0);
        let (max, min) = (r.max(g).max(b), r.min(g).min(b));
        let saturation = if max > 0.0 { (max - min) / max } else { 0.0 };
        if max < 0.2 || saturation < 0.25 {
            continue;
        }
        let hue = if max == r {
            ((g - b) / (max - min)).rem_euclid(6.0)
        } else if max == g {
            (b - r) / (max - min) + 2.0
        } else {
            (r - g) / (max - min) + 4.0
        };
        let slice = ((hue / 6.0 * 24.0) as usize).min(23);
        let w = saturation * max;
        weight[slice] += w;
        for (s, c) in sum[slice].iter_mut().zip([r, g, b]) {
            *s += c * w;
        }
    }
    let best = (0..24).max_by(|a, b| weight[*a].total_cmp(&weight[*b]))?;
    if weight[best] <= 0.0 {
        return None;
    }
    let [r, g, b] = sum[best].map(|s| s / weight[best]);
    // Its brightest channel at 0.85: an accent, not a stain.
    let lift = 0.85 / r.max(g).max(b).max(0.01);
    let byte = |c: f32| ((c * lift).clamp(0.0, 1.0) * 255.0) as u8;
    Some(format!("#{:02x}{:02x}{:02x}", byte(r), byte(g), byte(b)))
}

/// Each channel of an opaque picture brought down to `levels` shades,
/// dithered between them by the same Bayer matrix: a screenshot made into
/// pixel art the way the wallpaper is.
pub fn posterize(image: &mut Pixmap, levels: u32) {
    let steps = (levels.clamp(2, 32) - 1) as f32;
    for y in 0..image.height {
        for x in 0..image.width {
            let at = (y * image.width + x) as usize;
            let threshold = (BAYER[(y % 4) as usize][(x % 4) as usize] as f32 + 0.5) / 16.0;
            let shade =
                |v: u8| ((v as f32 / 255.0 * steps + threshold - 0.5).round().clamp(0.0, steps) / steps * 255.0) as u32;
            let [_, r, g, b] = image.pixels[at].to_be_bytes();
            image.pixels[at] = 0xff00_0000 | (shade(r) << 16) | (shade(g) << 8) | shade(b);
        }
    }
}

/// Notes a use, and starts a thread to let the picture go once it is idle.
fn used() {
    let Ok(mut last) = LAST_USE.lock() else { return };
    last.0 = Some(Instant::now());
    if std::mem::replace(&mut last.1, true) {
        return;
    }
    std::thread::spawn(|| {
        loop {
            std::thread::sleep(KEEP_FOR);
            let Ok(mut last) = LAST_USE.lock() else { return };
            if last.0.is_none_or(|t| t.elapsed() >= KEEP_FOR) {
                if let Ok(mut kept) = KEPT_IMAGE.lock() {
                    *kept = None;
                }
                last.1 = false;
                return;
            }
        }
    });
}
