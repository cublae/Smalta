//! What a bar shows, and where: laid out fresh on every draw.
//!
//! Modules turn into pieces -- a button, a line of text, a meter -- which are
//! measured first and then placed: the start section from the left edge, the
//! end section from the right, the centre one in the middle.

use chrono::Local;

use crate::canvas::{Canvas, Color, Rect};
use crate::compositor::{Snapshot, Workspace};
use crate::config::Config;
use crate::font::{self, Face};
use crate::icons::{self, Icon};
use crate::popups::Kind;
use crate::resources::Resources;
use crate::services::{Services, tray};
use crate::theme::Theme;

#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    FocusWorkspace(i64),
    SwitchLayout,
    Toggle(Kind),
    /// A tray item: its key, and whether a click means its menu.
    Tray(String, bool),
    PlayPause,
    StopRecording,
}

/// A clickable region from the last draw.
#[derive(Clone, Debug)]
pub struct Hit {
    pub rect: Rect,
    pub action: Action,
}

pub struct Frame {
    pub hits: Vec<Hit>,
    /// Where scrolling cycles workspaces.
    pub workspaces: Option<Rect>,
}

pub struct Context<'a> {
    pub config: &'a Config,
    pub theme: &'a Theme,
    pub face: &'a Face,
    pub snapshot: &'a Snapshot,
    pub resources: &'a Resources,
    pub output: &'a str,
    pub unread: bool,
    pub dnd: bool,
    pub services: &'a Services,
    /// Elapsed time while recording the screen.
    pub recording: Option<String>,
    /// The visualizer's latest levels; empty when it is not running.
    pub levels: &'a [u8],
}

/// One element of a button's content.
enum Part {
    Text(String),
    /// A sprite, in its own colour when given.
    Sprite(Icon, Option<Color>),
    Image(tray::Pixmap),
    /// Levels from the visualizer, 0 to its top, as columns of blocks.
    Levels(Vec<u8>),
}

enum Piece {
    Button {
        parts: Vec<Part>,
        action: Action,
        style: Style,
        /// A dot in the corner: something waiting.
        badge: bool,
        /// Room either side of the contents, inside the hover box.
        pad: i32,
    },
    /// Text that does nothing when clicked.
    Text(String),
    Meter {
        label: &'static str,
        value: f32,
    },
    /// Extra room between neighbours that would otherwise read as one.
    Space(i32),
    /// Marks where the workspace buttons start and end, for scrolling.
    GroupStart,
    GroupEnd,
}

#[derive(Clone, Copy, PartialEq)]
enum Style {
    Plain,
    /// Shown on its output, which does not have focus.
    Active,
    Focused,
    Urgent,
}

const PAD: i32 = 6;
/// Room inside a meter, between its label, its cells and its percentage.
const METER_GAP: i32 = 4;
/// A tray button's padding: half a normal one.
const TRAY_PAD: i32 = 3;
const CELLS: i32 = 6;
/// Tray icons, in shell pixels.
pub const ICON: i32 = 14;
/// Sprites on the bar are drawn at this many pixels to each of theirs: a
/// 7-pixel icon comes out as tall as a tray icon.
const SPRITE_MAG: i32 = 2;
/// Room between the parts of one button.
const PART_GAP: i32 = 5;
/// A visualizer column and the gap after it.
const LEVEL_STEP: i32 = 3;

fn button(parts: Vec<Part>, action: Action) -> Piece {
    Piece::Button { parts, action, style: Style::Plain, badge: false, pad: PAD }
}

fn sprite(icon: Icon) -> Part {
    Part::Sprite(icon, None)
}

impl Context<'_> {
    fn pieces(&self, ids: &[String]) -> Vec<Piece> {
        let t = self.theme;
        let s = self.services;
        let mut pieces = Vec::new();
        for id in ids {
            match id.as_str() {
                "launcher" => pieces.push(button(vec![sprite(icons::LAUNCHER)], Action::Toggle(Kind::Launcher))),
                "clipboard" => pieces.push(button(vec![sprite(icons::CLIPBOARD)], Action::Toggle(Kind::Clipboard))),
                "control-center" => pieces.push(button(vec![sprite(icons::GRID)], Action::Toggle(Kind::ControlCenter))),
                "workspaces" => {
                    pieces.push(Piece::GroupStart);
                    for w in self.shown_workspaces() {
                        let style = if w.focused {
                            Style::Focused
                        } else if w.urgent {
                            Style::Urgent
                        } else if w.active {
                            Style::Active
                        } else {
                            Style::Plain
                        };
                        pieces.push(Piece::Button {
                            parts: vec![Part::Text(w.label.clone())],
                            action: Action::FocusWorkspace(w.id),
                            style,
                            badge: false,
                            pad: PAD,
                        });
                    }
                    pieces.push(Piece::GroupEnd);
                }
                "clock" => pieces.push(button(
                    vec![Part::Text(clock_text(&self.config.clock.format))],
                    Action::Toggle(Kind::Calendar),
                )),
                "keyboard-layout" if self.snapshot.layouts > 1 => {
                    pieces.push(button(vec![Part::Text(layout_code(&self.snapshot.layout))], Action::SwitchLayout))
                }
                "focused-window" if !self.snapshot.title.is_empty() => {
                    pieces.push(Piece::Text(shorten(&self.snapshot.title, 60)));
                }
                "tray" => {
                    for (i, item) in s.tray.iter().enumerate() {
                        // Tray icons sit close, the way the indicators beside them do:
                        // a narrow box each, and no gap between the boxes.
                        if i > 0 {
                            pieces.push(Piece::Space(-self.gap()));
                        }
                        let part = match &item.icon {
                            Some(icon) => Part::Image(icon.clone()),
                            None => Part::Text(
                                item.title.chars().next().map(|c| c.to_uppercase().collect()).unwrap_or_default(),
                            ),
                        };
                        pieces.push(Piece::Button {
                            parts: vec![part],
                            action: Action::Tray(item.key.clone(), item.is_menu),
                            style: if item.needs_attention { Style::Urgent } else { Style::Plain },
                            badge: false,
                            pad: TRAY_PAD,
                        });
                    }
                }
                "notifications" => pieces.push(Piece::Button {
                    parts: vec![sprite(if self.dnd { icons::BELL_OFF } else { icons::BELL })],
                    action: Action::Toggle(Kind::Notifications),
                    style: Style::Plain,
                    badge: self.unread,
                    pad: PAD,
                }),
                "system-indicators" => {
                    let mut parts = Vec::new();
                    let net = &s.network;
                    if net.available {
                        parts.push(sprite(if net.wired {
                            icons::WIRED
                        } else if net.connected.is_some() {
                            icons::WIFI
                        } else {
                            icons::WIFI_OFF
                        }));
                    }
                    if let Some(speaker) = s.audio.speaker() {
                        parts.push(sprite(if speaker.muted { icons::SPEAKER_MUTED } else { icons::SPEAKER }));
                    }
                    if let Some(battery) = &s.power.battery {
                        let low = battery.low();
                        parts.push(Part::Sprite(icons::BATTERY, low.then_some(t.urgent)));
                        parts.push(Part::Text(format!("{}%", (battery.percent * 100.0).round())));
                    }
                    // Headphones, a mouse, a gamepad: whatever is connected and
                    // says how charged it is.
                    for device in s.bluetooth.connected() {
                        if let Some(level) = device.battery {
                            let low = level <= crate::services::bluetooth::LOW;
                            parts.push(Part::Sprite(icons::device(&device.icon), low.then_some(t.urgent)));
                            parts.push(Part::Text(format!("{level}%")));
                        }
                    }
                    if parts.is_empty() {
                        parts.push(sprite(icons::GRID));
                    }
                    pieces.push(button(parts, Action::Toggle(Kind::ControlCenter)));
                }
                "media" => {
                    if let Some(player) = s.media.current().filter(|p| !p.title.is_empty()) {
                        let text = if player.artist.is_empty() {
                            player.title.clone()
                        } else {
                            format!("{} — {}", player.title, player.artist)
                        };
                        let icon = if player.playing { icons::MUSIC } else { icons::PAUSE };
                        let mut parts = vec![sprite(icon)];
                        if player.playing && !self.levels.is_empty() {
                            parts.push(Part::Levels(self.levels.to_vec()));
                        }
                        parts.push(Part::Text(shorten(&text, 40)));
                        pieces.push(button(parts, Action::PlayPause));
                    }
                }
                "recording" => {
                    if let Some(elapsed) = &self.recording {
                        let parts = vec![Part::Sprite(icons::RECORD, Some(t.urgent)), Part::Text(elapsed.clone())];
                        pieces.push(button(parts, Action::StopRecording));
                    }
                }
                "screencast" if self.snapshot.casting => {
                    // With how many windows the viewers see as black boxes.
                    let mut parts = vec![Part::Sprite(icons::CAST, Some(t.urgent))];
                    if self.snapshot.hidden > 0 {
                        parts.push(Part::Text(self.snapshot.hidden.to_string()));
                    }
                    pieces.push(button(parts, Action::Toggle(Kind::ControlCenter)));
                }
                // Someone is listening or watching: a microphone, a camera, or both.
                "privacy" if !s.audio.recording.is_empty() || !s.camera.is_empty() => {
                    let mut parts = Vec::new();
                    if !s.audio.recording.is_empty() {
                        parts.push(Part::Sprite(icons::MIC, Some(t.urgent)));
                    }
                    if !s.camera.is_empty() {
                        parts.push(Part::Sprite(icons::CAMERA, Some(t.urgent)));
                    }
                    pieces.push(button(parts, Action::Toggle(Kind::ControlCenter)));
                }
                "weather" => {
                    if let Some(now) = &s.weather.current {
                        let parts = vec![
                            sprite(crate::services::weather::icon(now.code)),
                            Part::Text(format!("{}°", now.temperature.round())),
                        ];
                        pieces.push(button(parts, Action::Toggle(Kind::Weather)));
                    }
                }
                "resources" => {
                    pieces.push(Piece::Meter { label: "cpu", value: self.resources.cpu });
                    pieces.push(Piece::Space(6));
                    pieces.push(Piece::Meter { label: "ram", value: self.resources.memory });
                }
                // Not known, or nothing to show right now.
                _ => {}
            }
        }
        pieces
    }

    /// Workspaces on this bar: per output unless told otherwise, and only those
    /// holding windows plus whichever are on screen.
    pub fn shown_workspaces(&self) -> Vec<&Workspace> {
        let ws = &self.config.workspaces;
        self.snapshot
            .workspaces
            .iter()
            .filter(|w| !ws.per_monitor || w.output == self.output)
            .filter(|w| ws.show_empty || w.windows > 0 || w.active)
            .collect()
    }

    fn width(&self, piece: &Piece) -> i32 {
        match piece {
            Piece::Button { parts, pad, .. } => {
                let inner: i32 = parts.iter().map(|part| self.part_width(part) + PART_GAP).sum();
                inner - PART_GAP + pad * 2
            }
            Piece::Text(text) => font::text_width(self.face, text),
            Piece::Meter { label, .. } => {
                font::text_width(self.face, label)
                    + METER_GAP
                    + CELLS * 4
                    + METER_GAP
                    + font::text_width(self.face, "100%")
            }
            Piece::Space(w) => *w - self.gap(),
            Piece::GroupStart | Piece::GroupEnd => -self.gap(),
        }
    }

    fn part_width(&self, part: &Part) -> i32 {
        match part {
            Part::Text(text) => font::text_width(self.face, text),
            Part::Sprite(rows, _) => rows.iter().map(|r| r.chars().count() as i32).max().unwrap_or(0) * SPRITE_MAG,
            Part::Image(_) => ICON,
            Part::Levels(levels) => levels.len() as i32 * LEVEL_STEP - 1,
        }
    }

    /// Shortens the free text in `pieces` until the section fits `room`.
    fn squeeze(&self, pieces: &mut [Piece], room: i32) {
        let over = self.section_width(pieces) - room;
        if over <= 0 {
            return;
        }
        for piece in pieces.iter_mut() {
            if let Piece::Text(text) = piece {
                let target = (font::text_width(self.face, text) - over).max(0);
                let mut chars: Vec<char> = text.chars().collect();
                while !chars.is_empty()
                    && font::text_width(self.face, &(chars.iter().collect::<String>() + "…")) > target
                {
                    chars.pop();
                }
                *text = if chars.is_empty() { String::new() } else { chars.into_iter().collect::<String>() + "…" };
                return;
            }
        }
    }

    fn section_width(&self, pieces: &[Piece]) -> i32 {
        let total: i32 = pieces.iter().map(|p| self.width(p) + self.gap()).sum();
        (total - self.gap()).max(0)
    }
}

/// Two letters of the layout's language: "English (US)" reads "EN".
fn layout_code(name: &str) -> String {
    name.split_whitespace().next().unwrap_or("").chars().take(2).collect::<String>().to_uppercase()
}

fn shorten(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    text.chars().take(max - 1).collect::<String>().trim_end().to_owned() + "…"
}

pub fn draw(
    canvas: &mut Canvas,
    width: i32,
    height: i32,
    bottom: bool,
    cx: &Context,
    hover: Option<(f64, f64)>,
) -> Frame {
    let theme = cx.theme;
    let panel = panel(cx, width, height, bottom);
    if cx.config.bar.floating {
        // Clear around it, framed, with the popups' hard shadow.
        canvas.clear(Color(0));
        let shadow = crate::popups::SHADOW;
        canvas.fill(Rect { x: panel.x + shadow, y: panel.y + shadow, ..panel }, Color(0x9900_0000));
        canvas.fill(panel, theme.bg);
        canvas.frame(panel, theme.border);
    } else {
        canvas.clear(theme.bg);
        // The edge facing the screen.
        canvas.fill(Rect { x: 0, y: if bottom { 0 } else { height - 1 }, w: width, h: 1 }, theme.border);
    }

    let modules = &cx.config.bar.modules;
    let mut start = cx.pieces(&modules.start);
    let center = cx.pieces(&modules.center);
    let mut end = cx.pieces(&modules.end);

    // Free text -- a window title -- gives way before anything overlaps:
    // the side sections may reach the centre one and no further.
    let bar = &cx.config.bar;
    let (edge_start, edge_end) = (bar.edge_start.min(200) as i32, bar.edge_end.min(200) as i32);
    let center_width = cx.section_width(&center);
    let room = if center.is_empty() {
        (panel.w - edge_start - edge_end) / 2
    } else {
        (panel.w - center_width) / 2 - edge_start.max(edge_end) - cx.gap() * 2
    };
    cx.squeeze(&mut start, room);
    cx.squeeze(&mut end, room);

    let (top, inner) = (panel.y, panel.h);
    let mut frame = Frame { hits: Vec::new(), workspaces: None };
    place(canvas, cx, &start, panel.x + edge_start, top, inner, hover, &mut frame);
    place(canvas, cx, &center, panel.x + (panel.w - center_width) / 2, top, inner, hover, &mut frame);
    let end_x = panel.x + panel.w - edge_end - cx.section_width(&end);
    place(canvas, cx, &end, end_x, top, inner, hover, &mut frame);
    frame
}

#[allow(clippy::too_many_arguments)]
fn place(
    canvas: &mut Canvas,
    cx: &Context,
    pieces: &[Piece],
    mut x: i32,
    top: i32,
    height: i32,
    hover: Option<(f64, f64)>,
    frame: &mut Frame,
) {
    let theme = cx.theme;
    let face = cx.face;
    let line = face.line_height();
    let box_height = line + 4;
    let box_y = top + (height - box_height) / 2;
    let text_y = top + (height - line) / 2;
    let mut group_from = None;

    for piece in pieces {
        let w = cx.width(piece);
        match piece {
            Piece::GroupStart => group_from = Some(x),
            Piece::GroupEnd => {
                if let Some(from) = group_from.take() {
                    frame.workspaces = Some(Rect { x: from, y: top, w: (x - cx.gap() - from).max(1), h: height });
                }
            }
            Piece::Button { parts, action, style, badge, pad } => {
                let rect = Rect { x, y: box_y, w, h: box_height };
                let hovered = hover.is_some_and(|(px, py)| rect.contains(px, py));
                let (fill, edge, ink) = match style {
                    Style::Focused => (Some(theme.accent), Some(theme.accent), theme.on_accent),
                    Style::Urgent => (None, Some(theme.urgent), theme.urgent),
                    Style::Active => (None, Some(theme.fg), theme.fg),
                    Style::Plain => (None, None, theme.fg),
                };
                match (fill, hovered) {
                    (Some(fill), _) => canvas.fill(rect, fill),
                    (None, true) => canvas.fill(rect, theme.hover),
                    (None, false) => {}
                }
                match (edge, hovered) {
                    (Some(edge), _) => canvas.frame(rect, edge),
                    (None, true) => canvas.frame(rect, theme.border),
                    (None, false) => {}
                }
                let mut pen = x + pad;
                for part in parts {
                    match part {
                        Part::Text(text) => {
                            canvas.text(face, pen, text_y, text, ink);
                        }
                        Part::Sprite(rows, color) => {
                            let top = box_y + (box_height - rows.len() as i32 * SPRITE_MAG) / 2;
                            canvas.sprite_scaled(pen, top, rows, color.unwrap_or(ink), SPRITE_MAG);
                        }
                        Part::Image(image) => canvas.image(pen, box_y + (box_height - ICON) / 2, ICON, image),
                        Part::Levels(levels) => {
                            // Each step a block of two pixels, a pixel apart, from the bottom.
                            let room = (box_height - 4) / 3;
                            let floor = box_y + box_height - 2;
                            for (i, level) in levels.iter().enumerate() {
                                let lit = (*level as i32 * room + crate::visualizer::TOP as i32 - 1)
                                    / crate::visualizer::TOP as i32;
                                for step in 0..lit.max(1) {
                                    let y = floor - (step + 1) * 3 + 1;
                                    let color = if step == 0 { ink } else { cx.theme.accent };
                                    canvas.fill(Rect { x: pen + i as i32 * LEVEL_STEP, y, w: 2, h: 2 }, color);
                                }
                            }
                        }
                    }
                    pen += cx.part_width(part) + PART_GAP;
                }
                if *badge {
                    canvas.fill(Rect { x: rect.x + rect.w - 5, y: rect.y + 2, w: 3, h: 3 }, theme.accent);
                }
                frame.hits.push(Hit { rect, action: action.clone() });
            }
            Piece::Space(_) => {}
            Piece::Text(text) => {
                canvas.text(face, x, text_y, text, theme.dim);
            }
            Piece::Meter { label, value } => {
                let mut pen = x;
                pen += canvas.text(face, pen, text_y, label, theme.dim) + METER_GAP;
                // Segments rather than a bar: a glance reads a count of cells
                // faster than a length.
                let lit = (value.clamp(0.0, 1.0) * CELLS as f32).ceil() as i32;
                let cell_h = line - 6;
                for cell in 0..CELLS {
                    let color = if cell < lit { theme.fg } else { theme.faint };
                    canvas.fill(Rect { x: pen + cell * 4, y: text_y + 3, w: 3, h: cell_h }, color);
                }
                pen += CELLS * 4 + METER_GAP;
                let percent = format!("{:>3}%", (value * 100.0).round() as i32);
                canvas.text(face, pen, text_y, &percent, theme.dim);
            }
        }
        // Group markers measure minus a gap, so they take no room.
        x += w + cx.gap();
    }
}

/// The time in the configured strftime format, with `%a` as the interface
/// language's two-letter weekday rather than chrono's English one.
fn clock_text(format: &str) -> String {
    let now = Local::now();
    let weekday = crate::i18n::weekday_short(chrono::Datelike::weekday(&now).num_days_from_monday());
    now.format(&format.replace("%a", weekday)).to_string()
}

/// Where the bar itself is drawn on its surface: all of it, or for a
/// floating bar a narrower box off the screen's edge. The surface is that
/// much taller (see [`thickness`]); its spare room is transparent.
fn panel(cx: &Context, width: i32, height: i32, bottom: bool) -> Rect {
    let bar = &cx.config.bar;
    if !bar.floating {
        return Rect { x: 0, y: 0, w: width, h: height };
    }
    let size = bar.size as i32;
    let w = width * bar.width.clamp(20, 100) as i32 / 100;
    // Against the screen's edge sits the margin; on a top bar the shadow
    // needs its own room below.
    let y = if bottom { height - bar.margin.min(64) as i32 - size } else { bar.margin.min(64) as i32 };
    Rect { x: (width - w) / 2, y, w, h: size }
}

/// The bar surface's height: the bar, and for a floating one its margin and
/// the shadow under a top bar.
pub fn thickness(config: &Config, bottom: bool) -> i32 {
    let bar = &config.bar;
    let size = bar.size as i32;
    if !bar.floating {
        return size;
    }
    size + bar.margin.min(64) as i32 + if bottom { 0 } else { crate::popups::SHADOW }
}

impl Context<'_> {
    /// Room between modules.
    fn gap(&self) -> i32 {
        self.config.bar.gap.min(64) as i32
    }
}
