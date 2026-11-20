//! Settings: the config, edited in place and put to use as it changes.
//!
//! The file is kept as a JSON tree and every control writes only its own key
//! into it, so whatever else the file holds -- Tessera's keys Smalta
//! ignores, settings with no control here -- comes through untouched, and a
//! key never touched is never added. What the controls show is read back
//! through [`Config`], the same way the shell reads it, so a default shows
//! as what it means.
//!
//! Each change is written out at once and the shell applies it without a
//! restart. Text applies on Enter. The few settings the shell only reads at
//! start (the weather's place, the notification server, the tray) restart
//! it, and say so.
//!
//! A config Home Manager wrote is a link into the Nix store: it is shown,
//! and nothing is changed, since the next switch would undo it anyway.

use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Instant;

use serde_json::{Map, Value, json};

use crate::canvas::{Color, Rect};
use crate::config::{self, Config};
use crate::i18n::{russian, tr};
use crate::icons;
use crate::ui::{Align, Field, Key, Look, Ui};

use super::{Ctx, Outcome, Request};

/// When the shell started, for the about page.
pub static STARTED: OnceLock<Instant> = OnceLock::new();

/// Every bar module, in the order they are offered.
const MODULES: [&str; 16] = [
    "launcher",
    "clipboard",
    "workspaces",
    "focused-window",
    "clock",
    "media",
    "weather",
    "tray",
    "privacy",
    "screencast",
    "recording",
    "resources",
    "keyboard-layout",
    "notifications",
    "system-indicators",
    "control-center",
];

/// Accents to pick from; `None` keeps the shell grey.
const ACCENTS: [Option<&str>; 8] = [
    None,
    Some("#e0607e"),
    Some("#e07a4f"),
    Some("#d9b45a"),
    Some("#7fb069"),
    Some("#5aa9c9"),
    Some("#7d8fe0"),
    Some("#b07de0"),
];

pub const WIDTH: i32 = 820;
const SIDEBAR: i32 = 168;

#[derive(Clone, Copy, Default, PartialEq)]
enum Page {
    #[default]
    Theme,
    Wallpaper,
    Bar,
    Modules,
    ControlCenter,
    Desktop,
    Notifications,
    System,
    About,
}

impl Page {
    fn label(self) -> &'static str {
        tr(match self {
            Page::Theme => "Theme",
            Page::Wallpaper => "Wallpaper",
            Page::Bar => "Bar",
            Page::Modules => "Modules",
            Page::ControlCenter => "Control center",
            Page::Desktop => "Widgets",
            Page::Notifications => "Notifications",
            Page::System => "System",
            Page::About => "About",
        })
    }
}

/// What the launcher finds after `?`: a setting's name, and its page.
const TOPICS: [(&str, Page); 41] = [
    ("Pixelate", Page::Wallpaper),
    ("Visualizer", Page::Desktop),
    ("Wallpaper", Page::Wallpaper),
    ("Accent from the wallpaper", Page::Wallpaper),
    ("Dithering", Page::Wallpaper),
    ("Shades", Page::Wallpaper),
    ("Darker", Page::Wallpaper),
    ("Icons in the theme's tones", Page::Theme),
    ("Screen sharing", Page::System),
    ("Language", Page::Theme),
    ("Pixel", Page::Theme),
    ("Accent", Page::Theme),
    ("Screen edge", Page::Bar),
    ("Height", Page::Bar),
    ("Floating", Page::Bar),
    ("Width, %", Page::Bar),
    ("Margin", Page::Bar),
    ("Start edge", Page::Bar),
    ("End edge", Page::Bar),
    ("Between modules", Page::Bar),
    ("Clock", Page::Bar),
    ("Bar modules", Page::Modules),
    ("Tiles", Page::ControlCenter),
    ("Control center", Page::ControlCenter),
    ("Desktop widgets", Page::Desktop),
    ("Graphs", Page::Desktop),
    ("Quiet while sharing", Page::Notifications),
    ("Critical stay", Page::Notifications),
    ("Time on screen", Page::Notifications),
    ("Pause under the pointer", Page::Notifications),
    ("On screen at once", Page::Notifications),
    ("Kept in the list", Page::Notifications),
    ("Corner", Page::Notifications),
    ("OSD", Page::Notifications),
    ("Notification server", Page::Notifications),
    ("Tray", Page::Notifications),
    ("Weather", Page::System),
    ("Units", Page::System),
    ("Config file", Page::System),
    ("Memory", Page::About),
    ("Commands", Page::About),
];

/// Settings by name, for the launcher: each as its name, its page's name and
/// what to pass to [`Settings::go`].
pub fn topics() -> Vec<(String, String, usize)> {
    TOPICS.iter().enumerate().map(|(i, (name, page))| (tr(name).to_owned(), page.label().to_owned(), i)).collect()
}

/// The sidebar: groups of pages.
const GROUPS: [(&str, &[Page]); 3] = [
    ("Appearance", &[Page::Theme, Page::Wallpaper]),
    ("Desktop", &[Page::Bar, Page::Modules, Page::ControlCenter, Page::Desktop]),
    ("System", &[Page::Notifications, Page::System, Page::About]),
];

#[derive(Clone, Copy, PartialEq)]
enum Focus {
    Clock,
    City,
    Wallpaper,
}

/// A module being dragged: from which column, at which place.
#[derive(Clone, Copy)]
struct Dragging {
    column: usize,
    index: usize,
}

#[derive(Default)]
pub struct Settings {
    /// The whole file, so a write keeps what is not edited here.
    doc: Value,
    /// `doc` as the shell reads it.
    current: Config,
    path: PathBuf,
    /// Written by Home Manager: shown, not changed.
    managed: bool,
    page: Page,
    clock: Field,
    city: Field,
    wallpaper: Field,
    /// The wallpaper as the current settings make it, small, and the
    /// settings it was made for.
    preview: Option<(String, Option<crate::services::tray::Pixmap>)>,
    focus: Option<Focus>,
    error: Option<String>,
    /// How far the page is scrolled, and how tall it was last drawn.
    offset: i32,
    content_h: i32,
    dragging: Option<Dragging>,
    /// The column whose "add" list is open.
    adding: Option<usize>,
}

fn field(text: &str) -> Field {
    Field { text: text.to_owned(), cursor: text.chars().count() }
}

impl Settings {
    /// Straight to the page of one of [`topics`].
    pub fn go(&mut self, topic: usize) {
        if let Some((_, page)) = TOPICS.get(topic) {
            self.page = *page;
            self.offset = 0;
        }
    }

    pub fn opened(&mut self) {
        let dir = config::dir();
        self.path = dir.join("smalta/config.json");
        // Smalta's own file, or Tessera's the first time, or nothing.
        let source = [self.path.clone(), dir.join("tessera/config.json")]
            .into_iter()
            .find_map(|p| std::fs::read_to_string(p).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()));
        self.doc = source.filter(Value::is_object).unwrap_or_else(|| json!({}));
        self.managed = std::fs::read_link(&self.path).is_ok_and(|target| target.starts_with("/nix/store"));
        self.read_back();
        self.clock = field(&self.current.clock.format);
        self.city = field(&self.current.weather.location);
        self.wallpaper = field(&self.current.wallpaper.path);
        self.focus = None;
        self.error = None;
        self.offset = 0;
        self.dragging = None;
        self.adding = None;
    }

    fn read_back(&mut self) {
        self.current = serde_json::from_value(self.doc.clone()).unwrap_or_default();
    }

    /// One key changed, written out, and the shell told to use it.
    fn edit(&mut self, pointer: &str, value: Value, cx: &mut Ctx) {
        if self.managed {
            return;
        }
        set(&mut self.doc, pointer, value);
        self.read_back();
        match self.write() {
            Ok(()) => {
                self.error = None;
                cx.requests.push(Request::ApplyConfig);
            }
            Err(error) => self.error = Some(error),
        }
    }

    fn write(&self) -> Result<(), String> {
        let text = serde_json::to_string_pretty(&self.doc).map_err(|e| e.to_string())?;
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        std::fs::write(&self.path, text + "\n").map_err(|e| format!("{}: {e}", self.path.display()))
    }

    /// The text being typed, applied.
    fn commit(&mut self, cx: &mut Ctx) {
        match self.focus.take() {
            Some(Focus::Clock) if self.clock.text != self.current.clock.format => {
                let text = self.clock.text.clone();
                self.edit("/clock/format", json!(text), cx);
            }
            Some(Focus::City) if self.city.text.trim() != self.current.weather.location => {
                let text = self.city.text.trim().to_owned();
                self.edit("/weather/location", json!(text), cx);
            }
            Some(Focus::Wallpaper) if self.wallpaper.text.trim() != self.current.wallpaper.path => {
                let text = self.wallpaper.text.trim().to_owned();
                self.edit("/wallpaper/path", json!(text), cx);
            }
            _ => {}
        }
    }

    pub fn key(&mut self, key: &Key, cx: &mut Ctx) -> Outcome {
        match key {
            Key::Escape if self.focus.is_some() => {
                // Back to what it was.
                self.clock = field(&self.current.clock.format);
                self.city = field(&self.current.weather.location);
                self.wallpaper = field(&self.current.wallpaper.path);
                self.focus = None;
            }
            Key::Escape => return Outcome::Close,
            Key::Enter => self.commit(cx),
            _ => match self.focus {
                Some(Focus::Clock) => _ = self.clock.key(key),
                Some(Focus::City) => _ = self.city.key(key),
                Some(Focus::Wallpaper) => _ = self.wallpaper.key(key),
                None => return Outcome::Ignored,
            },
        }
        Outcome::Redraw
    }

    pub fn draw(&mut self, ui: &mut Ui, area: Rect, cx: &mut Ctx) -> i32 {
        let t = ui.theme;
        let line = ui.line();

        // The title row: what this is, which shell, how big, and a way out.
        let (head, rest) = area.take_top(line + 4, 8);
        let (close, head) = head.take_right(line + 4, 8);
        if ui.icon_button(close, icons::CROSS, Look::Flat) {
            cx.requests.push(Request::Close);
        }
        let (gear, head) = head.take_left(10, 8);
        ui.sprite(gear, icons::GEAR, t.accent);
        let title = tr("Settings").to_lowercase();
        let (name, head) = head.take_left(ui.text_width(&title), 8);
        ui.text(name, &title, t.fg, Align::Left);
        let about = format!("smalta {} · {}", env!("CARGO_PKG_VERSION"), memory());
        ui.text(head, &about, t.dim, Align::Left);
        ui.fill(Rect::new(rest.x - 12, rest.y - 4, rest.w + 24, 1), t.border);

        // The page list on the left, the page on the right.
        let (side, body) = rest.take_left(SIDEBAR, 0);
        ui.fill(Rect::new(side.x + side.w, side.y - 4, 1, side.h + 16), t.border);
        self.sidebar(ui, side);
        let mut body = Rect::new(body.x + 16, body.y + 4, body.w - 16, body.h - 4);

        // A note across the top when nothing can be changed or a write failed.
        let note = match (&self.error, self.managed) {
            (Some(error), _) => Some(error.clone()),
            (None, true) => Some(tr("Set by Home Manager: programs.smalta.settings").to_owned()),
            _ => None,
        };
        if let Some(note) = note {
            let (row, rest) = body.take_top(line + 8, 8);
            ui.frame(row, t.urgent);
            ui.text(row.inset(4), &note, t.urgent, Align::Left);
            body = rest;
        }

        // The page scrolls; a click outside it is not for a control hidden
        // past its edge.
        self.offset += ui.scrolled_pixels(body);
        self.offset = self.offset.clamp(0, (self.content_h - body.h).max(0));
        let outside = ui.input.click.filter(|&(x, y)| !body.contains(x, y));
        if outside.is_some() {
            ui.input.click = None;
        }
        ui.canvas.clip = Some(body);
        let mut flow = Flow { x: body.x, y: body.y - self.offset, w: body.w - 8, top: body.y - self.offset };
        match self.page {
            Page::Theme => self.theme(ui, &mut flow, cx),
            Page::Wallpaper => self.wallpaper_page(ui, &mut flow, cx),
            Page::Bar => self.bar(ui, &mut flow, cx),
            Page::Modules => self.modules(ui, &mut flow, cx),
            Page::ControlCenter => self.control_center(ui, &mut flow, cx),
            Page::Desktop => self.desktop(ui, &mut flow, cx),
            Page::Notifications => self.notifications(ui, &mut flow, cx),
            Page::System => self.system(ui, &mut flow, cx),
            Page::About => self.about(ui, &mut flow, cx),
        }
        self.content_h = flow.y - flow.top;
        ui.canvas.clip = None;
        if outside.is_some() {
            ui.input.click = outside;
        }
        // How far down, in the gutter, when it scrolls.
        if self.content_h > body.h {
            let thumb = (body.h * body.h / self.content_h).max(8);
            let at = body.y + self.offset * (body.h - thumb) / (self.content_h - body.h).max(1);
            ui.fill(Rect::new(body.x + body.w - 2, at, 2, thumb), t.accent);
        }
        area.h
    }

    fn sidebar(&mut self, ui: &mut Ui, side: Rect) {
        let t = ui.theme;
        let line = ui.line();
        let mut y = side.y;
        for (group, pages) in GROUPS {
            ui.fill(Rect::new(side.x, y + line / 2 - 2, 4, 4), t.accent);
            ui.text(Rect::new(side.x + 10, y, side.w - 22, line), &tr(group).to_lowercase(), t.dim, Align::Left);
            y += line + 4;
            for &page in pages {
                let row = Rect::new(side.x + 4, y, side.w - 16, line + 6);
                let chosen = page == self.page;
                if chosen {
                    ui.fill(row, t.accent);
                    ui.fill(Rect::new(side.x - 8, row.y, 2, row.h), t.accent);
                } else if ui.hovered(row) {
                    ui.fill(row, t.hover);
                }
                let ink = if chosen { t.on_accent } else { t.fg };
                ui.text(row, &page.label().to_lowercase(), ink, Align::Center);
                if ui.clicked(row) && !chosen {
                    self.page = page;
                    self.offset = 0;
                    self.adding = None;
                }
                y += line + 8;
            }
            y += 8;
        }
    }

    // -- pages ------------------------------------------------------------------

    fn theme(&mut self, ui: &mut Ui, flow: &mut Flow, cx: &mut Ctx) {
        let line = ui.line();
        heading(ui, flow, tr("Interface"));
        let row = flow.row(line + 10);
        let (left, right) = halves(row);
        let languages = [("auto", tr("Auto")), ("ru", "русский"), ("en", "english")];
        let at = languages.iter().position(|l| l.0 == self.current.interface.language).unwrap_or(0);
        if choice(ui, left, tr("Language"), languages[at].1) {
            let next = languages[(at + 1) % languages.len()].0;
            self.edit("/interface/language", json!(next), cx);
        }
        // Whole steps stay square; the ones between are allowed, their
        // pixels alternating a screen pixel wider and narrower.
        const SCALES: [f32; 7] = [1.0, 1.25, 1.5, 1.75, 2.0, 2.5, 3.0];
        let pixel = self.current.theme.pixel;
        let at = SCALES
            .iter()
            .position(|s| (s - pixel).abs() < 0.01)
            .unwrap_or_else(|| SCALES.iter().position(|s| *s > pixel).unwrap_or(SCALES.len()).saturating_sub(1));
        if let Some(step) = stepper(ui, right, tr("Pixel"), &format!("{pixel}x")) {
            let next = (at as i32 + step).clamp(0, SCALES.len() as i32 - 1) as usize;
            self.edit("/theme/pixel", json!(SCALES[next]), cx);
        }

        heading(ui, flow, tr("Accent"));
        let row = flow.row(line + 10);
        let mut swatches: Vec<Option<String>> = ACCENTS.iter().map(|a| a.map(str::to_owned)).collect();
        // A colour typed into the file by hand stays offered.
        if !swatches.contains(&self.current.theme.accent) {
            swatches.push(self.current.theme.accent.clone());
        }
        let t = ui.theme;
        let size = row.h;
        for (i, swatch) in swatches.iter().enumerate() {
            let cell = Rect::new(row.x + i as i32 * (size + 6), row.y, size, size);
            let chosen = *swatch == self.current.theme.accent;
            ui.frame(cell, if chosen { t.fg } else { t.border });
            let inner = cell.inset(if chosen { 3 } else { 2 });
            match swatch.as_deref().and_then(Color::parse) {
                Some(color) => ui.fill(inner, color),
                // No accent: the grey the shell is without one, half-toned.
                None => ui.canvas.dither(inner, t.dim),
            }
            if ui.clicked(cell) && !chosen {
                self.edit("/theme/accent", swatch.as_ref().map_or(Value::Null, |a| json!(a)), cx);
            }
        }

        heading(ui, flow, tr("Icons"));
        let row = flow.row(line * 2 + 4);
        let tint = self.current.theme.tint_icons;
        if check(
            ui,
            halves(row).0,
            tint,
            tr("Icons in the theme's tones"),
            tr("app and tray icons, from the background to the accent"),
        ) {
            self.edit("/theme/tintIcons", json!(!tint), cx);
        }
    }

    fn wallpaper_page(&mut self, ui: &mut Ui, flow: &mut Flow, cx: &mut Ctx) {
        let t = ui.theme;
        let line = ui.line();
        let w = self.current.wallpaper.clone();
        heading(ui, flow, tr("Wallpaper"));
        let row = flow.row(line * 2 + 4);
        let (left, right) = halves(row);
        if check(ui, left, w.enabled, tr("Draw the wallpaper"), tr("over one another program draws")) {
            self.edit("/wallpaper/enabled", json!(!w.enabled), cx);
        }
        if check(ui, right, w.accent, tr("Accent from the wallpaper"), tr("when the theme picks none")) {
            self.edit("/wallpaper/accent", json!(!w.accent), cx);
        }
        let row = flow.row(line + 10);
        let (left, right) = halves(row);
        let fallback = self.current.theme.wallpaper.clone().unwrap_or_default();
        let placeholder = if fallback.is_empty() { tr("path to a picture") } else { tr("Tessera's, or a path") };
        self.text_field(ui, left, Focus::Wallpaper, placeholder, cx);
        let file = w.file(&self.current.theme).unwrap_or_default();
        let name = file.rsplit('/').next().unwrap_or("").to_owned();
        ui.text(right, &name, t.dim, Align::Left);

        heading(ui, flow, tr("Pixels"));
        let row = flow.row(line * 2 + 4);
        let hint = tr("big pixels and a few shades; off, the picture as it is");
        if check(ui, halves(row).0, w.pixelate, tr("Pixelate"), hint) {
            self.edit("/wallpaper/pixelate", json!(!w.pixelate), cx);
        }
        // Cells, shades and dithering mean nothing to a picture left as it is.
        if w.pixelate {
            let row = flow.row(line + 10);
            let (left, right) = halves(row);
            if let Some(step) = stepper(ui, left, tr("Cell"), &w.cell.to_string()) {
                self.edit("/wallpaper/cell", json!((w.cell as i32 + step).clamp(1, 16)), cx);
            }
            if let Some(step) = stepper(ui, right, tr("Shades"), &w.levels.to_string()) {
                self.edit("/wallpaper/levels", json!((w.levels as i32 + step).clamp(2, 16)), cx);
            }
        }
        let row = flow.row(line * 2 + 6);
        let (left, right) = halves(row);
        if w.pixelate && check(ui, left, w.dither, tr("Dithering"), tr("a pattern between shades, not bands")) {
            self.edit("/wallpaper/dither", json!(!w.dither), cx);
        }
        let dim = format!("{}%", (w.dim * 100.0).round());
        if let Some(v) = level(ui, right, tr("Darker"), &dim, w.dim / 0.9) {
            self.edit("/wallpaper/dim", json!((v * 18.0).round() / 20.0), cx);
        }
        let row = flow.row(line + 10);
        let (left, _) = halves(row);
        let tints = [("none", tr("as it is")), ("mono", tr("greys")), ("duotone", tr("through the accent"))];
        let at = tints.iter().position(|x| x.0 == w.tint).unwrap_or(0);
        if choice(ui, left, tr("Colour"), tints[at].1) {
            self.edit("/wallpaper/tint", json!(tints[(at + 1) % tints.len()].0), cx);
        }

        // What it will look like, made again only when a setting changes.
        heading(ui, flow, tr("Preview"));
        let (pw, ph) = (320, 180);
        // Read again here: a control above may have just changed it, and the
        // picture must be made and drawn from the same settings.
        let w = self.current.wallpaper.clone();
        let file = w.file(&self.current.theme).unwrap_or_default();
        let key = format!("{w:?}{file}{:?}", t.accent);
        if self.preview.as_ref().is_none_or(|(k, _)| *k != key) {
            let image = (!file.is_empty()).then(|| crate::wallpaper::render(&w, t, &file, pw, ph)).flatten();
            self.preview = Some((key, image));
        }
        let frame = flow.row(ph / 2 + 2);
        let frame = Rect::new(frame.x, frame.y, pw / 2 + 2, ph / 2 + 2);
        ui.frame(frame, t.border);
        if let Some((_, Some(image))) = &self.preview {
            // Shown at half size: one shell pixel to two of a full screen's.
            let cell = w.cell() as i32;
            for py in 0..ph / 2 {
                for px in 0..pw / 2 {
                    let (sx, sy) = ((px * 2 / cell).min(image.width - 1), (py * 2 / cell).min(image.height - 1));
                    let color = crate::canvas::Color(image.pixels[(sy * image.width + sx) as usize]);
                    ui.fill(Rect::new(frame.x + 1 + px, frame.y + 1 + py, 1, 1), color);
                }
            }
        } else {
            ui.text(frame, tr("no picture"), t.dim, Align::Center);
        }
    }

    fn bar(&mut self, ui: &mut Ui, flow: &mut Flow, cx: &mut Ctx) {
        let line = ui.line();
        let bar = self.current.bar.clone();
        heading(ui, flow, tr("Placement"));
        let row = flow.row(line + 10);
        let (left, right) = halves(row);
        let bottom = bar.position != "top";
        if choice(ui, left, tr("Screen edge"), &if bottom { tr("Bottom") } else { tr("Top") }.to_lowercase()) {
            self.edit("/bar/position", json!(if bottom { "top" } else { "bottom" }), cx);
        }
        if let Some(step) = stepper(ui, right, tr("Height"), &bar.size.to_string()) {
            self.edit("/bar/size", json!((bar.size as i32 + step * 2).clamp(20, 64)), cx);
        }

        heading(ui, flow, tr("Shape"));
        let row = flow.row(line * 2 + 4);
        let (left, right) = halves(row);
        if check(ui, left, bar.floating, tr("Floating"), tr("off the edge, framed, with a shadow")) {
            self.edit("/bar/floating", json!(!bar.floating), cx);
        }
        let width = Rect::new(right.x, right.y, right.w, line + 10);
        if let Some(step) = stepper(ui, width, tr("Width, %"), &bar.width.to_string()) {
            self.edit("/bar/width", json!((bar.width as i32 + step * 5).clamp(20, 100)), cx);
        }
        let row = flow.row(line + 10);
        let (left, _) = halves(row);
        if let Some(step) = stepper(ui, left, tr("Margin"), &bar.margin.to_string()) {
            self.edit("/bar/margin", json!((bar.margin as i32 + step * 2).clamp(0, 64)), cx);
        }

        heading(ui, flow, tr("Spacing"));
        let row = flow.row(line + 10);
        let (left, right) = halves(row);
        if let Some(step) = stepper(ui, left, tr("Start edge"), &bar.edge_start.to_string()) {
            self.edit("/bar/edgeStart", json!((bar.edge_start as i32 + step * 2).clamp(0, 64)), cx);
        }
        if let Some(step) = stepper(ui, right, tr("End edge"), &bar.edge_end.to_string()) {
            self.edit("/bar/edgeEnd", json!((bar.edge_end as i32 + step * 2).clamp(0, 64)), cx);
        }
        let row = flow.row(line + 10);
        let (left, _) = halves(row);
        if let Some(step) = stepper(ui, left, tr("Between modules"), &bar.gap.to_string()) {
            self.edit("/bar/gap", json!((bar.gap as i32 + step).clamp(0, 32)), cx);
        }

        heading(ui, flow, tr("Clock"));
        let row = flow.row(line + 10);
        let (left, right) = halves(row);
        self.text_field(ui, left, Focus::Clock, "%H:%M", cx);
        ui.text(right, tr("Enter applies · %a is the weekday"), ui.theme.dim, Align::Left);
    }

    fn modules(&mut self, ui: &mut Ui, flow: &mut Flow, cx: &mut Ctx) {
        let t = ui.theme;
        let line = ui.line();
        heading(ui, flow, tr("Bar modules"));
        let modules = &self.current.bar.modules;
        let mut columns = [modules.start.clone(), modules.center.clone(), modules.end.clone()];
        let names = [tr("Start"), tr("Center"), tr("End")];
        let defaults = config::Modules::default();
        let default_columns = [defaults.start, defaults.center, defaults.end];
        let width = (flow.w - 16) / 3;
        let item_h = line + 8;
        let top = flow.y;
        let first = top + line + 14;
        let mut changed = false;
        let mut bottom = top;
        let mut drop_at: Option<(usize, usize)> = None;
        let pointer = ui.input.pointer;
        let held = ui.input.held;
        let shown = columns.clone();

        for (c, column) in shown.iter().enumerate() {
            let x = flow.x + c as i32 * (width + 8);
            let head = Rect::new(x + 8, top + 6, width - 16, line);
            let reset = tr("reset");
            let (reset_rect, head) = head.take_right(ui.text_width(reset) + 8, 4);
            ui.text(head, &names[c].to_lowercase(), t.fg, Align::Left);
            if ui.button(reset_rect, reset, Look::Flat) && *column != default_columns[c] {
                columns[c] = default_columns[c].clone();
                changed = true;
            }
            let mut y = first;
            for (i, name) in column.iter().enumerate() {
                let row = Rect::new(x + 6, y, width - 12, item_h);
                let lifted = self.dragging.is_some_and(|d| d.column == c && d.index == i);
                ui.frame(row, if lifted { t.accent } else { t.border });
                let (remove, rest) = row.take_right(item_h, 0);
                let (grip, rest) = rest.take_left(14, 2);
                ui.text(grip, "⠿", t.dim, Align::Center);
                ui.text(rest, &module_name(name), if lifted { t.dim } else { t.fg }, Align::Left);
                if ui.icon_button(remove, icons::CROSS, Look::Flat) {
                    columns[c].retain(|m| m != name);
                    changed = true;
                } else if ui.clicked(row) {
                    self.dragging = Some(Dragging { column: c, index: i });
                    self.adding = None;
                }
                y += item_h + 4;
            }
            // Where a dragged module would land in this column.
            if let (Some(_), Some((px, py))) = (self.dragging, pointer)
                && px >= x as f64
                && px < (x + width) as f64
            {
                let index =
                    (0..column.len()).filter(|&i| (first + i as i32 * (item_h + 4) + item_h / 2) as f64 <= py).count();
                drop_at = Some((c, index));
                let mark = first + index as i32 * (item_h + 4) - 3;
                ui.fill(Rect::new(x + 6, mark, width - 12, 2), t.accent);
            }
            let add = Rect::new(x + 6, y, width - 12, item_h);
            let label = format!("+ {}", tr("add"));
            if ui.button(add, &label, Look::Flat) {
                self.adding = if self.adding == Some(c) { None } else { Some(c) };
            }
            y += item_h + 4;
            // What is not on the bar yet, to pick from.
            if self.adding == Some(c) {
                for name in MODULES.iter().filter(|m| !shown.iter().flatten().any(|u| u == *m)) {
                    let row = Rect::new(x + 6, y, width - 12, item_h);
                    if ui.hovered(row) {
                        ui.fill(row, t.hover);
                    }
                    ui.text(row.inset(4), &module_name(name), t.dim, Align::Left);
                    if ui.clicked(row) {
                        columns[c].push(name.to_string());
                        changed = true;
                    }
                    y += item_h;
                }
                y += 4;
            }
            ui.frame(Rect::new(x, top, width, y + 4 - top), t.border);
            bottom = bottom.max(y + 4);
        }

        // The module under the pointer, following it until it is let go.
        if let Some(dragging) = self.dragging {
            match pointer {
                Some((px, py)) if held => {
                    let ghost = Rect::new(px as i32 - 20, py as i32 - item_h / 2, width - 12, item_h);
                    ui.fill(ghost, t.bg);
                    ui.frame(ghost, t.accent);
                    let name = module_name(&shown[dragging.column][dragging.index]);
                    ui.text(ghost.inset(4), &name, t.fg, Align::Left);
                }
                _ => {
                    self.dragging = None;
                    if let Some((column, mut index)) = drop_at
                        && !held
                    {
                        let name = columns[dragging.column].remove(dragging.index);
                        if column == dragging.column && index > dragging.index {
                            index -= 1;
                        }
                        let index = index.min(columns[column].len());
                        columns[column].insert(index, name);
                        changed = columns != shown;
                    }
                }
            }
        }
        if changed {
            let modules = json!({ "start": columns[0], "center": columns[1], "end": columns[2] });
            self.edit("/bar/modules", modules, cx);
        }
        flow.y = bottom + 12;
        let row = flow.row(line);
        ui.text(row, tr("Drag to reorder or to move between sections"), t.dim, Align::Left);
    }

    fn control_center(&mut self, ui: &mut Ui, flow: &mut Flow, cx: &mut Ctx) {
        let line = ui.line();
        heading(ui, flow, tr("Tiles"));
        let tiles = self.current.control_center.tiles.clone();
        for pair in config::TILES.chunks(2) {
            let row = flow.row(line + 4);
            let (left, right) = halves(row);
            for (rect, tile) in [left, right].into_iter().zip(pair) {
                let on = tiles.iter().any(|t| t == tile);
                if check(ui, rect, on, &tile_name(tile), "") {
                    // Kept in the order they are drawn.
                    let next: Vec<&str> = config::TILES
                        .iter()
                        .copied()
                        .filter(|t| if t == tile { !on } else { tiles.iter().any(|x| x == t) })
                        .collect();
                    self.edit("/controlCenter/tiles", json!(next), cx);
                }
            }
        }
    }

    fn desktop(&mut self, ui: &mut Ui, flow: &mut Flow, cx: &mut Ctx) {
        let line = ui.line();
        let d = &self.current.desktop;
        let (enabled, date, calendar, graphs, media) =
            (d.enabled, d.show_date, d.show_calendar, d.show_graphs, d.show_media);
        heading(ui, flow, tr("Desktop widgets"));
        let row = flow.row(line * 2 + 4);
        if check(ui, halves(row).0, enabled, tr("Widgets"), tr("clock, calendar and graphs on the wallpaper")) {
            self.edit("/desktop/enabled", json!(!enabled), cx);
        }
        let items = [
            ("/desktop/showDate", date, tr("Date")),
            ("/desktop/showCalendar", calendar, tr("Calendar")),
            ("/desktop/showGraphs", graphs, tr("Graphs")),
            ("/desktop/showMedia", media, tr("What is playing")),
        ];
        for pair in items.chunks(2) {
            let row = flow.row(line + 4);
            let (left, right) = halves(row);
            for (rect, (pointer, on, label)) in [left, right].into_iter().zip(pair) {
                if check(ui, rect, *on, label, "") {
                    self.edit(pointer, json!(!on), cx);
                }
            }
        }

        let visualizer = self.current.media.visualizer;
        heading(ui, flow, tr("Music"));
        let row = flow.row(line * 2 + 4);
        let hint = tr("bars by the song on the bar, through cava");
        if check(ui, halves(row).0, visualizer, tr("Visualizer"), hint) {
            self.edit("/media/visualizer", json!(!visualizer), cx);
        }
    }

    fn notifications(&mut self, ui: &mut Ui, flow: &mut Flow, cx: &mut Ctx) {
        let line = ui.line();
        let n = self.current.notifications.clone();
        let osd = self.current.osd.clone();

        heading(ui, flow, tr("When to show"));
        let row = flow.row(line * 2 + 4);
        let (left, right) = halves(row);
        let hint = tr("straight to the list while the screen is shared");
        if check(ui, left, n.quiet_when_sharing, tr("Quiet while sharing"), hint) {
            self.edit("/notifications/quietWhenSharing", json!(!n.quiet_when_sharing), cx);
        }
        if check(ui, right, n.critical_sticky, tr("Critical stay"), tr("until closed, not timed out")) {
            self.edit("/notifications/criticalSticky", json!(!n.critical_sticky), cx);
        }

        heading(ui, flow, tr("How long"));
        let row = flow.row(line * 2 + 6);
        let (left, right) = halves(row);
        let value = format!("{} {}", n.timeout, tr("s"));
        if let Some(v) = level(ui, left, tr("Time on screen"), &value, n.timeout as f32 / 30.0) {
            self.edit("/notifications/timeout", json!(((v * 30.0).round() as u32).max(1)), cx);
        }
        if check(ui, right, n.pause_on_hover, tr("Pause under the pointer"), tr("a toast stays while pointed at")) {
            self.edit("/notifications/pauseOnHover", json!(!n.pause_on_hover), cx);
        }
        let row = flow.row(line * 2 + 4);
        let (left, _) = halves(row);
        if check(ui, left, n.app_timeout, tr("Time from the app"), tr("when an app asks for its own, up to a minute")) {
            self.edit("/notifications/appTimeout", json!(!n.app_timeout), cx);
        }

        heading(ui, flow, tr("Toasts"));
        let row = flow.row(line + 10);
        let (left, right) = halves(row);
        if let Some(step) = stepper(ui, left, tr("On screen at once"), &n.max_toasts.to_string()) {
            self.edit("/notifications/maxToasts", json!((n.max_toasts as i32 + step).clamp(1, 8)), cx);
        }
        if let Some(step) = stepper(ui, right, tr("Kept in the list"), &n.history.to_string()) {
            self.edit("/notifications/history", json!((n.history as i32 + step * 10).clamp(10, 500)), cx);
        }
        let row = flow.row(line + 10);
        let (left, _) = halves(row);
        let corners = [
            ("auto", tr("by the bar")),
            ("top-right", tr("top right")),
            ("top-left", tr("top left")),
            ("bottom-right", tr("bottom right")),
            ("bottom-left", tr("bottom left")),
        ];
        let at = corners.iter().position(|c| c.0 == n.position).unwrap_or(0);
        if choice(ui, left, tr("Corner"), corners[at].1) {
            self.edit("/notifications/position", json!(corners[(at + 1) % corners.len()].0), cx);
        }

        heading(ui, flow, "OSD");
        let kinds = [
            ("/osd/volume", osd.volume, tr("Volume")),
            ("/osd/microphone", osd.microphone, tr("Microphone")),
            ("/osd/brightness", osd.brightness, tr("Brightness")),
            ("/osd/layout", osd.layout, tr("Layout")),
            ("/osd/locks", osd.locks, "Caps / Num Lock"),
        ];
        for pair in kinds.chunks(2) {
            let row = flow.row(line + 4);
            let (left, right) = halves(row);
            for (rect, (pointer, on, label)) in [left, right].into_iter().zip(pair) {
                if check(ui, rect, *on, label, "") {
                    self.edit(pointer, json!(!on), cx);
                }
            }
        }
        let row = flow.row(line * 2 + 6);
        let (left, right) = halves(row);
        let seconds = osd.timeout as f32 / 1000.0;
        let value = format!("{seconds:.1} {}", tr("s"));
        if let Some(v) = level(ui, left, tr("Time on screen"), &value, seconds / 5.0) {
            self.edit("/osd/timeout", json!(((v * 50.0).round() as u32).max(3) * 100), cx);
        }
        let places =
            [("auto", tr("by the bar")), ("top", tr("Top")), ("center", tr("center")), ("bottom", tr("Bottom"))];
        let at = places.iter().position(|p| p.0 == osd.position).unwrap_or(0);
        let place = Rect::new(right.x, right.y, right.w, line + 10);
        if choice(ui, place, tr("Place"), &places[at].1.to_lowercase()) {
            self.edit("/osd/position", json!(places[(at + 1) % places.len()].0), cx);
        }
        let row = flow.row(line + 10);
        let (left, _) = halves(row);
        if ui.button(left, &tr("Show").to_lowercase(), Look::Framed) {
            cx.requests.push(Request::PreviewOsd);
        }

        heading(ui, flow, tr("Services"));
        let row = flow.row(line * 2 + 4);
        let (left, right) = halves(row);
        let tray = self.current.tray.enabled;
        if check(ui, left, n.server, tr("Notification server"), tr("off for mako or dunst · restarts")) {
            self.edit("/notifications/server", json!(!n.server), cx);
        }
        if check(ui, right, tray, tr("Tray"), tr("off when another program hosts it · restarts")) {
            self.edit("/tray/enabled", json!(!tray), cx);
        }
    }

    fn system(&mut self, ui: &mut Ui, flow: &mut Flow, cx: &mut Ctx) {
        let line = ui.line();
        heading(ui, flow, tr("Weather"));
        let row = flow.row(line + 10);
        let (left, right) = halves(row);
        self.text_field(ui, left, Focus::City, tr("Moscow"), cx);
        let imperial = self.current.weather.units == "imperial";
        if choice(ui, right, tr("Units"), if imperial { "°F" } else { "°C" }) {
            self.edit("/weather/units", json!(if imperial { "metric" } else { "imperial" }), cx);
        }
        let row = flow.row(line);
        ui.text(row, tr("Enter applies · weather settings restart the shell"), ui.theme.dim, Align::Left);

        heading(ui, flow, tr("Screen sharing"));
        let row = flow.row(line);
        ui.text(
            row,
            tr("Ticked apps are a black box in a shared screen and in screenshots"),
            ui.theme.dim,
            Align::Left,
        );
        // What is open now, and whatever is ticked though closed.
        let hide = self.current.screenshare.hide.clone();
        let mut apps: Vec<String> = cx.snapshot.apps.clone();
        for app in &hide {
            if !apps.iter().any(|a| a.eq_ignore_ascii_case(app)) {
                apps.push(app.clone());
            }
        }
        apps.sort_by_key(|a| a.to_lowercase());
        for pair in apps.chunks(2) {
            let row = flow.row(line + 4);
            let (left, right) = halves(row);
            for (rect, app) in [left, right].into_iter().zip(pair) {
                let on = hide.iter().any(|h| h.eq_ignore_ascii_case(app));
                if check(ui, rect, on, app, "") {
                    let next: Vec<&String> = if on {
                        hide.iter().filter(|h| !h.eq_ignore_ascii_case(app)).collect()
                    } else {
                        hide.iter().chain(std::iter::once(app)).collect()
                    };
                    self.edit("/screenshare/hide", json!(next), cx);
                }
            }
        }
        let row = flow.row(line);
        let hint = format!("{} smalta toggle-hidden", tr("One window by hand:"));
        ui.text(row, &hint, ui.theme.dim, Align::Left);

        heading(ui, flow, tr("Quick"));
        let row = flow.row(line + 12);
        let cells = row.columns(4, 8);
        if ui.button(cells[0], &tr("Lock").to_lowercase(), Look::Framed) {
            cx.requests.push(Request::Lock);
        }
        if ui.button(cells[1], &tr("Mixer").to_lowercase(), Look::Framed) {
            cx.requests.push(Request::Mixer);
        }
        if ui.button(cells[2], &tr("Reread").to_lowercase(), Look::Framed) {
            cx.requests.push(Request::ApplyConfig);
        }
        if ui.button(cells[3], &tr("Config file").to_lowercase(), Look::Framed) {
            if !self.path.exists() {
                let _ = self.write();
            }
            cx.requests.push(Request::Spawn(vec!["xdg-open".into(), self.path.display().to_string()]));
        }
        let row = flow.row(line);
        ui.text(row, &format!("{} smalta reload", tr("or:")), ui.theme.dim, Align::Left);
    }

    fn about(&mut self, ui: &mut Ui, flow: &mut Flow, cx: &mut Ctx) {
        let t = ui.theme;
        let line = ui.line();
        let row = flow.row(line * 2);
        ui.canvas.text_big(ui.face, row.x, row.y, "SMALTA", t.accent, 2);
        let row = flow.row(line);
        ui.text(row, &format!("smalta {}", env!("CARGO_PKG_VERSION")), t.dim, Align::Left);
        flow.y += 8;
        let uptime = STARTED.get().map_or(0, |s| s.elapsed().as_secs());
        let running = format!("{} {} {} {}", uptime / 3600, tr("h"), uptime / 60 % 60, tr("min"));
        let facts = [
            (tr("Memory"), memory()),
            (tr("Running"), running),
            (tr("Notifications in the list"), cx.notifications.list.len().to_string()),
            (tr("Config"), self.path.display().to_string()),
            (tr("Commands"), "smalta toggle|show|close|reload|quit".to_owned()),
        ];
        for (name, value) in facts {
            let row = flow.row(line + 2);
            let (label, rest) = row.take_left(220, 8);
            ui.text(label, &name.to_lowercase(), t.dim, Align::Left);
            ui.text(rest, &value, t.fg, Align::Left);
        }
    }

    /// A text entry that applies on Enter or when another is clicked.
    fn text_field(&mut self, ui: &mut Ui, rect: Rect, which: Focus, placeholder: &str, cx: &mut Ctx) {
        let focused = self.focus == Some(which);
        let field = match which {
            Focus::Clock => &self.clock,
            Focus::City => &self.city,
            Focus::Wallpaper => &self.wallpaper,
        };
        ui.field(rect, field, placeholder, focused, false);
        if ui.clicked(rect) && !focused {
            self.commit(cx);
            self.focus = Some(which);
        }
    }
}

/// Where the next row of a page goes.
struct Flow {
    x: i32,
    y: i32,
    w: i32,
    /// Where the page began, to know how tall it came out.
    top: i32,
}

impl Flow {
    fn row(&mut self, h: i32) -> Rect {
        let rect = Rect::new(self.x, self.y, self.w, h);
        self.y += h + 8;
        rect
    }
}

fn halves(row: Rect) -> (Rect, Rect) {
    let w = (row.w - 16) / 2;
    (Rect::new(row.x, row.y, w, row.h), Rect::new(row.x + w + 16, row.y, row.w - w - 16, row.h))
}

/// A section's name with a square of the accent before it and a rule under it.
fn heading(ui: &mut Ui, flow: &mut Flow, text: &str) {
    let t = ui.theme;
    let line = ui.line();
    if flow.y > flow.top {
        flow.y += 6;
    }
    let row = flow.row(line);
    ui.fill(Rect::new(row.x, row.y + line / 2 - 2, 4, 4), t.accent);
    ui.text(Rect::new(row.x + 10, row.y, row.w - 10, line), &text.to_lowercase(), t.fg, Align::Left);
    ui.fill(Rect::new(row.x, row.y + line + 3, row.w, 1), t.border);
    flow.y += 2;
}

/// A box to tick, its name beside it and a quieter line under that.
fn check(ui: &mut Ui, rect: Rect, on: bool, label: &str, hint: &str) -> bool {
    let t = ui.theme;
    let line = ui.line();
    let area = Rect::new(rect.x, rect.y, rect.w, if hint.is_empty() { line } else { line * 2 + 2 });
    let mark = Rect::new(rect.x, rect.y + (line - 10) / 2, 10, 10);
    ui.frame(mark, if on { t.accent } else { t.border });
    if on {
        ui.fill(mark.inset(2), t.accent);
    }
    let text = Rect::new(rect.x + 18, rect.y, rect.w - 18, line);
    ui.text(text, &label.to_lowercase(), if ui.hovered(area) { t.accent } else { t.fg }, Align::Left);
    if !hint.is_empty() {
        ui.text(Rect::new(text.x, text.y + line + 2, text.w, line), hint, t.dim, Align::Left);
    }
    ui.clicked(area)
}

/// `name … ← value →`: a number stepped either way.
fn stepper(ui: &mut Ui, rect: Rect, label: &str, value: &str) -> Option<i32> {
    let t = ui.theme;
    ui.frame(rect, t.border);
    let inner = Rect::new(rect.x + 8, rect.y + 2, rect.w - 10, rect.h - 4);
    let (next, inner) = inner.take_right(inner.h, 0);
    let (shown, inner) = inner.take_right(ui.text_width(value).max(24) + 8, 0);
    let (previous, inner) = inner.take_right(inner.h, 0);
    ui.text(inner, &label.to_lowercase(), t.dim, Align::Left);
    ui.text(shown, value, t.fg, Align::Center);
    let mut step = None;
    if ui.icon_button(previous, icons::CHEVRON_LEFT, Look::Flat) {
        step = Some(-1);
    }
    if ui.icon_button(next, icons::CHEVRON, Look::Flat) {
        step = Some(1);
    }
    step
}

/// `name … value →`: a click takes the next of a few choices.
fn choice(ui: &mut Ui, rect: Rect, label: &str, value: &str) -> bool {
    let t = ui.theme;
    ui.frame(rect, if ui.hovered(rect) { t.fg } else { t.border });
    let inner = Rect::new(rect.x + 8, rect.y, rect.w - 12, rect.h);
    let (arrow, inner) = inner.take_right(8, 6);
    ui.sprite(arrow, icons::CHEVRON, t.accent);
    let (shown, inner) = inner.take_right(ui.text_width(value), 8);
    ui.text(shown, value, t.fg, Align::Right);
    ui.text(inner, &label.to_lowercase(), t.dim, Align::Left);
    ui.clicked(rect)
}

/// A name and its value over a row of cells to click or drag along.
fn level(ui: &mut Ui, rect: Rect, label: &str, value: &str, fraction: f32) -> Option<f32> {
    let t = ui.theme;
    let line = ui.line();
    let (top, track) = rect.take_top(line, 4);
    let (shown, name) = top.take_right(ui.text_width(value), 8);
    ui.text(name, &label.to_lowercase(), t.fg, Align::Left);
    ui.text(shown, value, t.accent, Align::Right);
    ui.slider(Rect::new(track.x, track.y, track.w, 10), fraction.clamp(0.0, 1.0))
}

/// `value` at a JSON pointer, making the objects on the way; `null`
/// removes the key instead.
fn set(doc: &mut Value, pointer: &str, value: Value) {
    let keys: Vec<&str> = pointer.trim_start_matches('/').split('/').collect();
    let (last, path) = keys.split_last().expect("a pointer names a key");
    let mut node = doc;
    for key in path {
        if !node.get(*key).is_some_and(Value::is_object) {
            node[*key] = Value::Object(Map::new());
        }
        node = &mut node[*key];
    }
    if let Some(object) = node.as_object_mut() {
        if value.is_null() {
            object.remove(*last);
        } else {
            object.insert((*last).to_owned(), value);
        }
    }
}

/// The shell's resident memory, from `/proc`.
fn memory() -> String {
    let pages = std::fs::read_to_string("/proc/self/statm")
        .ok()
        .and_then(|s| s.split_whitespace().nth(1).and_then(|p| p.parse::<u64>().ok()))
        .unwrap_or(0);
    let mib = pages * 4096 / (1024 * 1024);
    format!("{mib} {}", if russian() { "МБ" } else { "MB" })
}

fn module_name(id: &str) -> String {
    let (en, ru) = match id {
        "launcher" => ("launcher", "лаунчер"),
        "clipboard" => ("clipboard", "буфер"),
        "workspaces" => ("workspaces", "столы"),
        "focused-window" => ("window title", "заголовок окна"),
        "clock" => ("clock", "часы"),
        "media" => ("music", "музыка"),
        "weather" => ("weather", "погода"),
        "tray" => ("tray", "трей"),
        "privacy" => ("mic or camera on", "микрофон и камера"),
        "screencast" => ("screen shared", "показ экрана"),
        "recording" => ("recording", "запись"),
        "resources" => ("load", "нагрузка"),
        "keyboard-layout" => ("layout", "раскладка"),
        "notifications" => ("notifications", "уведомления"),
        "system-indicators" => ("indicators", "индикаторы"),
        "control-center" => ("control center", "центр управления"),
        other => (other, other),
    };
    if russian() { ru } else { en }.to_owned()
}

fn tile_name(id: &str) -> String {
    let (en, ru) = match id {
        "wifi" => ("Wi-Fi", "Wi-Fi"),
        "bluetooth" => ("Bluetooth", "Bluetooth"),
        "dnd" => ("do not disturb", "не беспокоить"),
        "profiles" => ("power profiles", "профили питания"),
        "volume" => ("volume", "громкость"),
        "microphone" => ("microphone", "микрофон"),
        "brightness" => ("brightness", "яркость"),
        "actions" => ("screenshot and recording", "скриншот и запись"),
        "media" => ("what is playing", "что играет"),
        other => (other, other),
    };
    if russian() { ru } else { en }.to_owned()
}
