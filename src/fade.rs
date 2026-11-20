//! Changes of theme and wallpaper, dissolved in over half a second by an
//! 8x8 Bayer matrix in big blocks, rather than cross-faded: each block flips
//! from the old picture to the new at its own moment, the order the matrix
//! gives, so the new one comes through as a growing dither pattern.

use std::time::{Duration, Instant};

pub const LENGTH: Duration = Duration::from_millis(480);
/// Between frames.
pub const FRAME: Duration = Duration::from_millis(30);
/// A block's side, in shell pixels.
const BLOCK: f32 = 4.0;

/// Where in the 8x8 Bayer order the cell `x, y` comes, 0 to 63: the bits of
/// `x ^ y` and `y`, interleaved and read from the lowest up.
fn order(x: u32, y: u32) -> u32 {
    let mut value = 0;
    for bit in 0..3 {
        let (xb, yb) = ((x >> bit) & 1, (y >> bit) & 1);
        value |= ((xb ^ yb) << (2 * (2 - bit) + 1)) | (yb << (2 * (2 - bit)));
    }
    value
}

/// How far along a fade begun at `start` is, 0 to 1.
pub fn progress(start: Instant) -> f32 {
    (start.elapsed().as_secs_f32() / LENGTH.as_secs_f32()).min(1.0)
}

/// `pixels`, `width` device pixels wide, with the blocks whose turn has not
/// come yet put back as they were in `old`. `scale` is device pixels to a
/// shell pixel.
pub fn dissolve(pixels: &mut [u8], old: &[u8], width: i32, scale: f32, progress: f32) {
    if pixels.len() != old.len() || width <= 0 {
        return;
    }
    let block = (BLOCK * scale).round().max(1.0) as usize;
    let shown = (progress * 64.0) as u32;
    let row_bytes = width as usize * 4;
    for (y, (row, before)) in pixels.chunks_mut(row_bytes).zip(old.chunks(row_bytes)).enumerate() {
        let by = ((y / block) % 8) as u32;
        for (bx, (run, was)) in row.chunks_mut(block * 4).zip(before.chunks(block * 4)).enumerate() {
            if order((bx % 8) as u32, by) >= shown {
                run.copy_from_slice(was);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_place_once() {
        let mut seen = [false; 64];
        for y in 0..8 {
            for x in 0..8 {
                seen[order(x, y) as usize] = true;
            }
        }
        assert!(seen.iter().all(|s| *s));
        // Neighbours as far apart in the order as can be: the 2x2 matrix, scaled.
        assert_eq!([order(0, 0), order(1, 0), order(0, 1), order(1, 1)], [0, 32, 48, 16]);
        // The first row of the classic matrix.
        assert_eq!((0..8).map(|x| order(x, 0)).collect::<Vec<_>>(), [0, 32, 8, 40, 2, 34, 10, 42]);
    }
}
