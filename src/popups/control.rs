//! The control center: toggles and sliders on the main page, a page each for
//! Wi-Fi, Bluetooth, the mixer and the session.

use crate::canvas::Rect;
use crate::i18n::tr;
use crate::icons;
use crate::services::{audio, bluetooth, media, network, power};
use crate::ui::{Align, Field, Key, Look, Ui};

use super::{Ctx, Kind, Outcome, Request};

#[derive(Clone, Copy, Default, PartialEq)]
pub enum Page {
    #[default]
    Main,
    Wifi,
    Bluetooth,
    Mixer,
    Session,
}

#[derive(Default)]
pub struct ControlCenter {
    pub page: Page,
    /// The network a password is being typed for, and whether it is hidden.
    asking: Option<(String, bool)>,
    password: Field,
    /// Name of a hidden network, typed before its password.
    hidden: Option<Field>,
}

const ROW: i32 = 22;

impl ControlCenter {
    pub fn opened(&mut self) {
        self.page = Page::Main;
        self.asking = None;
        self.hidden = None;
    }

    pub fn draw(&mut self, ui: &mut Ui, area: Rect, cx: &mut Ctx) -> i32 {
        let used = match self.page {
            Page::Main => self.main(ui, area, cx),
            Page::Wifi => self.wifi(ui, area, cx),
            Page::Bluetooth => self.bluetooth(ui, area, cx),
            Page::Mixer => self.mixer(ui, area, cx),
            Page::Session => self.session(ui, area, cx),
        };
        used - area.y
    }

    pub fn key(&mut self, key: &Key, cx: &mut Ctx) -> Outcome {
        if let Some(hidden) = &mut self.hidden {
            return match key {
                Key::Escape => {
                    self.hidden = None;
                    Outcome::Redraw
                }
                Key::Enter if !hidden.text.trim().is_empty() => {
                    self.asking = Some((hidden.text.trim().to_owned(), true));
                    self.password = Field::default();
                    self.hidden = None;
                    Outcome::Redraw
                }
                _ => {
                    hidden.key(key);
                    Outcome::Redraw
                }
            };
        }
        if let Some((ssid, hidden)) = self.asking.clone() {
            return match key {
                Key::Escape => {
                    self.asking = None;
                    Outcome::Redraw
                }
                Key::Enter => {
                    let password = Some(self.password.text.clone()).filter(|p| !p.is_empty());
                    let _ = cx.handles.network.send(network::Command::Connect { ssid, password, hidden });
                    self.asking = None;
                    self.password = Field::default();
                    Outcome::Redraw
                }
                _ => {
                    self.password.key(key);
                    Outcome::Redraw
                }
            };
        }
        match key {
            // Escape steps back to the main page before it closes anything.
            Key::Escape if self.page != Page::Main => {
                self.page = Page::Main;
                Outcome::Redraw
            }
            Key::Escape => Outcome::Close,
            _ => Outcome::Ignored,
        }
    }

    fn header(&mut self, ui: &mut Ui, area: Rect, title: &str) -> Rect {
        let (row, rest) = area.take_top(ROW, 8);
        let (back, row) = row.take_left(ROW, 6);
        if ui.icon_button(back, icons::BACK, Look::Flat) {
            self.page = Page::Main;
            self.asking = None;
            self.hidden = None;
        }
        ui.text(row, title, ui.theme.fg, Align::Left);
        rest
    }

    // -- main ----------------------------------------------------------------

    fn main(&mut self, ui: &mut Ui, area: Rect, cx: &mut Ctx) -> i32 {
        let t = ui.theme;
        let s = cx.services;

        // Battery on the left, the settings and session buttons on the right.
        let (top, rest) = area.take_top(ROW, 10);
        let (session, top) = top.take_right(ROW, 4);
        if ui.icon_button(session, icons::POWER, Look::Framed) {
            self.page = Page::Session;
        }
        let (settings, top) = top.take_right(ROW, 4);
        if ui.icon_button(settings, icons::GEAR, Look::Framed) {
            cx.requests.push(Request::Show(Kind::Settings));
        }
        if let Some(battery) = &s.power.battery {
            let (icon, top) = top.take_left(10, 6);
            ui.sprite(icon, icons::BATTERY, t.fg);
            let (cells, top) = top.take_left(40, 6);
            ui.cells(Rect::new(cells.x, cells.y + 7, cells.w, 8), battery.percent, 4, 1);
            let mut label =
                format!("{}%{}", (battery.percent * 100.0).round(), if battery.charging { " +" } else { "" });
            if let Some(left) = battery.left() {
                label = format!("{label} · {left}");
            }
            ui.text(top, &label, t.dim, Align::Left);
        }

        // Toggles with a page behind them.
        let mut y = rest;
        let tiles = &cx.config.control_center;
        let net = &s.network;
        if tiles.has("wifi") && net.available && (net.has_wifi || net.wired) {
            let (row, rest) = y.take_top(ROW, 2);
            y = rest;
            let detail = if net.wired {
                tr("Wired").to_owned()
            } else if let Some(busy) = &net.busy {
                format!("{busy}…")
            } else {
                net.connected.clone().unwrap_or_else(|| tr(if net.wifi_enabled { "Offline" } else { "Off" }).to_owned())
            };
            let (chevron, row) = row.take_right(ROW, 2);
            if ui.toggle(row, net.wifi_enabled, "wi-fi", &detail) {
                let _ = cx.handles.network.send(network::Command::SetWifi(!net.wifi_enabled));
            }
            if ui.icon_button(chevron, icons::CHEVRON, Look::Flat) {
                self.page = Page::Wifi;
                let _ = cx.handles.network.send(network::Command::Rescan);
            }
        }
        let bt = &s.bluetooth;
        if tiles.has("bluetooth") && bt.available {
            let (row, rest) = y.take_top(ROW, 2);
            y = rest;
            let connected = bt.connected();
            let detail = match connected.as_slice() {
                [] => tr(if bt.powered { "On" } else { "Off" }).to_owned(),
                [one] => match one.battery {
                    Some(level) => format!("{} {level}%", one.name),
                    None => one.name.clone(),
                },
                many => format!("{}", many.len()),
            };
            let (chevron, row) = row.take_right(ROW, 2);
            if ui.toggle(row, bt.powered, "bluetooth", &detail) {
                let _ = cx.handles.bluetooth.send(bluetooth::Command::Power(!bt.powered));
            }
            if ui.icon_button(chevron, icons::CHEVRON, Look::Flat) {
                self.page = Page::Bluetooth;
            }
        }
        if tiles.has("dnd") {
            let (row, rest) = y.take_top(ROW, 8);
            y = rest;
            if ui.toggle(row, cx.dnd, &tr("Do not disturb").to_lowercase(), "") {
                cx.requests.push(Request::ToggleDnd);
            }
        }

        // Power profiles as a segmented choice.
        if tiles.has("profiles") && !s.power.profiles.is_empty() {
            let (row, rest) = y.take_top(ROW, 10);
            y = rest;
            let profiles = &s.power.profiles;
            for (cell, name) in row.columns(profiles.len() as i32, 2).into_iter().zip(profiles) {
                let label = match name.as_str() {
                    "power-saver" => tr("Saver"),
                    "balanced" => tr("Balanced"),
                    "performance" => tr("Performance"),
                    other => other,
                };
                let on = s.power.profile.as_deref() == Some(name.as_str());
                if ui.button(cell, &label.to_lowercase(), if on { Look::On } else { Look::Framed }) && !on {
                    let _ = cx.handles.power.send(power::Command::Profile(name.clone()));
                }
            }
        }

        // Sliders.
        let audio = &s.audio;
        if let Some(speaker) = audio.speaker().filter(|_| tiles.has("volume")) {
            let (row, rest) = y.take_top(ROW, 4);
            y = rest;
            let icon = if speaker.muted { icons::SPEAKER_MUTED } else { icons::SPEAKER };
            if let Some(change) =
                slider_row(ui, row, icon, speaker.volume, speaker.muted, &mut self.page, Some(Page::Mixer))
            {
                match change {
                    Change::Mute => drop(cx.handles.audio.send(audio::Command::Mute(true))),
                    Change::Value(v) => drop(cx.handles.audio.send(audio::Command::Volume(true, v))),
                }
            }
        }
        if let Some(mic) = audio.microphone().filter(|_| tiles.has("microphone")) {
            let (row, rest) = y.take_top(ROW, 4);
            y = rest;
            let icon = if mic.muted { icons::MIC_MUTED } else { icons::MIC };
            if let Some(change) = slider_row(ui, row, icon, mic.volume, mic.muted, &mut self.page, None) {
                match change {
                    Change::Mute => drop(cx.handles.audio.send(audio::Command::Mute(false))),
                    Change::Value(v) => drop(cx.handles.audio.send(audio::Command::Volume(false, v))),
                }
            }
        }
        if let Some(level) = s.power.brightness.filter(|_| tiles.has("brightness")) {
            let (row, rest) = y.take_top(ROW, 4);
            y = rest;
            if let Some(Change::Value(v)) = slider_row(ui, row, icons::SUN, level, false, &mut self.page, None) {
                let _ = cx.handles.power.send(power::Command::Brightness(v));
            }
        }

        // One-shot actions.
        if tiles.has("actions") {
            let (row, rest) = y.take_top(ROW, 10);
            y = rest;
            let cells = row.columns(3, 4);
            if ui.button(cells[0], &tr("Screenshot").to_lowercase(), Look::Framed) {
                cx.requests.push(Request::Screenshot);
            }
            if ui.button(cells[1], &tr("Pick colour").to_lowercase(), Look::Framed) {
                cx.requests.push(Request::PickColor);
            }
            let recording = cx.recording;
            let label = if recording { tr("Stop recording") } else { tr("Record") };
            if ui.button(cells[2], &label.to_lowercase(), if recording { Look::On } else { Look::Framed }) {
                cx.requests.push(Request::ToggleRecording);
            }
            // The window behind this one, out of a shared screen or back in.
            let (row, rest) = y.take_top(ROW, 10);
            y = rest;
            let label = match cx.snapshot.hidden {
                0 => tr("Hide window from sharing").to_lowercase(),
                n => format!("{} · {} {n}", tr("Hide window from sharing").to_lowercase(), tr("hidden")),
            };
            if ui.button(row, &label, Look::Framed) {
                cx.requests.push(Request::ToggleHidden);
            }
        }

        // Whatever is playing.
        if let Some(player) = s.media.current().filter(|_| tiles.has("media")) {
            let line = ui.line();
            let (card, rest) = y.take_top(line * 3 + 24, 0);
            y = rest;
            let card = Rect::new(card.x, card.y + line / 2, card.w, card.h - line / 2);
            ui.panel(card, &player.identity.to_lowercase());
            let inner = Rect::new(card.x + 8, card.y + line / 2 + 2, card.w - 16, card.h - line / 2 - 8);
            let (title, inner) = inner.take_top(line, 0);
            let (controls, title) = title.take_right(3 * 18, 4);
            let text = if player.title.is_empty() { tr("Nothing playing").to_owned() } else { player.title.clone() };
            ui.text(title, &text, t.fg, Align::Left);
            let buttons = controls.columns(3, 0);
            let bus = player.bus.clone();
            if ui.icon_button(buttons[0], icons::PREV, Look::Flat) {
                let _ = cx.handles.media.send(media::Command::Previous(bus.clone()));
            }
            if ui.icon_button(buttons[1], if player.playing { icons::PAUSE } else { icons::PLAY }, Look::Flat) {
                let _ = cx.handles.media.send(media::Command::PlayPause(bus.clone()));
            }
            if ui.icon_button(buttons[2], icons::NEXT, Look::Flat) {
                let _ = cx.handles.media.send(media::Command::Next(bus));
            }
            let (artist, inner) = inner.take_top(line, 4);
            ui.text(artist, &player.artist, t.dim, Align::Left);
            if player.length > 0.0 {
                let (bar, _) = inner.take_top(4, 0);
                ui.cells(bar, (player.position / player.length) as f32, 2, 1);
            }
        }
        y.y
    }

    // -- pages -----------------------------------------------------------------

    fn wifi(&mut self, ui: &mut Ui, area: Rect, cx: &mut Ctx) -> i32 {
        let t = ui.theme;
        let net = &cx.services.network;
        let mut y = self.header(ui, area, "wi-fi");

        if let Some(error) = &net.error {
            let (row, rest) = y.take_top(ui.line(), 6);
            y = rest;
            ui.text(row, error, t.urgent, Align::Left);
        }

        if let Some((ssid, _)) = self.asking.clone() {
            let (label, rest) = y.take_top(ui.line(), 4);
            ui.text(label, &format!("{}: {ssid}", tr("Password")), t.dim, Align::Left);
            let (field, rest) = rest.take_top(ROW + 2, 6);
            ui.field(field, &self.password, "", true, true);
            let (row, rest) = rest.take_top(ROW, 10);
            y = rest;
            let (connect, _) = row.take_right(ui.text_width(tr("Connect")) + 16, 0);
            if ui.button(connect, &tr("Connect").to_lowercase(), Look::Framed) {
                let password = Some(self.password.text.clone()).filter(|p| !p.is_empty());
                let (ssid, hidden) = self.asking.take().unwrap_or_default();
                let _ = cx.handles.network.send(network::Command::Connect { ssid, password, hidden });
                self.password = Field::default();
            }
        } else if let Some(hidden) = &self.hidden {
            let (label, rest) = y.take_top(ui.line(), 4);
            ui.text(label, tr("Network name"), t.dim, Align::Left);
            let (field, rest) = rest.take_top(ROW + 2, 10);
            y = rest;
            ui.field(field, hidden, "", true, false);
        }

        let shown = net.networks.iter().take(12);
        for wifi in shown {
            let (row, rest) = y.take_top(ROW, 0);
            y = rest;
            let (forget, row) = if wifi.saved { row.take_right(ROW, 2) } else { (Rect::default(), row) };
            if wifi.saved && ui.icon_button(forget, icons::TRASH, Look::Flat) {
                let _ = cx.handles.network.send(network::Command::Forget(wifi.ssid.clone()));
            }
            let busy = net.busy.as_deref() == Some(wifi.ssid.as_str());
            let detail = format!("{}{}", if wifi.secure { "▪ " } else { "" }, bars(wifi.signal));
            let title = if busy { format!("{}…", wifi.ssid) } else { wifi.ssid.clone() };
            if ui.row(row, None, &title, &detail, wifi.active) {
                if wifi.active {
                    let _ = cx.handles.network.send(network::Command::Disconnect);
                } else if wifi.secure && !wifi.saved {
                    self.asking = Some((wifi.ssid.clone(), false));
                    self.password = Field::default();
                } else {
                    let ssid = wifi.ssid.clone();
                    let _ = cx.handles.network.send(network::Command::Connect { ssid, password: None, hidden: false });
                }
            }
        }
        if self.asking.is_none() && self.hidden.is_none() {
            let (row, rest) = y.take_top(ROW, 0);
            y = rest;
            if ui.row(row, Some(icons::PLUS), &tr("Hidden network").to_lowercase(), "", false) {
                self.hidden = Some(Field::default());
            }
        }
        y.y
    }

    fn bluetooth(&mut self, ui: &mut Ui, area: Rect, cx: &mut Ctx) -> i32 {
        let bt = &cx.services.bluetooth;
        let y = self.header(ui, area, "bluetooth");
        let (row, mut y) = y.take_top(ROW, 8);
        let (scan, row) = row.take_right(ui.text_width(tr("Scanning…")) + 16, 4);
        if ui.toggle(row, bt.powered, &tr("Power").to_lowercase(), "") {
            let _ = cx.handles.bluetooth.send(bluetooth::Command::Power(!bt.powered));
        }
        let label = if bt.discovering { tr("Scanning…") } else { "scan" };
        if bt.powered && ui.button(scan, label, if bt.discovering { Look::On } else { Look::Framed }) {
            let _ = cx.handles.bluetooth.send(bluetooth::Command::Scan(!bt.discovering));
        }
        for device in bt.devices.iter().take(12) {
            let (row, rest) = y.take_top(ROW, 0);
            y = rest;
            let pairing = bt.pairing.as_deref() == Some(device.path.as_str());
            let detail = match (pairing, &bt.passkey, device.battery, device.connected, device.paired) {
                // The code a keyboard wants typed on it, while it is wanted.
                (true, Some(code), ..) => format!("{} {code}", tr("type")),
                (true, None, ..) => tr("Pairing…").to_lowercase(),
                (_, _, Some(level), _, _) => format!("{level}%"),
                (_, _, None, true, _) => tr("Connected").to_lowercase(),
                (_, _, None, false, false) => tr("pair").to_owned(),
                _ => String::new(),
            };
            if ui.row(row, Some(icons::device(&device.icon)), &device.name, &detail, device.connected) && !pairing {
                let path = device.path.clone();
                let command = if device.connected {
                    bluetooth::Command::Disconnect(path)
                } else if !device.paired {
                    bluetooth::Command::Pair(path)
                } else {
                    bluetooth::Command::Connect(path)
                };
                let _ = cx.handles.bluetooth.send(command);
            }
        }
        y.y
    }

    fn mixer(&mut self, ui: &mut Ui, area: Rect, cx: &mut Ctx) -> i32 {
        let t = ui.theme;
        let audio = &cx.services.audio;
        let mut y = self.header(ui, area, &tr("Output").to_lowercase());
        for sink in &audio.sinks {
            let (row, rest) = y.take_top(ROW, 0);
            y = rest;
            let current = sink.name == audio.default_sink;
            if ui.row(row, Some(icons::SPEAKER), &sink.description, "", current) && !current {
                let _ = cx.handles.audio.send(audio::Command::Default(true, sink.name.clone()));
            }
        }
        if !audio.sources.is_empty() {
            let (label, rest) = y.take_top(ui.line(), 4);
            y = Rect::new(rest.x, rest.y + 6, rest.w, rest.h);
            ui.text(Rect::new(label.x, label.y + 6, label.w, label.h), &tr("Input").to_lowercase(), t.dim, Align::Left);
        }
        for source in &audio.sources {
            let (row, rest) = y.take_top(ROW, 0);
            y = rest;
            let current = source.name == audio.default_source;
            if ui.row(row, Some(icons::MIC), &source.description, "", current) && !current {
                let _ = cx.handles.audio.send(audio::Command::Default(false, source.name.clone()));
            }
        }
        if !audio.streams.is_empty() {
            let (label, rest) = y.take_top(ui.line() + 6, 4);
            y = rest;
            ui.text(Rect::new(label.x, label.y + 6, label.w, ui.line()), "apps", t.dim, Align::Left);
        }
        for stream in &audio.streams {
            let (name, rest) = y.take_top(ui.line(), 2);
            ui.text(name, &stream.app, t.fg, Align::Left);
            let (row, rest) = rest.take_top(ROW, 4);
            y = rest;
            let mut page = self.page;
            let icon = if stream.muted { icons::SPEAKER_MUTED } else { icons::SPEAKER };
            match slider_row(ui, row, icon, stream.volume, stream.muted, &mut page, None) {
                Some(Change::Mute) => drop(cx.handles.audio.send(audio::Command::StreamMute(stream.index))),
                Some(Change::Value(v)) => drop(cx.handles.audio.send(audio::Command::StreamVolume(stream.index, v))),
                None => {}
            }
        }
        y.y
    }

    fn session(&mut self, ui: &mut Ui, area: Rect, cx: &mut Ctx) -> i32 {
        let mut y = self.header(ui, area, &tr("Session").to_lowercase());
        let actions = [
            (icons::LOCK, tr("Lock"), Request::Lock),
            (icons::LOGOUT, tr("Log out"), Request::LogOut),
            (icons::MOON, tr("Suspend"), Request::Suspend),
            (icons::RESTART, tr("Restart"), Request::Reboot),
            (icons::POWER, tr("Power off"), Request::PowerOff),
        ];
        for (icon, label, request) in actions {
            let (row, rest) = y.take_top(ROW, 0);
            y = rest;
            if ui.row(row, Some(icon), &label.to_lowercase(), "", false) {
                cx.requests.push(request);
            }
        }
        y.y
    }
}

enum Change {
    Mute,
    Value(f32),
}

/// Icon (click to mute), segmented slider, percentage, and a chevron when there is a page.
fn slider_row(
    ui: &mut Ui,
    row: Rect,
    icon: icons::Icon,
    value: f32,
    muted: bool,
    page: &mut Page,
    more: Option<Page>,
) -> Option<Change> {
    let t = ui.theme;
    let mut change = None;
    let (button, rest) = row.take_left(ROW, 6);
    if ui.icon_button(button, icon, Look::Flat) {
        change = Some(Change::Mute);
    }
    let (rest, chevron) = match more {
        Some(_) => {
            let (chevron, rest) = rest.take_right(ROW, 2);
            (rest, Some(chevron))
        }
        None => (rest, None),
    };
    let (label, track) = rest.take_right(ui.text_width("100%"), 6);
    let percent = format!("{}%", (value * 100.0).round() as i32);
    ui.text(label, &percent, if muted { t.border } else { t.dim }, Align::Right);
    if let Some(v) = ui.slider(track, value.min(1.0)) {
        change = Some(Change::Value(v));
    }
    if let (Some(chevron), Some(target)) = (chevron, more)
        && ui.icon_button(chevron, icons::CHEVRON, Look::Flat)
    {
        *page = target;
    }
    change
}

/// Signal strength as four ascending bars, from block characters.
fn bars(signal: u8) -> String {
    let lit = (signal as usize * 4).div_ceil(100).clamp(1, 4);
    let glyphs = ['▂', '▄', '▆', '█'];
    (0..4).map(|i| if i < lit { glyphs[i] } else { '·' }).collect()
}
