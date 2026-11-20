//! Raster images: decoding the formats the shell meets, and resampling.
//!
//! PNG is what icons and most copied images are; BMP is what some programs
//! put on the clipboard for a screenshot; JPEG is what wallpapers mostly are.
//! Anything else stays undecoded.

use std::io::{BufReader, Cursor};

use crate::services::tray::Pixmap;

/// PNG, BMP or JPEG, by their magic numbers.
pub fn decode(data: &[u8]) -> Option<Pixmap> {
    if data.starts_with(b"\x89PNG") {
        decode_png(data)
    } else if data.starts_with(b"BM") {
        decode_bmp(data)
    } else if data.starts_with(&[0xff, 0xd8]) {
        decode_jpeg(data)
    } else {
        None
    }
}

fn premultiplied(r: u8, g: u8, b: u8, a: u8) -> u32 {
    let pre = |c: u8| (c as u32 * a as u32 / 255) as u8;
    u32::from_be_bytes([a, pre(r), pre(g), pre(b)])
}

pub fn decode_png(data: &[u8]) -> Option<Pixmap> {
    let mut decoder = png::Decoder::new(BufReader::new(Cursor::new(data)));
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().ok()?;
    let mut buffer = vec![0; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut buffer).ok()?;
    let bytes = &buffer[..info.buffer_size()];
    let pixels: Vec<u32> = match info.color_type {
        png::ColorType::Rgba => {
            bytes.as_chunks::<4>().0.iter().map(|[r, g, b, a]| premultiplied(*r, *g, *b, *a)).collect()
        }
        png::ColorType::Rgb => {
            bytes.as_chunks::<3>().0.iter().map(|[r, g, b]| premultiplied(*r, *g, *b, 255)).collect()
        }
        png::ColorType::GrayscaleAlpha => {
            bytes.as_chunks::<2>().0.iter().map(|[v, a]| premultiplied(*v, *v, *v, *a)).collect()
        }
        png::ColorType::Grayscale => bytes.iter().map(|v| premultiplied(*v, *v, *v, 255)).collect(),
        png::ColorType::Indexed => return None,
    };
    Some(Pixmap { width: info.width as i32, height: info.height as i32, pixels })
}

/// A JPEG, which has no alpha: every pixel opaque.
fn decode_jpeg(data: &[u8]) -> Option<Pixmap> {
    use zune_jpeg::zune_core::{bytestream::ZCursor, colorspace::ColorSpace, options::DecoderOptions};
    let options = DecoderOptions::default().jpeg_set_out_colorspace(ColorSpace::RGB);
    let mut decoder = zune_jpeg::JpegDecoder::new_with_options(ZCursor::new(data), options);
    let bytes = decoder.decode().ok()?;
    let (width, height) = decoder.dimensions()?;
    let pixels = bytes.as_chunks::<3>().0.iter().map(|[r, g, b]| premultiplied(*r, *g, *b, 255)).collect();
    Some(Pixmap { width: width as i32, height: height as i32, pixels })
}

/// Uncompressed 24- and 32-bit BMP, the kinds a screenshot comes as.
fn decode_bmp(data: &[u8]) -> Option<Pixmap> {
    let u16_at = |at: usize| data.get(at..at + 2).map(|b| u16::from_le_bytes([b[0], b[1]]));
    let u32_at = |at: usize| data.get(at..at + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
    let offset = u32_at(10)? as usize;
    let width = u32_at(18)? as i32;
    let raw_height = u32_at(22)? as i32;
    let bits = u16_at(28)?;
    let compression = u32_at(30)?;
    // 0 is plain, 3 is bit fields, which for 32 bits is BGRA in practice.
    if width <= 0 || raw_height == 0 || !(compression == 0 || compression == 3) || !(bits == 24 || bits == 32) {
        return None;
    }
    let height = raw_height.abs();
    let bytes_per = bits as usize / 8;
    let stride = (width as usize * bytes_per).div_ceil(4) * 4;
    // A header that leaves alpha out means every pixel is opaque, whatever
    // the fourth byte says.
    let alpha = bits == 32 && compression == 3;
    let mut pixels = Vec::with_capacity((width * height) as usize);
    for row in 0..height as usize {
        // Positive height is stored bottom row first.
        let source = if raw_height > 0 { height as usize - 1 - row } else { row };
        let line = data.get(offset + source * stride..offset + source * stride + width as usize * bytes_per)?;
        for p in line.chunks_exact(bytes_per) {
            let a = if alpha { p[3] } else { 255 };
            pixels.push(premultiplied(p[2], p[1], p[0], a));
        }
    }
    Some(Pixmap { width, height, pixels })
}

/// The part of `image` at `(x, y)` sized `w` x `h` source pixels — which may
/// reach past its edges, where it is transparent — averaged down to
/// `width` x `height`. Nearest-neighbour from a screenshot to a thumbnail
/// keeps one pixel in a hundred; averaging keeps the picture.
pub fn resample(image: &Pixmap, (x, y, w, h): (f32, f32, f32, f32), width: i32, height: i32) -> Pixmap {
    let mut pixels = vec![0u32; (width * height).max(0) as usize];
    let (step_x, step_y) = (w / width as f32, h / height as f32);
    for ty in 0..height {
        let y0 = (y + ty as f32 * step_y).floor() as i32;
        let y1 = ((y + (ty + 1) as f32 * step_y).floor() as i32).max(y0 + 1);
        for tx in 0..width {
            let x0 = (x + tx as f32 * step_x).floor() as i32;
            let x1 = ((x + (tx + 1) as f32 * step_x).floor() as i32).max(x0 + 1);
            let mut sum = [0u32; 4];
            let mut count = 0;
            // Sample at most 8x8 points per pixel: from a big image, a
            // full average costs more than it shows.
            let (sx_step, sy_step) = (((x1 - x0) / 8).max(1), ((y1 - y0) / 8).max(1));
            for sy in (y0..y1).step_by(sy_step as usize) {
                for sx in (x0..x1).step_by(sx_step as usize) {
                    count += 1;
                    if sx < 0 || sy < 0 || sx >= image.width || sy >= image.height {
                        continue;
                    }
                    let p = image.pixels[(sy * image.width + sx) as usize].to_be_bytes();
                    for (s, c) in sum.iter_mut().zip(p) {
                        *s += c as u32;
                    }
                }
            }
            let [a, r, g, b] = sum.map(|s| (s / count.max(1)) as u8);
            pixels[(ty * width + tx) as usize] = u32::from_be_bytes([a, r, g, b]);
        }
    }
    Pixmap { width, height, pixels }
}

/// `image` filling `width` x `height`, cut to the middle where the shapes
/// differ. One that fits already is left as it is, not blown up.
pub fn cover(image: &Pixmap, width: i32, height: i32) -> Pixmap {
    let (iw, ih) = (image.width as f32, image.height as f32);
    if image.width <= width && image.height <= height {
        return image.clone();
    }
    let scale = (width as f32 / iw).max(height as f32 / ih).min(1.0);
    let (w, h) = ((width as f32 / scale).min(iw), (height as f32 / scale).min(ih));
    let (out_w, out_h) = ((w * scale).round() as i32, (h * scale).round() as i32);
    resample(image, ((iw - w) / 2.0, (ih - h) / 2.0, w, h), out_w.max(1), out_h.max(1))
}

/// Every pixel either solid or gone, its colour kept.
pub fn crisp(mut image: Pixmap) -> Pixmap {
    for pixel in &mut image.pixels {
        let [a, r, g, b] = pixel.to_be_bytes();
        *pixel = if a < 128 {
            0
        } else {
            let full = |c: u8| (c as u32 * 255 / a as u32).min(255) as u8;
            u32::from_be_bytes([255, full(r), full(g), full(b)])
        };
    }
    image
}
