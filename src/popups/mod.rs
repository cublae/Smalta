//! Dropdowns: one open at a time, on one output.

pub mod calendar;
pub mod clipboard;
pub mod control;
pub mod launcher;
pub mod notifications;
pub mod session;
pub mod settings;
pub mod switcher;
pub mod traymenu;
pub mod weather;

use smithay_client_toolkit::shell::wlr_layer::KeyboardInteractivity;

use crate::canvas::Rect;
use crate::i18n::tr;
use crate::services::{Handles, Services};
use crate::ui::{Align, Key, Ui};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    Calendar,
    ControlCenter,
    Launcher,
    Clipboard,
    Notifications,
    Weather,
    TrayMenu,
    Settings,
    /// The open windows, to switch between.
    Windows,
    /// Lock, log out, suspend, restart, power off, over the whole screen.
    Session,
}

/// Where a popup sits: against the bar at one end of it, or in the middle of the screen.
#[derive(Clone, Copy, PartialEq)]
pub enum Placement {
    Bar(Align),
    Center,
    /// Against the right edge of the screen, its full height.
    Side,
    /// Wherever it was dragged, at the size it was pulled to.
    Floating,
    /// The whole screen, drawn by the popup itself, without a panel.
    Screen,
}

impl Kind {
    pub const ALL: [Kind; 9] = [
        Kind::Calendar,
        Kind::ControlCenter,
        Kind::Launcher,
        Kind::Clipboard,
        Kind::Notifications,
        Kind::Weather,
        Kind::Settings,
        Kind::Windows,
        Kind::Session,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Kind::Calendar => "calendar",
            Kind::ControlCenter => "control-center",
            Kind::Launcher => "launcher",
            Kind::Clipboard => "clipboard",
            Kind::Notifications => "notification-center",
            Kind::Weather => "weather",
            Kind::TrayMenu => "tray-menu",
            Kind::Settings => "settings",
            Kind::Windows => "windows",
            Kind::Session => "session",
        }
    }

    pub fn parse(name: &str) -> Option<Kind> {
        Kind::ALL.into_iter().find(|kind| kind.name() == name)
    }

    pub fn title(self) -> &'static str {
        tr(match self {
            // The big clock heads it instead.
            Kind::Calendar => "",
            Kind::ControlCenter => "Control center",
            Kind::Launcher => "Launcher",
            Kind::Clipboard => "Clipboard",
            Kind::Notifications => "Notifications",
            Kind::Weather => "Weather",
            Kind::TrayMenu => "",
            // It draws its own title row, with a way to close it.
            Kind::Settings => "",
            Kind::Windows => "Windows",
            // It draws its own, a clock.
            Kind::Session => "",
        })
    }

    /// Content width in pixels, without the frame.
    pub fn width(self) -> i32 {
        match self {
            Kind::Calendar => calendar::WIDTH + COLUMN_GAP * 2 + 1 + NOTIFICATIONS_COLUMN,
            Kind::Launcher => 420,
            Kind::Clipboard => 380,
            Kind::ControlCenter => 320,
            Kind::Notifications => 360,
            Kind::TrayMenu => 220,
            Kind::Settings => settings::WIDTH,
            Kind::Windows => switcher::WIDTH,
            _ => 340,
        }
    }

    pub fn placement(self) -> Placement {
        match self {
            Kind::Calendar => Placement::Bar(Align::Center),
            Kind::Launcher | Kind::Windows => Placement::Center,
            Kind::Settings => Placement::Floating,
            Kind::Session => Placement::Screen,
            Kind::Clipboard => Placement::Side,
            Kind::Weather | Kind::TrayMenu => Placement::Bar(Align::Left),
            Kind::ControlCenter | Kind::Notifications => Placement::Bar(Align::Right),
        }
    }

    pub fn keyboard(self) -> KeyboardInteractivity {
        match self {
            Kind::Launcher | Kind::Clipboard | Kind::Settings | Kind::Windows | Kind::Session => {
                KeyboardInteractivity::Exclusive
            }
            _ => KeyboardInteractivity::OnDemand,
        }
    }
}

/// What a key press did.
pub enum Outcome {
    Ignored,
    Redraw,
    Close,
}

/// Things a popup asks the shell to do, which it cannot do itself.
#[derive(Clone, Debug, PartialEq)]
pub enum Request {
    Close,
    /// Open another popup in place of this one.
    Show(Kind),
    /// Read the config again and use it, restarting only if that takes it.
    ApplyConfig,
    /// The control center, at its mixer.
    Mixer,
    /// The OSD for a moment, to see where it comes up.
    PreviewOsd,
    /// The focused window out of screen capture, or back in.
    ToggleHidden,
    /// The settings, at the page of one of `settings::topics`.
    Settings(usize),
    /// Start a program, through the compositor so it is the compositor's child.
    Spawn(Vec<String>),
    /// The screenshot finished: copied, kept, or thrown away.
    Shot(crate::shot::Finish),
    /// Put text on the clipboard.
    Copy(String),
    /// Bring a window forward, by its compositor id.
    FocusWindow(String),
    Dismiss(u32),
    Invoke(u32, String),
    ClearNotifications,
    Tray(crate::services::tray::Command),
    Screenshot,
    PickColor,
    ToggleRecording,
    ToggleDnd,
    Lock,
    LogOut,
    Suspend,
    Reboot,
    PowerOff,
}

/// What a popup reads, and where it sends what it wants done.
pub struct Ctx<'a> {
    pub services: &'a Services,
    pub handles: &'a Handles,
    pub requests: &'a mut Vec<Request>,
    pub dnd: bool,
    pub recording: bool,
    pub notifications: &'a crate::notifications::Notifications,
    pub config: &'a crate::config::Config,
    /// What the compositor last said: open applications, hidden windows.
    pub snapshot: &'a crate::compositor::Snapshot,
}

#[derive(Default)]
pub struct Popups {
    pub calendar: calendar::Calendar,
    pub control: control::ControlCenter,
    pub launcher: launcher::Launcher,
    pub clipboard: clipboard::History,
    pub traymenu: traymenu::TrayMenu,
    pub settings: settings::Settings,
    pub switcher: switcher::Switcher,
    pub session: session::Session,
}

/// The calendar's notification column, and the room either side of the rule before it.
const NOTIFICATIONS_COLUMN: i32 = 340;
const COLUMN_GAP: i32 = 12;

/// Room around the content: the frame and its padding, and the shadow.
pub const PADDING: i32 = 12;
pub const SHADOW: i32 = 3;

impl Popups {
    /// Called whenever a popup opens, to start it from a clean slate.
    pub fn opened(&mut self, kind: Kind) {
        match kind {
            Kind::Calendar => self.calendar.offset = 0,
            Kind::ControlCenter => self.control.opened(),
            Kind::Launcher => self.launcher.opened(),
            Kind::Clipboard => self.clipboard.opened(),
            Kind::Settings => self.settings.opened(),
            Kind::Session => self.session.opened(),
            _ => {}
        }
    }

    /// Draws `kind`'s content into `area` and returns the height it needs.
    pub fn draw(&mut self, kind: Kind, ui: &mut Ui, area: Rect, cx: &mut Ctx) -> i32 {
        match kind {
            Kind::Calendar => {
                // The calendar on the left sets the height; the notifications
                // fill the same height on the right, past a rule.
                let (left, right) = area.take_left(calendar::WIDTH, COLUMN_GAP);
                let height = self.calendar.draw(ui, left);
                ui.fill(Rect::new(right.x, area.y, 1, height), ui.theme.border);
                let right = Rect::new(right.x + 1 + COLUMN_GAP, area.y, right.w - 1 - COLUMN_GAP, height);
                notifications::column(ui, right, cx);
                height
            }
            Kind::ControlCenter => self.control.draw(ui, area, cx),
            Kind::Launcher => self.launcher.draw(ui, area, cx),
            Kind::Clipboard => self.clipboard.draw(ui, area, cx),
            Kind::Notifications => notifications::draw(ui, area, cx),
            Kind::TrayMenu => traymenu::draw(&self.traymenu, ui, area, cx),
            Kind::Weather => weather::draw(ui, area, cx),
            Kind::Settings => self.settings.draw(ui, area, cx),
            Kind::Windows => self.switcher.draw(ui, area, cx),
            Kind::Session => self.session.draw(ui, area, cx),
        }
    }

    pub fn key(&mut self, kind: Kind, key: &Key, cx: &mut Ctx) -> Outcome {
        match kind {
            Kind::ControlCenter => self.control.key(key, cx),
            Kind::Launcher => self.launcher.key(key, cx),
            Kind::Clipboard => self.clipboard.key(key, cx),
            Kind::Settings => self.settings.key(key, cx),
            Kind::Windows => self.switcher.key(key, cx),
            Kind::Session => self.session.key(key, cx),
            _ if *key == Key::Escape => Outcome::Close,
            _ => Outcome::Ignored,
        }
    }
}
