//! The colour picker: freeze the screen, show it with a magnifier, copy the
//! colour clicked.
//!
//! The screen comes from wlr-screencopy into shared memory -- no grim, and
//! no image file to decode. Reading the pixel from the frozen copy rather than
//! the live screen is what makes the colour the one under the cursor at the
//! moment of the click.

use smithay_client_toolkit::dispatch2::Dispatch2;
use smithay_client_toolkit::reexports::client::{
    Connection, QueueHandle, WEnum,
    globals::GlobalList,
    protocol::{wl_output, wl_shm},
};
use smithay_client_toolkit::reexports::protocols_wlr::screencopy::v1::client::{
    zwlr_screencopy_frame_v1::{self, ZwlrScreencopyFrameV1},
    zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1,
};
use smithay_client_toolkit::shm::slot::{Buffer, SlotPool};

use crate::App;
use crate::canvas::{Canvas, Color, Rect};
use crate::theme::Theme;

pub struct ManagerData;
/// Which output a frame is of, to find its capture among several.
pub struct FrameData(pub String, pub Purpose);

/// What a frozen screen is wanted for.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Purpose {
    /// The colour picker, over it.
    Picker,
    /// Previews of the windows on it, for the switcher.
    Switcher,
    /// A screenshot, cut from it.
    Shot,
}

impl Dispatch2<ZwlrScreencopyManagerV1, App> for ManagerData {
    fn event(
        &self,
        _: &mut App,
        _: &ZwlrScreencopyManagerV1,
        _: <ZwlrScreencopyManagerV1 as smithay_client_toolkit::reexports::client::Proxy>::Event,
        _: &Connection,
        _: &QueueHandle<App>,
    ) {
    }
}

pub fn bind(globals: &GlobalList, qh: &QueueHandle<App>) -> Option<ZwlrScreencopyManagerV1> {
    globals.bind(qh, 1..=3, ManagerData).ok()
}

/// A frozen output, as premultiplied ARGB in device pixels.
pub struct Capture {
    pub output: String,
    pub width: i32,
    pub height: i32,
    pub pixels: Vec<u32>,
}

/// A capture under way.
pub struct Pending {
    pub output: String,
    pub purpose: Purpose,
    frame: ZwlrScreencopyFrameV1,
    format: Option<(wl_shm::Format, i32, i32, i32)>,
    buffer: Option<Buffer>,
    pool: SlotPool,
    y_invert: bool,
    /// Red and blue arrive swapped.
    swapped: bool,
}

impl Pending {
    pub fn start(
        manager: &ZwlrScreencopyManagerV1,
        output: &wl_output::WlOutput,
        name: String,
        purpose: Purpose,
        pool: SlotPool,
        qh: &QueueHandle<App>,
    ) -> Self {
        let frame = manager.capture_output(0, output, qh, FrameData(name.clone(), purpose));
        Pending { output: name, purpose, frame, format: None, buffer: None, pool, y_invert: false, swapped: false }
    }
}

/// Where a capture has got to.
pub enum Progress {
    Waiting,
    Done(Capture),
    Failed,
}

impl Dispatch2<ZwlrScreencopyFrameV1, App> for FrameData {
    fn event(
        &self,
        app: &mut App,
        _: &ZwlrScreencopyFrameV1,
        event: zwlr_screencopy_frame_v1::Event,
        _: &Connection,
        _: &QueueHandle<App>,
    ) {
        let Some(index) = app.picking.iter().position(|p| p.output == self.0 && p.purpose == self.1) else { return };
        let capture = match advance(&mut app.picking[index], event) {
            Progress::Waiting => return,
            Progress::Done(capture) => Some(capture),
            Progress::Failed => {
                log::error!("{} could not be captured", self.0);
                None
            }
        };
        app.picking.remove(index);
        app.captured(self.1, &self.0, capture);
    }
}

fn advance(pending: &mut Pending, event: zwlr_screencopy_frame_v1::Event) -> Progress {
    use zwlr_screencopy_frame_v1::Event;
    match event {
        Event::Buffer { format: WEnum::Value(format), width, height, stride } => {
            // The first 8-bit format offered is the one to take.
            let usable = matches!(
                format,
                wl_shm::Format::Argb8888
                    | wl_shm::Format::Xrgb8888
                    | wl_shm::Format::Abgr8888
                    | wl_shm::Format::Xbgr8888
            );
            if usable && pending.format.is_none() {
                pending.format = Some((format, width as i32, height as i32, stride as i32));
                pending.swapped = matches!(format, wl_shm::Format::Abgr8888 | wl_shm::Format::Xbgr8888);
                // Before version 3 there is no `buffer_done`: the description
                // is complete as soon as it arrives.
                if smithay_client_toolkit::reexports::client::Proxy::version(&pending.frame) < 3 {
                    return copy(pending);
                }
            }
            Progress::Waiting
        }
        Event::BufferDone => copy(pending),
        Event::Flags { flags: WEnum::Value(flags) } => {
            pending.y_invert = flags.contains(zwlr_screencopy_frame_v1::Flags::YInvert);
            Progress::Waiting
        }
        Event::Ready { .. } => {
            let Some((_, width, height, stride)) = pending.format else { return Progress::Failed };
            let Some(bytes) = pending.buffer.as_ref().and_then(|b| b.canvas(&mut pending.pool)) else {
                return Progress::Failed;
            };
            let mut pixels = Vec::with_capacity((width * height) as usize);
            for row in 0..height {
                let source = if pending.y_invert { height - 1 - row } else { row };
                let line = &bytes[(source * stride) as usize..];
                for column in 0..width as usize {
                    let p = u32::from_le_bytes(line[column * 4..column * 4 + 4].try_into().unwrap());
                    let p =
                        if pending.swapped { (p & 0xff00ff00) | ((p >> 16) & 0xff) | ((p & 0xff) << 16) } else { p };
                    // The screen is opaque, whatever the format says about alpha.
                    pixels.push(p | 0xff00_0000);
                }
            }
            pending.frame.destroy();
            Progress::Done(Capture { output: pending.output.clone(), width, height, pixels })
        }
        Event::Failed => Progress::Failed,
        _ => Progress::Waiting,
    }
}

/// Allocates the buffer the compositor described and asks it to copy into it.
fn copy(pending: &mut Pending) -> Progress {
    let Some((format, width, height, stride)) = pending.format else { return Progress::Failed };
    match pending.pool.create_buffer(width, height, stride, format) {
        Ok((buffer, _)) => {
            pending.frame.copy(buffer.wl_buffer());
            pending.buffer = Some(buffer);
            Progress::Waiting
        }
        Err(_) => Progress::Failed,
    }
}

/// The colour at a device pixel.
pub fn at(capture: &Capture, x: i32, y: i32) -> Option<u32> {
    (x >= 0 && y >= 0 && x < capture.width && y < capture.height)
        .then(|| capture.pixels[(y * capture.width + x) as usize])
}

pub fn hex(color: u32) -> String {
    format!("#{:06x}", color & 0xffffff)
}

/// The frozen screen with a magnifier at the pointer. `scale` maps the
/// overlay's shell pixels to the capture's device pixels.
pub fn draw(
    canvas: &mut Canvas,
    capture: &Capture,
    theme: &Theme,
    face: &crate::font::Face,
    pointer: Option<(f64, f64)>,
    scale: f64,
) {
    canvas.blit(capture.width, capture.height, &capture.pixels);
    let Some((px, py)) = pointer else { return };
    let (cx, cy) = ((px * scale) as i32, (py * scale) as i32);
    // Nine by nine source pixels, eleven shell pixels each.
    let zoom = 11;
    let span = 9;
    let size = zoom * span;
    let x = (px as i32 + 24).min((capture.width as f64 / scale) as i32 - size - 8);
    let y = (py as i32 + 24).min((capture.height as f64 / scale) as i32 - size - 8 - 22);
    let frame = Rect::new(x - 1, y - 1, size + 2, size + 2 + 22);
    canvas.fill(Rect::new(frame.x + 3, frame.y + 3, frame.w, frame.h), Color(0x9900_0000));
    canvas.fill(frame, theme.bg);
    for row in 0..span {
        for column in 0..span {
            let color = at(capture, cx + column - span / 2, cy + row - span / 2).unwrap_or(0xff00_0000);
            canvas.fill(Rect::new(x + column * zoom, y + row * zoom, zoom, zoom), Color(color));
        }
    }
    // The pixel that would be picked, outlined.
    let middle = Rect::new(x + (span / 2) * zoom, y + (span / 2) * zoom, zoom, zoom);
    canvas.frame(middle, Color(0xffffffff));
    canvas.frame(middle.inset(1), Color(0xff000000));
    canvas.frame(frame, theme.border);
    if let Some(color) = at(capture, cx, cy) {
        canvas.fill(Rect::new(x + 4, y + size + 6, 12, 12), Color(color));
        canvas.text(face, x + 22, y + size + 4, &hex(color), theme.fg);
    }
}
