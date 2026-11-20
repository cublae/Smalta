//! Smalta: a pixel desktop shell for niri and Hyprland.

mod appicons;
mod apps;
mod bar;
mod calc;
mod canvas;
mod clipboard;
mod compositor;
mod config;
mod convert;
mod desktop;
mod emoji;
mod fade;
mod files;
mod font;
mod i18n;
mod icons;
mod image;
mod ipc;
mod keyboard;
mod notifications;
mod picker;
mod popups;
mod recorder;
mod resources;
mod services;
mod shot;
mod theme;
mod ui;
mod viewport;
mod visualizer;
mod wallpaper;

use std::os::unix::net::UnixListener;
use std::sync::mpsc;
use std::time::Duration;

use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState},
    delegate_registry,
    output::{OutputHandler, OutputState},
    reexports::{
        calloop::{
            EventLoop, Interest, LoopHandle, Mode, PostAction,
            channel::{self, Event},
            generic::Generic,
            timer::{TimeoutAction, Timer},
        },
        calloop_wayland_source::WaylandSource,
        client::{
            Connection, QueueHandle,
            globals::registry_queue_init,
            protocol::{wl_output, wl_pointer, wl_seat, wl_shm, wl_surface},
        },
        protocols::wp::cursor_shape::v1::client::wp_cursor_shape_device_v1::{Shape, WpCursorShapeDeviceV1},
    },
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    seat::{
        Capability, SeatHandler, SeatState,
        pointer::{PointerEvent, PointerEventKind, PointerHandler, cursor_shape::CursorShapeManager},
    },
    shell::{
        WaylandSurface,
        wlr_layer::{
            Anchor, KeyboardInteractivity, Layer, LayerShell, LayerShellHandler, LayerSurface, LayerSurfaceConfigure,
        },
    },
    shm::{Shm, ShmHandler, slot::SlotPool},
};

use crate::bar::{Action, Context, Hit};
use crate::canvas::{Canvas, Color, Rect};
use crate::compositor::{Command, Snapshot};
use crate::config::Config;
use crate::font::Face;
use crate::i18n::tr;
use crate::keyboard::Keyboard;
use crate::popups::{Ctx, Kind, Outcome, Placement, Popups, Request};
use crate::recorder::Recorder;
use crate::resources::Resources;
use crate::services::{Handles, Services};
use crate::theme::Theme;
use crate::ui::{Align, Input, Key, Ui};

const BTN_LEFT: u32 = 0x110;
const BTN_MIDDLE: u32 = 0x112;

/// Gap between a popup and the bar or the screen edge.
const POPUP_MARGIN: i32 = 8;

#[derive(Clone, Copy, PartialEq, Debug)]
enum Role {
    Bar,
    Popup(Kind),
    /// Notifications that just arrived, stacked by the bar.
    Toasts,
    /// Volume, brightness or layout, just after it changed.
    Osd,
    /// Widgets on the background layer of one output.
    Desktop,
    /// The frozen screen the colour picker shows.
    Picker,
    /// A frozen screen to cut a screenshot from.
    Shot,
    /// Transparent, under an open popup: a click on it closes the popup.
    Catcher,
    /// The shell's own wallpaper, under the desktop widgets.
    Wallpaper,
}

/// A floating popup's place and size on its output, in shell pixels.
#[derive(Clone, Copy, Debug)]
struct Geometry {
    x: i32,
    y: i32,
    w: i32,
    h: i32,
}

impl Geometry {
    const MIN_W: i32 = 420;
    const MIN_H: i32 = 220;

    /// At least its minimum size, and on the output: a strip of it always
    /// stays reachable to drag it back.
    fn clamped(self, output: (i32, i32)) -> Self {
        let w = self.w.clamp(Self::MIN_W, output.0.max(Self::MIN_W));
        let h = self.h.clamp(Self::MIN_H, output.1.max(Self::MIN_H));
        Self { x: self.x.clamp(0, (output.0 - 40).max(0)), y: self.y.clamp(0, (output.1 - 40).max(0)), w, h }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum DragKind {
    Move,
    Resize,
}

/// A floating popup being dragged by its title or its corner.
#[derive(Clone, Copy)]
struct Drag {
    kind: DragKind,
    /// Where the press was, on the surface the popup is drawn in.
    from: (f64, f64),
    start: Geometry,
}

struct Surface {
    role: Role,
    output: wl_output::WlOutput,
    output_name: String,
    layer: LayerSurface,
    pool: SlotPool,
    /// In the shell's own pixels, which are `pixel` screen pixels each.
    width: i32,
    height: i32,
    /// The size the compositor gave, in its logical pixels: the buffer is
    /// exactly this, times the output's scale, whatever the shell's pixel.
    logical: (i32, i32),
    /// The output's integer scale.
    scale: i32,
    configured: bool,
    input: Input,
    /// Bar buttons from the last draw.
    hits: Vec<Hit>,
    /// Clickable rectangles from a popup's last draw.
    hot: Vec<Rect>,
    workspaces: Option<Rect>,
    /// For a fade: draw the next picture into `old` too, the picture faded
    /// from, and the one faded to.
    keep: bool,
    old: Option<Vec<u8>>,
    new: Option<Vec<u8>>,
    /// For a surface shown stretched from a smaller buffer.
    viewport: Option<smithay_client_toolkit::reexports::protocols::wp::viewporter::client::wp_viewport::WpViewport>,
}

impl Drop for Surface {
    fn drop(&mut self) {
        // Before the surface it belongs to goes.
        if let Some(viewport) = self.viewport.take() {
            viewport.destroy();
        }
    }
}

pub struct App {
    registry_state: RegistryState,
    seat_state: SeatState,
    output_state: OutputState,
    compositor_state: CompositorState,
    layer_shell: LayerShell,
    shm: Shm,
    cursor_shapes: Option<CursorShapeManager>,
    pointer: Option<wl_pointer::WlPointer>,
    cursor: Option<WpCursorShapeDeviceV1>,
    /// The serial of the pointer's last entry, which a shape change has to quote.
    enter_serial: u32,
    keyboard: Keyboard,
    handle: LoopHandle<'static, App>,
    qh: QueueHandle<App>,

    surfaces: Vec<Surface>,
    open: Option<(Kind, String)>,
    /// Where along the bar the open popup points, for menus under their icon.
    anchor_x: Option<i32>,
    popups: Popups,

    config: Config,
    theme: Theme,
    face: Face,
    snapshot: Snapshot,
    resources: Resources,
    clock: String,
    commands: Option<mpsc::Sender<Command>>,
    services: Services,
    handles: Handles,
    recorder: Recorder,
    clipboard: clipboard::Clipboard,
    notifications: notifications::Notifications,
    /// Do not disturb: notifications are kept but not shown.
    dnd: bool,
    /// What the OSD shows, and the timer that hides it.
    osd: Option<(icons::Icon, Option<f32>, String)>,
    osd_timer: Option<smithay_client_toolkit::reexports::calloop::RegistrationToken>,
    /// Values change at startup before anyone touched anything; until
    /// this, the OSD keeps quiet.
    started: std::time::Instant,
    screencopy: Option<smithay_client_toolkit::reexports::protocols_wlr::screencopy::v1::client::zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1>,
    viewporter: Option<smithay_client_toolkit::reexports::protocols::wp::viewporter::client::wp_viewporter::WpViewporter>,
    /// Outputs being captured for the colour picker, and those captured.
    pub picking: Vec<picker::Pending>,
    capture: Vec<picker::Capture>,
    /// The screenshot being taken, if one is.
    shot: shot::Shot,
    /// cava, while something plays, and where its frames arrive.
    visualizer: visualizer::Visualizer,
    frames: channel::Sender<Vec<u8>>,
    /// When the theme or wallpaper last changed, while it is fading in.
    fading_since: Option<std::time::Instant>,
    /// The last id of a notification the shell made itself.
    local_id: u32,
    /// Screens are being frozen for the switcher, which opens once they are.
    switching: bool,
    /// Devices already warned about running low: Bluetooth ones by path, and `battery`.
    warned: Vec<String>,
    exit: bool,
    /// Start again once the loop ends: a new config is waiting.
    restart: bool,
    /// Surfaces to draw once the event loop is idle, and whether that is queued.
    dirty: Vec<wl_surface::WlSurface>,
    flush_queued: bool,
    /// Where the floating popup was left, kept for its next opening.
    floating: Option<Geometry>,
    drag: Option<Drag>,
}

impl App {
    /// Screen pixels to a shell pixel; whole or not.
    fn pixel(&self) -> f32 {
        pixel_of(&self.config)
    }

    fn bottom(&self) -> bool {
        self.config.bar.position != "top"
    }

    fn index_of(&self, surface: &wl_surface::WlSurface) -> Option<usize> {
        self.surfaces.iter().position(|s| s.layer.wl_surface() == surface)
    }

    fn draw(&mut self, index: usize) {
        let pixel = self.pixel();
        let fading = self.fading_since.map(fade::progress);
        let bottom = self.bottom();
        let recording = self.recorder.recording();
        let elapsed = recording.then(|| self.recorder.elapsed());
        let dnd = self.dnd;
        let mut requests = Vec::new();
        let Self {
            surfaces,
            config,
            theme,
            face,
            snapshot,
            resources,
            popups,
            services,
            handles,
            notifications,
            capture,
            shot,
            visualizer,
            floating,
            compositor_state,
            ..
        } = self;
        let unread = notifications.unread;
        let surface = &mut surfaces[index];
        if !surface.configured || surface.width <= 0 || surface.height <= 0 {
            return;
        }
        // The catcher only has to be there to be clicked: one transparent
        // pixel, stretched over the screen.
        if surface.role == Role::Catcher
            && let Some(viewport) = &surface.viewport
        {
            let Ok((buffer, pixels)) = surface.pool.create_buffer(1, 1, 4, wl_shm::Format::Argb8888) else { return };
            pixels.fill(0);
            viewport.set_destination(surface.logical.0, surface.logical.1);
            let wl = surface.layer.wl_surface();
            wl.set_buffer_scale(1);
            wl.damage_buffer(0, 0, 1, 1);
            if buffer.attach_to(wl).is_ok() {
                surface.layer.commit();
            }
            return;
        }
        let device = surface.scale as f32 * pixel;
        let (w, h) = (surface.logical.0 * surface.scale, surface.logical.1 * surface.scale);
        let Ok((buffer, pixels)) = surface.pool.create_buffer(w, h, w * 4, wl_shm::Format::Argb8888) else {
            log::error!("could not allocate a {w}x{h} buffer");
            return;
        };
        // Midway through a fade the new picture, drawn once, is copied back
        // rather than drawn again: a wallpaper is costly to make.
        let fading = fading.filter(|_| surface.old.is_some());
        let reuse = fading.is_some() && surface.new.as_ref().is_some_and(|new| new.len() == pixels.len());
        if let Some(new) = surface.new.as_deref().filter(|_| reuse) {
            pixels.copy_from_slice(new);
        }
        let mut canvas = Canvas::new(&mut *pixels, w, h, device);
        canvas.tint = config.theme.tint_icons.then_some([theme.bg, theme.accent, theme.fg]);
        let mut wanted_height = None;

        match surface.role {
            _ if reuse => {}
            Role::Bar => {
                let cx = Context {
                    config,
                    theme,
                    face,
                    snapshot,
                    resources,
                    output: &surface.output_name,
                    unread,
                    dnd,
                    services,
                    recording: elapsed.clone(),
                    levels: &visualizer.levels,
                };
                let pointer = surface.input.pointer;
                let frame = bar::draw(&mut canvas, surface.width, surface.height, bottom, &cx, pointer);
                surface.hits = frame.hits;
                surface.workspaces = frame.workspaces;
            }
            Role::Catcher => canvas.clear(Color(0)),
            Role::Wallpaper => {
                // Made afresh: it is drawn only when the screen or the settings
                // change, and a screen-sized copy is not worth keeping for that.
                // Pixelated, in cells of logical pixels; otherwise one sample
                // to each of the screen's own.
                let (scale, cell) = match config.wallpaper.pixelate {
                    true => (surface.scale, config.wallpaper.cell() as i32 * surface.scale),
                    false => (1, 1),
                };
                let image = config
                    .wallpaper
                    .file(&config.theme)
                    .and_then(|file| wallpaper::render(&config.wallpaper, theme, &file, w / scale, h / scale));
                match image {
                    Some(image) => canvas.blocks(&image, cell),
                    None => canvas.clear(theme.bg),
                }
            }
            Role::Picker => {
                if let Some(capture) = capture.iter().find(|c| c.output == surface.output_name) {
                    picker::draw(&mut canvas, capture, theme, face, surface.input.pointer, device as f64);
                }
            }
            Role::Shot => {
                // Out of the editor while it draws, which wants itself whole.
                let captures = std::mem::take(&mut shot.captures);
                if let Some(capture) = captures.iter().find(|c| c.output == surface.output_name) {
                    let ratio = capture.width as f32 / surface.width.max(1) as f32;
                    let size = (surface.width, surface.height);
                    let (hot, finish) = shot.draw(&mut canvas, &mut surface.input, capture, size, ratio, theme, face);
                    surface.hot = hot;
                    requests.extend(finish.map(Request::Shot));
                }
                shot.captures = captures;
            }
            Role::Desktop => {
                let d = desktop::Desktop { config, theme, face, resources, services };
                desktop::draw(&mut canvas, surface.width, surface.height, &d);
            }
            Role::Osd => {
                canvas.clear(Color(0));
                if let Some((icon, level, text)) = &self.osd {
                    let panel = Rect::new(0, 0, surface.width - popups::SHADOW, surface.height - popups::SHADOW);
                    canvas.fill(Rect::new(popups::SHADOW, popups::SHADOW, panel.w, panel.h), Color(0x9900_0000));
                    canvas.fill(panel, theme.bg);
                    canvas.frame(panel, theme.border);
                    let mut ui = Ui { canvas: &mut canvas, face, theme, input: &mut surface.input, hot: Vec::new() };
                    let inner = panel.inset(10);
                    let (cell, rest) = inner.take_left(12, 10);
                    ui.sprite(cell, icon, theme.fg);
                    match level {
                        Some(level) => {
                            let (label, track) = rest.take_right(ui.text_width("100%"), 8);
                            ui.text(label, &format!("{}%", (level * 100.0).round()), theme.dim, Align::Right);
                            ui.cells(Rect::new(track.x, track.y + (track.h - 8) / 2, track.w, 8), level.min(1.0), 3, 1);
                        }
                        None => ui.text(rest, text, theme.fg, Align::Left),
                    }
                }
            }
            Role::Toasts => {
                canvas.clear(Color(0));
                let mut ui = Ui { canvas: &mut canvas, face, theme, input: &mut surface.input, hot: Vec::new() };
                let mut y = 0;
                for n in notifications.toasts(&config.notifications) {
                    let (height, action) = notifications::card(&mut ui, Rect::new(0, y, surface.width, 1000), n);
                    if let Some(action) = action {
                        requests.push(match action {
                            notifications::CardAction::Dismiss => Request::Dismiss(n.data.id),
                            notifications::CardAction::Invoke(key) => Request::Invoke(n.data.id, key),
                        });
                    }
                    y += height + 6;
                }
                surface.hot = ui.hot;
                surface.input.click = None;
                let needed = (y - 6).max(1);
                if needed != surface.height {
                    wanted_height = Some(needed);
                }
            }
            // The whole screen, and nothing drawn for it: the popup is all of it.
            Role::Popup(kind) if kind.placement() == Placement::Screen => {
                canvas.clear(Color(0));
                let mut ui = Ui { canvas: &mut canvas, face, theme, input: &mut surface.input, hot: Vec::new() };
                let mut cx =
                    Ctx { services, handles, requests: &mut requests, dnd, recording, notifications, config, snapshot };
                popups.draw(kind, &mut ui, Rect::new(0, 0, surface.width, surface.height), &mut cx);
                surface.hot = ui.hot;
                surface.input.click = None;
            }
            Role::Popup(kind) => {
                canvas.clear(Color(0));
                let shadow = popups::SHADOW;
                // A floating popup is drawn where it was left inside a surface
                // that covers the screen and never moves, so moving it is only
                // drawing it again; only its own rectangle takes input.
                let (width, height) = match (kind.placement(), *floating) {
                    (Placement::Floating, Some(g)) => {
                        canvas.offset = (g.x, g.y);
                        if let Ok(region) = smithay_client_toolkit::compositor::Region::new(&*compositor_state) {
                            let logical = |v: i32| to_logical(v, pixel);
                            region.add(logical(g.x), logical(g.y), logical(g.w), logical(g.h));
                            surface.layer.wl_surface().set_input_region(Some(region.wl_region()));
                        }
                        (g.w, g.h)
                    }
                    _ => (surface.width, surface.height),
                };
                let panel = Rect::new(0, 0, width - shadow, height - shadow);
                // A hard shadow: the panel's own shape, offset and darkened.
                canvas.fill(Rect::new(shadow, shadow, panel.w, panel.h), Color(0x9900_0000));
                canvas.fill(panel, theme.bg);
                canvas.frame(panel, theme.border);
                let mut ui = Ui { canvas: &mut canvas, face, theme, input: &mut surface.input, hot: Vec::new() };
                let pad = popups::PADDING;
                // A popup with no title starts its content at the top.
                let top = if kind.title().is_empty() {
                    pad
                } else {
                    let title = Rect::new(pad, pad - 4, panel.w - pad * 2, ui.line());
                    ui.text(title, &kind.title().to_lowercase(), theme.dim, Align::Left);
                    title.bottom() + 8
                };
                // Unbounded below: the content says how tall it is, and the canvas clips
                // whatever does not fit until the surface has grown to match.
                // A side panel and a floating one have the height they were given.
                let fixed = matches!(kind.placement(), Placement::Side | Placement::Floating);
                let room = if fixed { panel.h - top - pad } else { 100_000 };
                let content = Rect::new(pad, top, panel.w - pad * 2, room);
                let mut cx =
                    Ctx { services, handles, requests: &mut requests, dnd, recording, notifications, config, snapshot };
                let used = popups.draw(kind, &mut ui, content, &mut cx);
                if kind.placement() == Placement::Floating {
                    // The corner to pull: three steps of dots.
                    for row in 0..3 {
                        for column in 0..=row {
                            let (x, y) = (panel.w - 4 - column * 3, panel.h - 4 - (row - column) * 3);
                            ui.fill(Rect::new(x, y, 1, 1), theme.dim);
                        }
                    }
                }
                surface.hot = ui.hot;
                surface.input.click = None;
                let needed = top + used + pad + shadow;
                if !fixed && needed != surface.height {
                    wanted_height = Some(needed);
                }
            }
        }

        if let Some(progress) = fading {
            if surface.new.is_none() {
                surface.new = Some(pixels.to_vec());
            }
            if let Some(old) = &surface.old {
                fade::dissolve(pixels, old, w, device, progress);
            }
        }
        // The picture as it was, kept to fade from.
        if std::mem::take(&mut surface.keep) {
            surface.old = Some(pixels.to_vec());
        }

        let wl = surface.layer.wl_surface();
        wl.set_buffer_scale(surface.scale);
        wl.damage_buffer(0, 0, w, h);
        if buffer.attach_to(wl).is_ok() {
            surface.layer.commit();
        }
        // The content decides the height; the compositor answers with a
        // configure, and the next draw fits.
        if let Some(height) = wanted_height {
            surface.layer.set_size(surface.logical.0 as u32, to_logical(height, pixel) as u32);
            surface.layer.commit();
        }
        self.handle_requests(requests);
    }

    fn handle_requests(&mut self, requests: Vec<Request>) {
        for request in requests {
            let systemctl = |verb: &str| services::launch(&["systemctl", verb]);
            match request {
                Request::Close => self.close(),
                Request::Show(kind) => self.show(kind, None),
                Request::ApplyConfig => self.apply_config(config::load()),
                Request::ToggleHidden => self.compositor(Command::ToggleHidden),
                Request::Settings(topic) => {
                    self.show(Kind::Settings, None);
                    self.popups.settings.go(topic);
                    self.redraw_popup();
                }
                Request::Mixer => {
                    self.show(Kind::ControlCenter, None);
                    self.popups.control.page = popups::control::Page::Mixer;
                    self.redraw_popup();
                }
                Request::PreviewOsd => {
                    self.started = std::time::Instant::now() - Duration::from_secs(10);
                    self.osd(icons::SPEAKER, Some(0.6), String::new());
                }
                Request::Spawn(argv) => self.compositor(Command::Spawn(argv)),
                Request::Copy(text) => clipboard::copy_bytes(text.as_bytes()),
                Request::FocusWindow(id) => self.compositor(Command::FocusWindow(id)),
                Request::Dismiss(id) => {
                    self.notifications.close(id, 2);
                    self.refresh_notifications();
                }
                Request::Invoke(id, key) => {
                    self.notifications.invoke(id, &key);
                    self.refresh_notifications();
                }
                Request::Tray(command) => drop(self.handles.tray.send(command)),
                Request::ClearNotifications => {
                    self.notifications.clear();
                    self.refresh_notifications();
                }
                Request::Screenshot => {
                    // Out of the way first, or it is in the picture: a popup
                    // the compositor fades out is given the time to go.
                    let open = self.open.is_some();
                    self.close();
                    let wait = Duration::from_millis(if open { 350 } else { 0 });
                    let _ = self.handle.insert_source(Timer::from_duration(wait), |_, _, app: &mut App| {
                        app.screenshot();
                        TimeoutAction::Drop
                    });
                }
                Request::Shot(finish) => self.finish_shot(finish),
                Request::PickColor => {
                    self.close();
                    self.pick_color();
                }
                Request::ToggleRecording => {
                    self.close();
                    self.recorder.toggle();
                }
                Request::ToggleDnd => {
                    self.dnd = !self.dnd;
                    if self.dnd {
                        for n in &mut self.notifications.list {
                            n.fresh &= n.data.urgency >= 2;
                        }
                    }
                    self.refresh_notifications();
                }
                Request::Lock => {
                    self.close();
                    services::launch(&["loginctl", "lock-session"]);
                }
                Request::LogOut => self.compositor(Command::Quit),
                Request::Suspend => {
                    self.close();
                    systemctl("suspend");
                }
                Request::Reboot => systemctl("reboot"),
                Request::PowerOff => systemctl("poweroff"),
            }
        }
    }

    /// Makes the toast surface match the notifications: created when the
    /// first arrives, gone with the last, redrawn in between. The bar
    /// redraws too, for its unread dot.
    fn refresh_notifications(&mut self) {
        let wanted = self.notifications.toasts(&self.config.notifications).next().is_some();
        let existing = self.surfaces.iter().position(|s| s.role == Role::Toasts);
        match (wanted, existing) {
            (false, Some(index)) => {
                self.surfaces.remove(index);
            }
            (true, None) => {
                let output_name = self.focused_output();
                let output = self
                    .surfaces
                    .iter()
                    .find(|s| s.role == Role::Bar && s.output_name == output_name)
                    .or_else(|| self.surfaces.iter().find(|s| s.role == Role::Bar))
                    .map(|s| s.output.clone());
                if let Some(output) = output {
                    let pixel = self.pixel();
                    let width = 340;
                    let layer = self.create_layer(Role::Toasts, &output, &output_name, "smalta-notifications");
                    layer.set_anchor(self.toast_corner());
                    let margin = to_logical(POPUP_MARGIN, pixel);
                    layer.set_margin(margin, margin, margin, margin);
                    layer.set_size(to_logical(width, pixel) as u32, to_logical(60, pixel) as u32);
                    layer.set_keyboard_interactivity(KeyboardInteractivity::None);
                    layer.commit();
                }
            }
            (true, Some(index)) => self.draw(index),
            (false, None) => {}
        }
        self.draw_role(|role| matches!(role, Role::Bar | Role::Popup(Kind::Notifications | Kind::Calendar)));
    }

    /// The bar on `output`, and the desktop widgets when they are on.
    fn add_output(&mut self, output: &wl_output::WlOutput) {
        let name = self.output_state.info(output).and_then(|info| info.name).unwrap_or_default();
        let pixel = self.pixel();
        let thickness = bar::thickness(&self.config, self.bottom());
        let layer = self.create_layer(Role::Bar, output, &name, "smalta-bar");
        let edge = if self.bottom() { Anchor::BOTTOM } else { Anchor::TOP };
        layer.set_anchor(edge | Anchor::LEFT | Anchor::RIGHT);
        layer.set_size(0, to_logical(thickness, pixel) as u32);
        layer.set_exclusive_zone(to_logical(thickness, pixel));
        layer.commit();
        log::info!("bar on {name}");

        // The wallpaper first, so the widgets are above it.
        if self.config.wallpaper.enabled && self.config.wallpaper.file(&self.config.theme).is_some() {
            let layer = self.create_layer(Role::Wallpaper, output, &name, "smalta-wallpaper");
            layer.set_anchor(Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT);
            // Under the bar too, to the screen's edges.
            layer.set_exclusive_zone(-1);
            layer.set_keyboard_interactivity(KeyboardInteractivity::None);
            if let Ok(region) = smithay_client_toolkit::compositor::Region::new(&self.compositor_state) {
                layer.wl_surface().set_input_region(Some(region.wl_region()));
            }
            layer.commit();
        }

        if self.config.desktop.enabled {
            let layer = self.create_layer(Role::Desktop, output, &name, "smalta-desktop");
            layer.set_anchor(Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT);
            layer.set_keyboard_interactivity(KeyboardInteractivity::None);
            // Clicks go through to whatever is under the widgets.
            if let Ok(region) = smithay_client_toolkit::compositor::Region::new(&self.compositor_state) {
                layer.wl_surface().set_input_region(Some(region.wl_region()));
            }
            layer.commit();
        }
    }

    /// The config read again and put to use without a restart. What the
    /// services were started with -- the weather's place, whether the
    /// notification server and the tray run -- still takes one; so does a
    /// new font size, which every surface is measured in.
    fn apply_config(&mut self, config: Config) {
        let old = &self.config;
        if old.screenshare != config.screenshare {
            self.compositor(Command::HideApps(config.screenshare.hide.clone()));
        }
        let restart = old.weather != config.weather
            || old.notifications.server != config.notifications.server
            || old.tray.enabled != config.tray.enabled
            || old.theme.font != config.theme.font;
        if restart {
            self.restart = true;
            self.exit = true;
            return;
        }
        // A wallpaper turned on or off, or one whose file is newly found or
        // lost, is a surface made or gone.
        let wallpaper = |c: &Config| c.wallpaper.enabled && c.wallpaper.file(&c.theme).is_some();
        let rebuild = (
            &old.bar.position,
            old.bar.size,
            old.bar.floating,
            old.bar.margin,
            old.theme.pixel,
            old.desktop.enabled,
            wallpaper(old),
        ) != (
            &config.bar.position,
            config.bar.size,
            config.bar.floating,
            config.bar.margin,
            config.theme.pixel,
            config.desktop.enabled,
            wallpaper(&config),
        );
        // The floating popup stays where it is on screen: its place is kept in
        // shell pixels, which have just changed size.
        let (before, after) = (self.pixel(), pixel_of(&config));
        if before != after
            && let Some(g) = &mut self.floating
        {
            let scale = |v: i32| (v as f32 * before / after).round() as i32;
            *g = Geometry { x: scale(g.x), y: scale(g.y), w: scale(g.w), h: scale(g.h) };
        }
        // A new accent or picture fades in; a slider dragged through shades
        // and darkness would only flicker, so those change at once.
        let next = theme_for(&config);
        let (a, b) = (&self.config.wallpaper, &config.wallpaper);
        let picture =
            |w: &config::Wallpaper, c: &Config| (w.enabled, w.file(&c.theme), w.tint.clone(), w.dither, w.pixelate);
        if next.accent != self.theme.accent || picture(a, &self.config) != picture(b, &config) {
            self.begin_fade();
        }
        i18n::init(&config.interface.language);
        self.theme = next;
        // A new number of bars takes a new cava.
        if self.config.media.bars != config.media.bars {
            self.visualizer.stop();
        }
        self.config = config;
        self.visualize();
        log::info!("config applied");
        if rebuild {
            // Bars, wallpapers, desktops and whatever hangs off the bar's edge, made again.
            self.surfaces
                .retain(|s| !matches!(s.role, Role::Bar | Role::Desktop | Role::Toasts | Role::Osd | Role::Wallpaper));
            for output in self.output_state.outputs().collect::<Vec<_>>() {
                self.add_output(&output);
            }
            self.apply_floating();
            self.refresh_notifications();
        } else {
            // The toasts' corner may have moved.
            self.surfaces.retain(|s| s.role != Role::Toasts);
            self.refresh_notifications();
            // Settings can apply a config during a draw; do not re-enter its widgets.
            for index in 0..self.surfaces.len() {
                self.schedule(index);
            }
        }
    }

    /// The bar, the wallpaper and the widgets kept as they are now, to
    /// dissolve from into whatever the next draw makes of them.
    fn begin_fade(&mut self) {
        let fades = |role: Role| matches!(role, Role::Bar | Role::Wallpaper | Role::Desktop);
        for index in 0..self.surfaces.len() {
            let surface = &mut self.surfaces[index];
            if !fades(surface.role) || !surface.configured {
                continue;
            }
            surface.new = None;
            // Midway through a fade, the picture it began from stays the one to leave.
            if surface.old.is_none() {
                surface.keep = true;
                self.draw(index);
            }
        }
        let running = self.fading_since.is_some();
        self.fading_since = Some(std::time::Instant::now());
        if running {
            return;
        }
        let _ = self.handle.insert_source(Timer::immediate(), move |_, _, app: &mut App| {
            let done = app.fading_since.is_none_or(|start| fade::progress(start) >= 1.0);
            if done {
                app.fading_since = None;
                for surface in &mut app.surfaces {
                    surface.old = None;
                    surface.new = None;
                }
            }
            app.draw_role(fades);
            if done { TimeoutAction::Drop } else { TimeoutAction::ToDuration(fade::FRAME) }
        });
    }

    /// The corner toasts come up in: the configured one, or the right-hand
    /// corner by the bar.
    fn toast_corner(&self) -> Anchor {
        let bar = if self.bottom() { Anchor::BOTTOM } else { Anchor::TOP };
        match self.config.notifications.position.as_str() {
            "top-left" => Anchor::TOP | Anchor::LEFT,
            "top-right" => Anchor::TOP | Anchor::RIGHT,
            "bottom-left" => Anchor::BOTTOM | Anchor::LEFT,
            "bottom-right" => Anchor::BOTTOM | Anchor::RIGHT,
            _ => bar | Anchor::RIGHT,
        }
    }

    /// A notification of the shell's own, as if a program had sent it.
    fn notice(&mut self, app: &str, summary: String, body: String) {
        // Above any id the server hands out.
        self.local_id = self.local_id.max(0x8000_0000) + 1;
        let incoming = services::notifications::Incoming {
            id: self.local_id,
            app: app.to_owned(),
            summary,
            body,
            actions: Vec::new(),
            urgency: 1,
            timeout: -1,
        };
        let settings = &self.config.notifications;
        let quiet = self.dnd || (settings.quiet_when_sharing && self.snapshot.casting);
        self.notifications.add(incoming, quiet, settings);
        self.refresh_notifications();
    }

    /// cava running while something plays and the bar shows what, stopped otherwise.
    fn visualize(&mut self) {
        let media = &self.config.media;
        let modules = &self.config.bar.modules;
        let shown =
            [&modules.start, &modules.center, &modules.end].iter().any(|ids| ids.iter().any(|id| id == "media"));
        let playing = self.services.media.players.iter().any(|p| p.playing);
        let was = self.visualizer.running();
        self.visualizer.want(media.visualizer && shown && playing, media.bars, &self.frames);
        if was && !self.visualizer.running() {
            self.draw_role(|role| role == Role::Bar);
        }
    }

    /// Once per device each time it runs low: a warning that it wants charging.
    /// The laptop's own battery counts as one, under the path `battery`.
    fn warn_low_devices(&mut self) {
        let mut low = self.services.bluetooth.low();
        if let Some(battery) = self.services.power.battery.as_ref().filter(|b| b.low()) {
            let level = (battery.percent * 100.0).round() as u8;
            low.push(("battery".to_owned(), tr("Laptop").to_owned(), level));
        }
        self.warned.retain(|path| low.iter().any(|(p, ..)| p == path));
        for (path, name, level) in low {
            if !self.warned.contains(&path) {
                self.warned.push(path);
                let summary = format!("{} — {name}, {level}%", tr("Battery low"));
                self.notice("bluetooth", summary, tr("Charge it soon").to_owned());
            }
        }
    }

    /// Show the OSD for a moment, creating its surface if it is not up.
    fn osd(&mut self, icon: icons::Icon, level: Option<f32>, text: String) {
        if self.started.elapsed() < Duration::from_secs(2) {
            return;
        }
        self.osd = Some((icon, level, text));
        if let Some(token) = self.osd_timer.take() {
            self.handle.remove(token);
        }
        let timer = Timer::from_duration(Duration::from_millis(self.config.osd.timeout.clamp(300, 10_000) as u64));
        self.osd_timer = self
            .handle
            .insert_source(timer, |_, _, app: &mut App| {
                app.osd_timer = None;
                app.osd = None;
                app.surfaces.retain(|s| s.role != Role::Osd);
                TimeoutAction::Drop
            })
            .ok();
        if let Some(index) = self.surfaces.iter().position(|s| s.role == Role::Osd) {
            self.draw(index);
            return;
        }
        let output_name = self.focused_output();
        let Some(output) = self
            .surfaces
            .iter()
            .find(|s| s.role == Role::Bar && s.output_name == output_name)
            .or_else(|| self.surfaces.iter().find(|s| s.role == Role::Bar))
            .map(|s| s.output.clone())
        else {
            return;
        };
        let pixel = self.pixel();
        let layer = self.create_layer(Role::Osd, &output, &output_name, "smalta-osd");
        let bar = if self.bottom() { Anchor::BOTTOM } else { Anchor::TOP };
        layer.set_anchor(match self.config.osd.position.as_str() {
            "top" => Anchor::TOP,
            "bottom" => Anchor::BOTTOM,
            "center" => Anchor::empty(),
            _ => bar,
        });
        let margin = to_logical(48, pixel);
        layer.set_margin(margin, 0, margin, 0);
        layer.set_size(
            to_logical(240, pixel) as u32,
            to_logical(self.face.line_height() + 20 + popups::SHADOW, pixel) as u32,
        );
        layer.set_keyboard_interactivity(KeyboardInteractivity::None);
        // Input passes straight through: the OSD is never in the way.
        if let Ok(region) = smithay_client_toolkit::compositor::Region::new(&self.compositor_state) {
            layer.wl_surface().set_input_region(Some(region.wl_region()));
        }
        layer.commit();
    }

    /// Freeze every output with a bar and, as each copy comes in, show it: a
    /// colour can be picked from any of them.
    fn pick_color(&mut self) {
        if self.picking.iter().any(|p| p.purpose == picker::Purpose::Picker) || !self.capture.is_empty() {
            return;
        }
        if self.freeze(picker::Purpose::Picker, None) == 0 {
            log::error!("this compositor offers no wlr-screencopy; no colour picker");
        }
    }

    /// Start copying the outputs with a bar -- or only those named -- for
    /// `purpose`; each arrives at [`App::captured`]. Returns how many were started.
    fn freeze(&mut self, purpose: picker::Purpose, only: Option<&[String]>) -> usize {
        let Some(manager) = self.screencopy.clone() else { return 0 };
        let outputs: Vec<(wl_output::WlOutput, String)> = self
            .surfaces
            .iter()
            .filter(|s| s.role == Role::Bar && only.is_none_or(|names| names.contains(&s.output_name)))
            .map(|s| (s.output.clone(), s.output_name.clone()))
            .collect();
        let mut started = 0;
        for (output, name) in outputs {
            let size = self
                .output_state
                .info(&output)
                .and_then(|i| i.modes.iter().find(|m| m.current).map(|m| m.dimensions))
                .unwrap_or((1920, 1080));
            let Ok(pool) = SlotPool::new((size.0 * size.1 * 4).max(4) as usize, &self.shm) else { continue };
            self.picking.push(picker::Pending::start(&manager, &output, name, purpose, pool, &self.qh));
            started += 1;
        }
        started
    }

    /// A frozen output has come in, or failed to.
    pub fn captured(&mut self, purpose: picker::Purpose, output: &str, capture: Option<picker::Capture>) {
        match purpose {
            picker::Purpose::Picker => {
                if let Some(capture) = capture {
                    self.show_picker(capture);
                }
            }
            picker::Purpose::Switcher => {
                if let Some(capture) = capture {
                    let logical = self
                        .surfaces
                        .iter()
                        .find(|s| s.role == Role::Bar && s.output_name == output)
                        .and_then(|s| self.output_state.info(&s.output))
                        .and_then(|info| info.logical_size)
                        .unwrap_or((capture.width, capture.height));
                    self.popups.switcher.take(capture, logical);
                }
                // Up once the last screen is in.
                if !self.picking.iter().any(|p| p.purpose == picker::Purpose::Switcher) {
                    self.open_switcher();
                }
            }
            picker::Purpose::Shot => match capture {
                Some(capture) => self.show_shot(capture),
                // None of the screens came: the compositor's own way, then.
                None if !self.shot.active() && !self.picking.iter().any(|p| p.purpose == picker::Purpose::Shot) => {
                    self.compositor(Command::Screenshot)
                }
                None => {}
            },
        }
    }

    /// Freeze every screen and take a screenshot from them, marked up in the
    /// shell's own editor; without wlr-screencopy, the compositor's way.
    fn screenshot(&mut self) {
        if self.shot.active() || self.picking.iter().any(|p| p.purpose == picker::Purpose::Shot) {
            return;
        }
        if self.freeze(picker::Purpose::Shot, None) == 0 {
            self.compositor(Command::Screenshot);
        }
    }

    fn show_shot(&mut self, capture: picker::Capture) {
        let Some(output) = self
            .surfaces
            .iter()
            .find(|s| s.role == Role::Bar && s.output_name == capture.output)
            .map(|s| s.output.clone())
        else {
            return;
        };
        let name = capture.output.clone();
        self.shot.captures.push(capture);
        let layer = self.create_layer(Role::Shot, &output, &name, "smalta-screenshot");
        layer.set_anchor(Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT);
        layer.set_exclusive_zone(-1);
        layer.set_keyboard_interactivity(KeyboardInteractivity::Exclusive);
        layer.commit();
    }

    /// The screenshot copied, kept or dropped, and the frozen screens gone.
    fn finish_shot(&mut self, finish: shot::Finish) {
        let ratio = |capture: &picker::Capture| {
            let width = self
                .surfaces
                .iter()
                .find(|s| s.role == Role::Shot && s.output_name == capture.output)
                .map_or(capture.width, |s| s.width);
            capture.width as f32 / width.max(1) as f32
        };
        let picture = (finish != shot::Finish::Cancel).then(|| self.shot.render(ratio)).flatten();
        self.shot.clear();
        self.picking.retain(|p| p.purpose != picker::Purpose::Shot);
        self.surfaces.retain(|s| s.role != Role::Shot);
        if let Some((width, height, rgba)) = picture {
            let path = shot::deliver(width, height, rgba, finish == shot::Finish::Save);
            self.started = std::time::Instant::now() - Duration::from_secs(10);
            let text = shot::summary(path.as_ref());
            match path {
                Some(_) => self.notice("smalta", tr("Screenshot").to_owned(), text),
                None => self.osd(icons::CAMERA, None, text.to_lowercase()),
            }
        }
    }

    /// `smalta switch`: the switcher opened on the window used before this
    /// one, or, already open, the next one chosen.
    fn switch(&mut self, step: i32) {
        if self.open.as_ref().is_some_and(|(kind, _)| *kind == Kind::Windows) {
            self.popups.switcher.step(step);
            self.redraw_popup();
            return;
        }
        if self.switching {
            return;
        }
        self.close();
        self.popups.switcher.prepare(self.snapshot.windows.clone(), step, true);
        // The screens frozen first, before the switcher is on them.
        let outputs = self.popups.switcher.outputs();
        if self.freeze(picker::Purpose::Switcher, Some(&outputs)) == 0 {
            self.show(Kind::Windows, None);
            return;
        }
        self.switching = true;
        // A compositor slow to copy does not hold the switcher back for long.
        let timer = Timer::from_duration(Duration::from_millis(300));
        let _ = self.handle.insert_source(timer, |_, _, app: &mut App| {
            app.open_switcher();
            TimeoutAction::Drop
        });
    }

    fn open_switcher(&mut self) {
        if std::mem::take(&mut self.switching) {
            self.picking.retain(|p| p.purpose != picker::Purpose::Switcher);
            self.show(Kind::Windows, None);
        }
    }

    /// The modifiers changed while the shell had the keyboard.
    pub fn modifiers(&mut self, held: bool) {
        if !self.open.as_ref().is_some_and(|(kind, _)| *kind == Kind::Windows) {
            return;
        }
        let mut requests = Vec::new();
        let mut cx = Ctx {
            services: &self.services,
            handles: &self.handles,
            requests: &mut requests,
            dnd: self.dnd,
            recording: self.recorder.recording(),
            notifications: &self.notifications,
            config: &self.config,
            snapshot: &self.snapshot,
        };
        self.popups.switcher.modifiers(held, &mut cx);
        self.handle_requests(requests);
    }

    pub fn show_picker(&mut self, capture: picker::Capture) {
        let Some(output) = self
            .surfaces
            .iter()
            .find(|s| s.role == Role::Bar && s.output_name == capture.output)
            .map(|s| s.output.clone())
        else {
            return;
        };
        let name = capture.output.clone();
        self.capture.push(capture);
        let layer = self.create_layer(Role::Picker, &output, &name, "smalta-picker");
        layer.set_anchor(Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT);
        // Over the bar too: the whole screen is the picture.
        layer.set_exclusive_zone(-1);
        layer.set_keyboard_interactivity(KeyboardInteractivity::Exclusive);
        layer.commit();
    }

    fn close_picker(&mut self) {
        self.capture.clear();
        self.picking.clear();
        self.surfaces.retain(|s| s.role != Role::Picker);
    }

    fn compositor(&self, command: Command) {
        if let Some(commands) = &self.commands {
            let _ = commands.send(command);
        }
    }

    fn draw_all(&mut self) {
        for index in 0..self.surfaces.len() {
            self.draw(index);
        }
    }

    fn draw_role(&mut self, wanted: impl Fn(Role) -> bool) {
        for index in 0..self.surfaces.len() {
            if wanted(self.surfaces[index].role) {
                self.draw(index);
            }
        }
    }

    // -- popups ------------------------------------------------------------

    fn focused_output(&self) -> String {
        self.snapshot
            .workspaces
            .iter()
            .find(|w| w.focused)
            .map(|w| w.output.clone())
            .or_else(|| self.surfaces.iter().find(|s| s.role == Role::Bar).map(|s| s.output_name.clone()))
            .unwrap_or_default()
    }

    fn toggle(&mut self, kind: Kind, output: Option<String>) {
        if self.open.as_ref().is_some_and(|(open, _)| *open == kind) {
            self.close();
        } else if kind == Kind::Windows {
            // Without a modifier to let go of: Enter or a click chooses.
            self.switch(1);
            self.popups.switcher.armed = false;
        } else {
            self.show(kind, output);
        }
    }

    fn show(&mut self, kind: Kind, output: Option<String>) {
        self.close();
        let output_name = output.unwrap_or_else(|| self.focused_output());
        let Some(output) = self
            .surfaces
            .iter()
            .find(|s| s.role == Role::Bar && s.output_name == output_name)
            .or_else(|| self.surfaces.iter().find(|s| s.role == Role::Bar))
            .map(|s| s.output.clone())
        else {
            return;
        };
        self.popups.opened(kind);
        if kind == Kind::Weather {
            let _ = self.handles.weather.send(services::weather::Command::Refresh);
        }
        if kind == Kind::Notifications {
            self.notifications.unread = false;
            self.draw_role(|role| role == Role::Bar);
        }
        self.open = Some((kind, output_name.clone()));

        let catcher = self.create_layer(Role::Catcher, &output, &output_name, "smalta-catcher");
        if let (Some(viewporter), Some(surface)) = (&self.viewporter, self.surfaces.last_mut()) {
            surface.viewport = Some(viewport::get(viewporter, catcher.wl_surface(), &self.qh));
        }
        catcher.set_anchor(Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT);
        catcher.set_keyboard_interactivity(KeyboardInteractivity::None);
        catcher.commit();

        let pixel = self.pixel();
        let width = kind.width() + popups::PADDING * 2 + popups::SHADOW;
        let layer = self.create_layer(Role::Popup(kind), &output, &output_name, "smalta-popup");
        let edge = if self.bottom() { Anchor::BOTTOM } else { Anchor::TOP };
        let margin = to_logical(POPUP_MARGIN, pixel);
        match kind.placement() {
            Placement::Center => layer.set_anchor(Anchor::empty()),
            Placement::Bar(Align::Left) if self.anchor_x.is_some() => {
                layer.set_anchor(edge | Anchor::LEFT);
                let left = to_logical((self.anchor_x.unwrap_or(0) - width / 2).max(POPUP_MARGIN), pixel);
                layer.set_margin(margin, margin, margin, left);
            }
            Placement::Bar(Align::Left) => layer.set_anchor(edge | Anchor::LEFT),
            Placement::Bar(Align::Right) => layer.set_anchor(edge | Anchor::RIGHT),
            Placement::Bar(Align::Center) => layer.set_anchor(edge),
            Placement::Side => layer.set_anchor(Anchor::TOP | Anchor::BOTTOM | Anchor::RIGHT),
            Placement::Floating => layer.set_anchor(Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT),
            Placement::Screen => {
                layer.set_anchor(Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT);
                // Over the bar too, not beside it.
                layer.set_exclusive_zone(-1);
            }
        }
        if kind.placement() == Placement::Screen {
            layer.set_size(0, 0);
        } else if kind.placement() == Placement::Floating {
            // Where it was left, or the middle of the output the first time.
            let screen = self.output_size(&output);
            let geometry = self
                .floating
                .unwrap_or_else(|| {
                    let h = 540.min(screen.1 - 80);
                    Geometry { x: (screen.0 - width) / 2, y: (screen.1 - h) / 2, w: width, h }
                })
                .clamped(screen);
            self.floating = Some(geometry);
            // The whole screen: the popup is drawn into it where it belongs.
            layer.set_size(0, 0);
        } else {
            if !(kind.placement() == Placement::Bar(Align::Left) && self.anchor_x.is_some()) {
                layer.set_margin(margin, margin, margin, margin);
            }
            // A side panel takes the height the compositor gives it; the rest
            // start small and grow to their content.
            let height = if kind.placement() == Placement::Side { 0 } else { to_logical(120, pixel) };
            layer.set_size(to_logical(width, pixel) as u32, height as u32);
        }
        layer.set_keyboard_interactivity(kind.keyboard());
        layer.commit();
    }

    /// The drag a press at `x, y` on a floating popup starts: the corner
    /// resizes, the title row moves, the rest is the popup's own. `x, y`
    /// are the popup's own; `raw`, the surface's.
    fn drag_at(&self, index: usize, x: f64, y: f64, raw: (f64, f64)) -> Option<Drag> {
        let surface = &self.surfaces[index];
        let start = self.floating?;
        // A button in the title row is a button, not a handle.
        if surface.hot.iter().any(|rect| rect.contains(x, y)) {
            return None;
        }
        let (right, bottom) = ((start.w - popups::SHADOW) as f64, (start.h - popups::SHADOW) as f64);
        let title = (popups::PADDING + self.face.line_height() + 4) as f64;
        let kind = if x >= right - 14.0 && y >= bottom - 14.0 {
            DragKind::Resize
        } else if y < title {
            DragKind::Move
        } else {
            return None;
        };
        Some(Drag { kind, from: raw, start })
    }

    /// The popup being dragged, moved or resized to follow the pointer at
    /// `raw` on its surface, which stays put: the travel is exact.
    fn drag_to(&mut self, index: usize, raw: (f64, f64)) {
        let Some(drag) = self.drag else { return };
        let Some(now) = self.floating else { return };
        let (dx, dy) = ((raw.0 - drag.from.0).round() as i32, (raw.1 - drag.from.1).round() as i32);
        let start = drag.start;
        let wanted = match drag.kind {
            DragKind::Move => Geometry { x: start.x + dx, y: start.y + dy, ..start },
            DragKind::Resize => Geometry { w: start.w + dx, h: start.h + dy, ..start },
        };
        let surface = &self.surfaces[index];
        let geometry = wanted.clamped((surface.width, surface.height));
        if (geometry.x, geometry.y, geometry.w, geometry.h) != (now.x, now.y, now.w, now.h) {
            self.floating = Some(geometry);
            self.schedule(index);
        }
    }

    /// Where the floating popup was left, kept in screen pixels so a
    /// change of pixel scale does not move it.
    fn window_path() -> std::path::PathBuf {
        let home = std::env::var("HOME").unwrap_or_default();
        let state = std::env::var("XDG_STATE_HOME").unwrap_or_else(|_| format!("{home}/.local/state"));
        std::path::Path::new(&state).join("smalta/window.json")
    }

    fn save_window(&self) {
        let Some(g) = self.floating else { return };
        let pixel = self.pixel();
        let logical = |v: i32| to_logical(v, pixel);
        let saved = [logical(g.x), logical(g.y), logical(g.w), logical(g.h)];
        let path = Self::window_path();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(path, serde_json::to_string(&saved).unwrap_or_default());
    }

    fn load_window(pixel: f32) -> Option<Geometry> {
        let text = std::fs::read_to_string(Self::window_path()).ok()?;
        let [x, y, w, h]: [i32; 4] = serde_json::from_str(&text).ok()?;
        let shell = |v: i32| (v as f32 / pixel).round() as i32;
        Some(Geometry { x: shell(x), y: shell(y), w: shell(w), h: shell(h) })
    }

    /// An output's size in shell pixels.
    fn output_size(&self, output: &wl_output::WlOutput) -> (i32, i32) {
        let pixel = self.pixel();
        let (w, h) = self.output_state.info(output).and_then(|info| info.logical_size).unwrap_or((1920, 1080));
        ((w as f32 / pixel) as i32, (h as f32 / pixel) as i32)
    }

    /// The floating popup drawn again where `self.floating` now says.
    fn apply_floating(&mut self) {
        let floating = |role: Role| matches!(role, Role::Popup(kind) if kind.placement() == Placement::Floating);
        if let Some(index) = self.surfaces.iter().position(|s| floating(s.role)) {
            self.schedule(index);
        }
    }

    fn close(&mut self) {
        if self.open.take().is_some() {
            self.drag = None;
            self.anchor_x = None;
            self.surfaces.retain(|s| {
                matches!(s.role, Role::Bar | Role::Toasts | Role::Osd | Role::Desktop | Role::Wallpaper | Role::Shot)
            });
            self.keyboard.stop_repeat(&self.handle);
        }
    }

    fn create_layer(&mut self, role: Role, output: &wl_output::WlOutput, name: &str, namespace: &str) -> LayerSurface {
        let qh = self.qh.clone();
        let wl = self.compositor_state.create_surface(&qh);
        // The layer is fixed at creation: compositors do not all honour a
        // later `set_layer`, and Hyprland ignores one sent before the first commit.
        let level = match role {
            Role::Desktop | Role::Wallpaper => Layer::Background,
            Role::Picker | Role::Shot => Layer::Overlay,
            _ => Layer::Top,
        };
        let layer = self.layer_shell.create_layer_surface(&qh, wl, level, Some(namespace), Some(output));
        let scale = self.output_state.info(output).map_or(1, |info| info.scale_factor.max(1));
        let pool = SlotPool::new(256 * 256 * 4, &self.shm).expect("shared memory pool");
        self.surfaces.push(Surface {
            role,
            output: output.clone(),
            output_name: name.to_owned(),
            layer: layer.clone(),
            pool,
            width: 0,
            height: 0,
            logical: (0, 0),
            scale,
            configured: false,
            input: Input::default(),
            hits: Vec::new(),
            hot: Vec::new(),
            workspaces: None,
            viewport: None,
            keep: false,
            old: None,
            new: None,
        });
        layer
    }

    // -- input -------------------------------------------------------------

    fn act(&mut self, action: Action, output: &str) {
        self.act_with(action, output, BTN_LEFT, None);
    }

    fn act_with(&mut self, action: Action, output: &str, button: u32, x: Option<i32>) {
        if let Action::Tray(key, is_menu) = &action {
            let tray = &self.handles.tray;
            use services::tray::Command as T;
            match button {
                BTN_LEFT if !*is_menu => drop(tray.send(T::Activate(key.clone()))),
                BTN_MIDDLE => drop(tray.send(T::Secondary(key.clone()))),
                _ => {
                    self.services.tray_menu = None;
                    let _ = tray.send(T::Menu(key.clone()));
                    self.popups.traymenu.key = key.clone();
                    self.close();
                    self.anchor_x = x;
                    self.show(Kind::TrayMenu, Some(output.to_owned()));
                }
            }
            return;
        }
        if button != BTN_LEFT {
            return;
        }
        let command = match action {
            Action::FocusWorkspace(id) => Command::FocusWorkspace(id),
            Action::SwitchLayout => Command::SwitchLayout,
            Action::Toggle(kind) => {
                self.anchor_x = None;
                self.toggle(kind, Some(output.to_owned()));
                return;
            }
            Action::Tray(..) => return,
            Action::PlayPause => {
                if let Some(player) = self.services.media.current() {
                    let _ = self.handles.media.send(services::media::Command::PlayPause(player.bus.clone()));
                }
                return;
            }
            Action::StopRecording => {
                self.recorder.toggle();
                self.draw_role(|role| role == Role::Bar);
                return;
            }
        };
        if let Some(commands) = &self.commands {
            let _ = commands.send(command);
        }
    }

    /// Scroll over the workspaces moves to the neighbour on this bar.
    fn cycle(&mut self, index: usize, step: i32) {
        let surface = &self.surfaces[index];
        let cx = Context {
            config: &self.config,
            theme: &self.theme,
            face: &self.face,
            snapshot: &self.snapshot,
            resources: &self.resources,
            output: &surface.output_name,
            unread: false,
            dnd: false,
            services: &self.services,
            recording: None,
            levels: &[],
        };
        let shown = cx.shown_workspaces();
        let Some(current) = shown.iter().position(|w| w.active) else { return };
        let next = current as i32 + step;
        if next >= 0 && next < shown.len() as i32 {
            let id = shown[next as usize].id;
            let output = surface.output_name.clone();
            self.act(Action::FocusWorkspace(id), &output);
        }
    }

    fn set_cursor(&self, shape: Shape) {
        if let Some(cursor) = &self.cursor {
            cursor.set_shape(self.enter_serial, shape);
        }
    }

    /// The pointer over a surface: a hand on what can be clicked, and on a
    /// floating popup, a grip on its title and a resize arrow on its corner.
    fn cursor_at(&self, index: usize) -> Shape {
        let surface = &self.surfaces[index];
        let Some((x, y)) = surface.input.pointer else { return Shape::Default };
        if surface.hits.iter().any(|hit| hit.rect.contains(x, y)) || surface.hot.iter().any(|rect| rect.contains(x, y))
        {
            return Shape::Pointer;
        }
        if surface.role == Role::Shot {
            return Shape::Crosshair;
        }
        if let Role::Popup(kind) = surface.role
            && kind.placement() == Placement::Floating
            && let Some(g) = self.floating
        {
            let (right, bottom) = ((g.w - popups::SHADOW) as f64, (g.h - popups::SHADOW) as f64);
            let inside = x >= 0.0 && y >= 0.0 && x < right && y < bottom;
            if inside && x >= right - 14.0 && y >= bottom - 14.0 {
                return Shape::SeResize;
            }
            if inside && y < (popups::PADDING + self.face.line_height() + 4) as f64 {
                return Shape::Grab;
            }
        }
        Shape::Default
    }

    /// Redraw the open popup once the events at hand are through.
    fn redraw_popup(&mut self) {
        let Some((kind, _)) = self.open else { return };
        if let Some(index) = self.surfaces.iter().position(|s| s.role == Role::Popup(kind)) {
            self.schedule(index);
        }
    }

    /// Mark a surface to be drawn when the event loop next goes idle. Pointer
    /// events come as fast as the mouse reports them, two hundred a second
    /// and more; drawing for each falls behind on a large surface, and a
    /// compositor whose messages pile up unread drops the client. However
    /// many arrive together, this draws once.
    fn schedule(&mut self, index: usize) {
        let surface = self.surfaces[index].layer.wl_surface().clone();
        if !self.dirty.contains(&surface) {
            self.dirty.push(surface);
        }
        if !self.flush_queued {
            self.flush_queued = true;
            self.handle.insert_idle(|app: &mut App| app.flush());
        }
    }

    fn flush(&mut self) {
        self.flush_queued = false;
        let find = |app: &App, surface: &wl_surface::WlSurface| {
            app.surfaces.iter().position(|s| s.layer.wl_surface() == surface)
        };
        for surface in std::mem::take(&mut self.dirty) {
            let Some(index) = find(self, &surface) else { continue };
            let popup = matches!(self.surfaces[index].role, Role::Popup(_));
            // A popup draws twice when input changed it: once to let the
            // widgets claim a click or a drag, once to show what that did.
            let input = &self.surfaces[index].input;
            let shot = self.surfaces[index].role == Role::Shot;
            let twice = (popup && (input.click.is_some() || input.held)) || (shot && input.click.is_some());
            self.draw(index);
            if twice && let Some(index) = find(self, &surface) {
                self.draw(index);
            }
            // Scrolling not taken by the draw is not saved for later.
            if popup && let Some(index) = find(self, &surface) {
                self.surfaces[index].input.scroll = 0;
                self.surfaces[index].input.pixels = 0.0;
            }
        }
    }

    fn redraw_popup_times(&mut self, times: usize) {
        let Some((kind, _)) = self.open else { return };
        for _ in 0..times {
            if let Some(index) = self.surfaces.iter().position(|s| s.role == Role::Popup(kind)) {
                self.draw(index);
            }
        }
    }

    pub fn key(&mut self, key: Key) {
        if self.shot.active() {
            match self.shot.key(&key) {
                Some(finish) => self.finish_shot(finish),
                None => self.draw_role(|role| role == Role::Shot),
            }
            return;
        }
        if !self.capture.is_empty() {
            if key == Key::Escape {
                self.close_picker();
            }
            return;
        }
        let Some((kind, _)) = self.open else { return };
        let mut requests = Vec::new();
        let recording = self.recorder.recording();
        let mut cx = Ctx {
            services: &self.services,
            handles: &self.handles,
            requests: &mut requests,
            dnd: self.dnd,
            recording,
            notifications: &self.notifications,
            config: &self.config,
            snapshot: &self.snapshot,
        };
        let outcome = self.popups.key(kind, &key, &mut cx);
        self.handle_requests(requests);
        match outcome {
            Outcome::Close => self.close(),
            // A key has already changed the state; one draw shows it.
            Outcome::Redraw => self.redraw_popup_times(1),
            Outcome::Ignored => {}
        }
    }

    // -- ipc ---------------------------------------------------------------

    fn command(&mut self, line: &str) -> String {
        let words: Vec<&str> = line.split_whitespace().collect();
        match words.as_slice() {
            [verb @ ("toggle" | "show"), name] => match Kind::parse(name) {
                Some(kind) if *verb == "toggle" || kind == Kind::Windows => {
                    self.toggle(kind, None);
                    format!("toggled {name}")
                }
                Some(kind) => {
                    self.show(kind, None);
                    format!("shown {name}")
                }
                None => format!("error: no such window: {name}"),
            },
            ["close"] => {
                self.close();
                "closed".into()
            }
            ["quit"] => {
                self.exit = true;
                "bye".into()
            }
            ["screenshot"] => {
                self.handle_requests(vec![Request::Screenshot]);
                "taking a screenshot".into()
            }
            ["pick-color"] => {
                self.handle_requests(vec![Request::PickColor]);
                "picking a colour".into()
            }
            ["switch"] => {
                self.switch(1);
                "switched".into()
            }
            ["switch", "back"] => {
                self.switch(-1);
                "switched".into()
            }
            ["toggle-hidden"] => {
                self.compositor(Command::ToggleHidden);
                "toggled the focused window in screen capture".into()
            }
            ["reload"] => {
                self.apply_config(config::load());
                if self.restart { "restarting" } else { "reloaded" }.into()
            }
            ["windows"] => Kind::ALL.iter().map(|kind| kind.name()).collect::<Vec<_>>().join("\n"),
            _ => format!("error: unknown command: {line}"),
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        None => {}
        Some("-h" | "--help" | "help") => {
            println!("{}", ipc::USAGE);
            return;
        }
        Some(_) => std::process::exit(ipc::send(&args)),
    }

    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let _ = popups::settings::STARTED.set(std::time::Instant::now());
    tune_malloc();

    let Some(listener) = ipc::listen() else {
        eprintln!("smalta: already running");
        std::process::exit(1);
    };

    let config = config::load();
    i18n::init(&config.interface.language);
    let theme = theme_for(&config);
    let face = font::load_default(config.theme.font.as_deref());

    let conn = Connection::connect_to_env().expect("no Wayland compositor to connect to");
    let (globals, event_queue) = registry_queue_init(&conn).expect("Wayland registry");
    let qh: QueueHandle<App> = event_queue.handle();

    let mut event_loop: EventLoop<App> = EventLoop::try_new().expect("event loop");
    let handle = event_loop.handle();
    WaylandSource::new(conn.clone(), event_queue).insert(handle.clone()).expect("Wayland source");

    let (snapshots, received) = channel::channel();
    let commands = compositor::spawn(snapshots);
    if commands.is_none() {
        log::error!("neither HYPRLAND_INSTANCE_SIGNATURE nor NIRI_SOCKET is set; workspaces stay empty");
    }
    if let Some(commands) = &commands {
        let _ = commands.send(Command::HideApps(config.screenshare.hide.clone()));
    }
    handle
        .insert_source(received, |event, _, app: &mut App| {
            if let Event::Msg(snapshot) = event {
                let layout =
                    (!app.snapshot.layout.is_empty() && app.snapshot.layout != snapshot.layout && snapshot.layouts > 1)
                        .then(|| snapshot.layout.clone());
                app.snapshot = snapshot;
                if let Some(layout) = layout
                    && app.config.osd.layout
                {
                    app.osd(icons::KEYBOARD, None, layout);
                }
                app.draw_role(|role| role == Role::Bar);
            }
        })
        .expect("compositor channel");

    insert_ipc(&handle, listener);

    let (events, received) = channel::channel();
    // Off, another notification daemon serves the name.
    if config.notifications.server {
        services::notifications::spawn(events);
    }
    handle
        .insert_source(received, |event, _, app: &mut App| {
            use services::notifications::Event as N;
            match event {
                Event::Msg(N::Ready(connection)) => app.notifications.connection = Some(connection),
                Event::Msg(N::Notify(incoming)) => {
                    let settings = &app.config.notifications;
                    let quiet = app.dnd || (settings.quiet_when_sharing && app.snapshot.casting);
                    app.notifications.add(incoming, quiet, settings);
                    app.refresh_notifications();
                }
                Event::Msg(N::Close(id)) => {
                    app.notifications.close(id, 3);
                    app.refresh_notifications();
                }
                Event::Closed => {}
            }
        })
        .expect("notification channel");

    let (updates, received) = channel::channel();
    let handles = services::spawn(updates, &config);
    handle
        .insert_source(received, |event, _, app: &mut App| {
            if let Event::Msg(update) = event {
                let level = |app: &App| {
                    let audio = &app.services.audio;
                    (
                        audio.speaker().map(|s| (s.volume, s.muted)),
                        audio.microphone().map(|m| m.muted),
                        app.services.power.brightness,
                    )
                };
                let before = level(app);
                let locks = app.services.locks;
                app.services.apply(update);
                let (speaker, mic, brightness) = level(app);
                let osd = app.config.osd.clone();
                if osd.volume && before.0.is_some() && speaker.is_some() && before.0 != speaker {
                    let (volume, muted) = speaker.unwrap_or_default();
                    let icon = if muted { icons::SPEAKER_MUTED } else { icons::SPEAKER };
                    app.osd(icon, Some(if muted { 0.0 } else { volume }), String::new());
                }
                // The microphone only when it is muted or unmuted: its level moves
                // on its own with some apps.
                if osd.microphone && before.1.is_some() && mic.is_some() && before.1 != mic {
                    let muted = mic.unwrap_or_default();
                    let label = if muted { tr("Microphone off") } else { tr("Microphone on") };
                    app.osd(if muted { icons::MIC_MUTED } else { icons::MIC }, None, label.to_lowercase());
                }
                if osd.brightness && before.2.is_some() && brightness.is_some() && before.2 != brightness {
                    app.osd(icons::SUN, brightness, String::new());
                }
                let now = app.services.locks;
                let lights =
                    [(locks.caps, now.caps, icons::CAPS, "Caps Lock"), (locks.num, now.num, icons::NUM, "Num Lock")];
                for (was, is, icon, name) in lights {
                    if osd.locks
                        && let (Some(was), Some(is)) = (was, is)
                        && was != is
                    {
                        app.osd(icon, None, format!("{name} {}", tr(if is { "on" } else { "off" })));
                    }
                }
                app.warn_low_devices();
                app.visualize();
                app.visualize();
                app.draw_role(|role| matches!(role, Role::Bar | Role::Popup(_)));
            }
        })
        .expect("services channel");

    // Once a second: the clock when its text changes, the meters every other tick.
    let mut tick = 0u32;
    handle
        .insert_source(Timer::immediate(), move |_, _, app: &mut App| {
            let clock = chrono::Local::now().format(&app.config.clock.format).to_string();
            let mut dirty = clock != app.clock;
            app.clock = clock;
            // The session menu has a clock of its own.
            if dirty && app.open.as_ref().is_some_and(|(kind, _)| *kind == Kind::Session) {
                app.redraw_popup();
            }
            dirty |= app.recorder.recording();
            if tick.is_multiple_of(2) {
                app.resources.sample();
                dirty = true;
            }
            tick += 1;
            // A pointer resting on the toasts holds them where they are.
            let settings = &app.config.notifications;
            let held = settings.pause_on_hover
                && app.surfaces.iter().any(|s| s.role == Role::Toasts && s.input.pointer.is_some());
            let step = if held { Duration::ZERO } else { Duration::from_secs(1) };
            if app.notifications.expire(step, settings) {
                app.refresh_notifications();
            }
            if dirty {
                app.draw_role(|role| matches!(role, Role::Bar | Role::Toasts | Role::Desktop));
            }
            TimeoutAction::ToDuration(Duration::from_secs(1))
        })
        .expect("timer");

    // The visualizer's frames: the bar redrawn only when they change.
    let (frames, received) = channel::channel::<Vec<u8>>();
    handle
        .insert_source(received, |event, _, app: &mut App| {
            if let Event::Msg(levels) = event
                && app.visualizer.running()
                && levels != app.visualizer.levels
            {
                app.visualizer.levels = levels;
                app.draw_role(|role| role == Role::Bar);
            }
        })
        .expect("visualizer channel");

    let window = App::load_window(pixel_of(&config));
    let mut app = App {
        registry_state: RegistryState::new(&globals),
        seat_state: SeatState::new(&globals, &qh),
        output_state: OutputState::new(&globals, &qh),
        compositor_state: CompositorState::bind(&globals, &qh).expect("wl_compositor"),
        layer_shell: LayerShell::bind(&globals, &qh).expect("this compositor has no layer shell"),
        shm: Shm::bind(&globals, &qh).expect("wl_shm"),
        cursor_shapes: CursorShapeManager::bind(&globals, &qh).ok(),
        pointer: None,
        cursor: None,
        enter_serial: 0,
        keyboard: Keyboard::default(),
        handle: handle.clone(),
        qh: qh.clone(),
        surfaces: Vec::new(),
        open: None,
        anchor_x: None,
        popups: Popups::default(),
        config,
        theme,
        face,
        snapshot: Snapshot::default(),
        resources: Resources::default(),
        clock: String::new(),
        commands,
        services: Services::default(),
        handles,
        recorder: Recorder::default(),
        clipboard: clipboard::Clipboard::default(),
        notifications: notifications::Notifications::default(),
        dnd: false,
        osd: None,
        osd_timer: None,
        started: std::time::Instant::now(),
        screencopy: picker::bind(&globals, &qh),
        viewporter: viewport::bind(&globals, &qh),
        picking: Vec::new(),
        capture: Vec::new(),
        shot: shot::Shot::default(),
        visualizer: visualizer::Visualizer::default(),
        frames,
        fading_since: None,
        local_id: 0,
        switching: false,
        warned: Vec::new(),
        exit: false,
        restart: false,
        floating: window,
        drag: None,
        dirty: Vec::new(),
        flush_queued: false,
    };

    app.clipboard.start();
    while !app.exit {
        if let Err(error) = event_loop.dispatch(None, &mut app) {
            log::error!("event loop: {error}");
            break;
        }
    }
    app.clipboard.stop();
    let _ = std::fs::remove_file(ipc::path());
    if app.restart {
        // The same program over this process: the config is read afresh, and
        // a supervisor such as systemd sees nothing exit. Descriptors close
        // on exec, so the Wayland and D-Bus connections are given up first.
        log::info!("restarting");
        drop(app);
        drop(event_loop);
        use std::os::unix::process::CommandExt;
        // By path rather than /proc/self/exe: a binary rebuilt or upgraded
        // since the start is the one that runs, under its own name.
        let exe = std::env::current_exe().map(|path| {
            let text = path.to_string_lossy();
            text.strip_suffix(" (deleted)").map_or(path.clone(), std::path::PathBuf::from)
        });
        let mut args = std::env::args_os();
        let arg0 = args.next().unwrap_or_default();
        let error = match exe {
            Ok(exe) => std::process::Command::new(exe).arg0(arg0).args(args).exec(),
            Err(error) => error,
        };
        log::error!("restart: {error}");
        std::process::exit(1);
    }
}

fn insert_ipc(handle: &LoopHandle<'static, App>, listener: UnixListener) {
    let source = Generic::new(listener, Interest::READ, Mode::Level);
    handle
        .insert_source(source, |_, listener, app: &mut App| {
            while let Ok((stream, _)) = listener.accept() {
                if let Some(line) = ipc::read_request(&stream) {
                    let reply = app.command(&line);
                    ipc::reply(&stream, &reply);
                }
            }
            Ok(PostAction::Continue)
        })
        .expect("ipc socket");
}

// -- Wayland ---------------------------------------------------------------

impl CompositorHandler for App {
    fn scale_factor_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        surface: &wl_surface::WlSurface,
        factor: i32,
    ) {
        if let Some(index) = self.index_of(surface) {
            self.surfaces[index].scale = factor.max(1);
            self.draw(index);
        }
    }

    fn transform_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: wl_output::Transform,
    ) {
    }

    fn frame(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: u32) {}

    fn surface_enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }

    fn surface_leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }
}

impl OutputHandler for App {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }

    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, output: wl_output::WlOutput) {
        self.add_output(&output);
    }

    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, output: wl_output::WlOutput) {
        if let Some(name) = self.output_state.info(&output).and_then(|info| info.name) {
            for surface in self.surfaces.iter_mut().filter(|s| s.output == output) {
                surface.output_name = name.clone();
            }
        }
        self.draw_all();
    }

    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, output: wl_output::WlOutput) {
        if self.surfaces.iter().any(|s| s.output == output && s.role != Role::Bar) {
            self.close();
        }
        self.surfaces.retain(|s| s.output != output);
    }
}

impl LayerShellHandler for App {
    fn closed(&mut self, _: &Connection, _: &QueueHandle<Self>, layer: &LayerSurface) {
        if self.surfaces.iter().any(|s| &s.layer == layer && s.role != Role::Bar) {
            self.close();
        }
        self.surfaces.retain(|s| &s.layer != layer);
    }

    fn configure(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        layer: &LayerSurface,
        configure: LayerSurfaceConfigure,
        _: u32,
    ) {
        let pixel = self.pixel();
        let Some(index) = self.surfaces.iter().position(|s| &s.layer == layer) else { return };
        let surface = &mut self.surfaces[index];
        let (w, h) = configure.new_size;
        if w > 0 {
            surface.logical.0 = w as i32;
        }
        if h > 0 {
            surface.logical.1 = h as i32;
        }
        // As many whole shell pixels as fit.
        surface.width = (surface.logical.0 as f32 / pixel).floor() as i32;
        surface.height = (surface.logical.1 as f32 / pixel).floor() as i32;
        surface.configured = true;
        self.draw(index);
    }
}

impl SeatHandler for App {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seat_state
    }

    fn new_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}

    fn new_capability(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        seat: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Pointer && self.pointer.is_none() {
            self.pointer = self.seat_state.get_pointer(qh, &seat).ok();
            if let (Some(pointer), Some(shapes)) = (&self.pointer, &self.cursor_shapes) {
                self.cursor = Some(shapes.get_shape_device(pointer, qh));
            }
        }
        if capability == Capability::Keyboard && !self.keyboard.bound() {
            self.keyboard.bind(&seat, qh);
        }
    }

    fn remove_capability(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat, capability: Capability) {
        if capability == Capability::Pointer
            && let Some(pointer) = self.pointer.take()
        {
            if let Some(cursor) = self.cursor.take() {
                cursor.destroy();
            }
            pointer.release();
        }
        if capability == Capability::Keyboard {
            self.keyboard.release();
        }
    }

    fn remove_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
}

impl PointerHandler for App {
    fn pointer_frame(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_pointer::WlPointer,
        events: &[PointerEvent],
    ) {
        let pixel = self.pixel() as f64;
        for event in events {
            let Some(index) = self.index_of(&event.surface) else { continue };
            let (x, y) = (event.position.0 / pixel, event.position.1 / pixel);
            let role = self.surfaces[index].role;
            // A floating popup is drawn inside a larger surface: its widgets want
            // positions of their own, a drag the surface's.
            let raw = (x, y);
            let (x, y) = match (role, self.floating) {
                (Role::Popup(kind), Some(g)) if kind.placement() == Placement::Floating => {
                    (x - g.x as f64, y - g.y as f64)
                }
                _ => (x, y),
            };
            match event.kind {
                PointerEventKind::Enter { serial } => {
                    self.enter_serial = serial;
                    self.surfaces[index].input.pointer = Some((x, y));
                    self.set_cursor(self.cursor_at(index));
                    self.schedule(index);
                }
                PointerEventKind::Motion { .. } => {
                    if self.drag.is_some() {
                        self.drag_to(index, raw);
                        continue;
                    }
                    let before = self.cursor_at(index);
                    self.surfaces[index].input.pointer = Some((x, y));
                    let after = self.cursor_at(index);
                    if before != after {
                        self.set_cursor(after);
                    }
                    match role {
                        // Hover in a popup is cheap, and sliders drag.
                        Role::Popup(_) => self.redraw_popup(),
                        Role::Shot => {
                            let output = self.surfaces[index].output_name.clone();
                            self.shot.motion(&output, x as i32, y as i32);
                            self.schedule(index);
                        }
                        Role::Bar | Role::Toasts | Role::Picker => self.schedule(index),
                        Role::Catcher | Role::Osd | Role::Desktop | Role::Wallpaper => {}
                    }
                }
                PointerEventKind::Leave { .. } => {
                    let input = &mut self.surfaces[index].input;
                    input.pointer = None;
                    input.held = false;
                    self.schedule(index);
                }
                PointerEventKind::Press { button, .. } if role == Role::Picker => {
                    let device = (self.surfaces[index].scale as f32 * self.pixel()) as f64;
                    let picked = (button == BTN_LEFT)
                        .then(|| {
                            let output = &self.surfaces[index].output_name;
                            let capture = self.capture.iter().find(|c| &c.output == output)?;
                            picker::at(capture, (x * device) as i32, (y * device) as i32)
                        })
                        .flatten();
                    self.close_picker();
                    if let Some(color) = picked {
                        let hex = picker::hex(color);
                        clipboard::copy_bytes(hex.as_bytes());
                        self.started = std::time::Instant::now() - Duration::from_secs(10);
                        self.osd(icons::PICKER, None, hex);
                    }
                }
                PointerEventKind::Press { button: BTN_LEFT, .. } if role == Role::Shot => {
                    let output = self.surfaces[index].output_name.clone();
                    let on_toolbar =
                        self.shot.toolbar.as_ref().is_some_and(|(o, rect)| *o == output && rect.contains(x, y));
                    if on_toolbar {
                        // The toolbar's buttons claim it as they are drawn.
                        self.surfaces[index].input.click = Some((x, y));
                    } else {
                        self.shot.press(&output, x as i32, y as i32, &self.theme);
                    }
                    // A region begun here is gone from any other screen.
                    for i in 0..self.surfaces.len() {
                        if self.surfaces[i].role == Role::Shot {
                            self.schedule(i);
                        }
                    }
                }
                PointerEventKind::Release { button: BTN_LEFT, .. } if role == Role::Shot => {
                    let surface = &self.surfaces[index];
                    self.shot.release((surface.width, surface.height));
                    self.schedule(index);
                }
                PointerEventKind::Press { button, .. } if role == Role::Bar => {
                    let surface = &self.surfaces[index];
                    let hit = surface
                        .hits
                        .iter()
                        .find(|hit| hit.rect.contains(x, y))
                        .map(|hit| (hit.action.clone(), hit.rect));
                    let output = surface.output_name.clone();
                    if let Some((action, rect)) = hit {
                        self.act_with(action, &output, button, Some(rect.x + rect.w / 2));
                    }
                }
                PointerEventKind::Press { button: BTN_LEFT, .. } => match role {
                    Role::Catcher => self.close(),
                    Role::Bar | Role::Osd | Role::Desktop | Role::Picker | Role::Wallpaper | Role::Shot => {}
                    Role::Popup(kind) => {
                        if kind.placement() == Placement::Floating
                            && let Some(drag) = self.drag_at(index, x, y, raw)
                        {
                            self.drag = Some(drag);
                            self.set_cursor(Shape::Grabbing);
                            continue;
                        }
                        let input = &mut self.surfaces[index].input;
                        input.click = Some((x, y));
                        input.held = true;
                        self.redraw_popup();
                    }
                    Role::Toasts => {
                        self.surfaces[index].input.click = Some((x, y));
                        self.draw(index);
                    }
                },
                PointerEventKind::Release { button: BTN_LEFT, .. } => {
                    if self.drag.take().is_some() {
                        self.save_window();
                        self.set_cursor(self.cursor_at(index));
                    }
                    self.surfaces[index].input.held = false;
                    // A popup may be holding something to drop where it is let go.
                    if matches!(role, Role::Popup(_)) {
                        self.redraw_popup();
                    }
                }
                PointerEventKind::Axis { vertical, .. } => {
                    let delta = if vertical.discrete != 0 { vertical.discrete as f64 } else { vertical.absolute };
                    if delta == 0.0 {
                        continue;
                    }
                    let step = if delta > 0.0 { 1 } else { -1 };
                    match role {
                        Role::Bar => {
                            let surface = &self.surfaces[index];
                            let tray =
                                surface.hits.iter().find(|hit| hit.rect.contains(x, y)).and_then(|hit| {
                                    match &hit.action {
                                        Action::Tray(key, _) => Some(key.clone()),
                                        _ => None,
                                    }
                                });
                            if let Some(key) = tray {
                                let _ = self.handles.tray.send(services::tray::Command::Scroll(key, step));
                            } else if surface.workspaces.is_some_and(|rect| rect.contains(x, y)) {
                                self.cycle(index, step);
                            }
                        }
                        Role::Popup(_) => {
                            let input = &mut self.surfaces[index].input;
                            input.scroll += step;
                            // A notch is three lines of the font; a touchpad reports its own distance.
                            input.pixels += if vertical.discrete != 0 {
                                vertical.discrete as f64 * 48.0
                            } else {
                                vertical.absolute / pixel
                            };
                            self.redraw_popup();
                        }
                        Role::Catcher
                        | Role::Toasts
                        | Role::Osd
                        | Role::Desktop
                        | Role::Picker
                        | Role::Wallpaper
                        | Role::Shot => {}
                    }
                }
                _ => {}
            }
        }
    }
}

impl ShmHandler for App {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

impl ProvidesRegistryState for App {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }

    registry_handlers![OutputState, SeatState];
}

delegate_registry!(App);
// Every other protocol object dispatches through its own user data since 0.21.
smithay_client_toolkit::delegate_dispatch2!(App);

/// Screen pixels to a shell pixel, as configured, within reason.
/// glibc's allocator, set for a program that lives for days and now and then
/// needs a screen-sized buffer for a moment: a decoded wallpaper, a frozen
/// screen, the two pictures of a fade. Left to itself, glibc moves such a
/// buffer onto the heap once one has been freed, and a heap does not shrink
/// past whatever small allocation came after it: the shell would keep the
/// largest moment it ever had. Fixed at 128 KiB, the threshold sends every
/// large buffer to its own mapping, which goes back to the system when
/// freed. Two arenas, not eight per core: the service threads allocate little.
fn tune_malloc() {
    #[cfg(target_env = "gnu")]
    {
        unsafe extern "C" {
            fn mallopt(param: i32, value: i32) -> i32;
        }
        const M_MMAP_THRESHOLD: i32 = -3;
        const M_ARENA_MAX: i32 = -8;
        // SAFETY: plain settings of the allocator, made before any thread starts.
        unsafe {
            mallopt(M_MMAP_THRESHOLD, 128 * 1024);
            mallopt(M_ARENA_MAX, 2);
        }
    }
}

fn pixel_of(config: &Config) -> f32 {
    let pixel = config.theme.pixel;
    if pixel.is_finite() { pixel.clamp(1.0, 8.0) } else { 1.0 }
}

/// Shell pixels in logical pixels, rounded to whole ones.
fn to_logical(v: i32, pixel: f32) -> i32 {
    (v as f32 * pixel).round() as i32
}

/// The theme as configured, with the accent taken from the wallpaper when
/// that is asked for and the theme names none.
fn theme_for(config: &Config) -> Theme {
    let mut theme = config.theme.clone();
    if config.wallpaper.accent
        && theme.accent.is_none()
        && let Some(file) = config.wallpaper.file(&config.theme)
    {
        theme.accent = wallpaper::accent(&file);
    }
    Theme::new(&theme)
}
