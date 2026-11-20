//! The compositor, whichever it is, as one snapshot of what the bar shows.
//!
//! Each backend runs on its own thread, owns its sockets, and sends a fresh
//! [`Snapshot`] whenever something the bar draws has changed. Commands go the
//! other way over a plain channel.

mod hyprland;
mod niri;

use std::sync::mpsc;

use smithay_client_toolkit::reexports::calloop::channel::Sender;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Workspace {
    pub id: i64,
    pub label: String,
    pub output: String,
    /// Shown on its output.
    pub active: bool,
    /// Shown on the output that has focus.
    pub focused: bool,
    pub urgent: bool,
    pub windows: usize,
}

/// An open window, for the switcher and the launcher.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Window {
    /// What the compositor knows it by: niri's number, Hyprland's `0x…` address.
    pub id: String,
    pub title: String,
    pub app: String,
    /// The workspace's label, as the bar shows it.
    pub workspace: String,
    pub output: String,
    pub focused: bool,
    /// Where it is on its output, in logical pixels, when it is on screen.
    pub rect: Option<(i32, i32, i32, i32)>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Snapshot {
    /// Sorted by output, then position.
    pub workspaces: Vec<Workspace>,
    /// Display name of the active keyboard layout, e.g. "English (US)".
    pub layout: String,
    pub layouts: usize,
    /// The focused window's title and application id.
    pub title: String,
    pub app: String,
    /// Something is capturing the screen.
    pub casting: bool,
    /// The application ids of the open windows, sorted, each once.
    pub apps: Vec<String>,
    /// Open windows kept out of screen capture.
    pub hidden: usize,
    /// Open windows, the one used most recently first.
    pub windows: Vec<Window>,
}

#[derive(Debug)]
pub enum Command {
    FocusWorkspace(i64),
    SwitchLayout,
    /// The compositor's own region screenshot, or grim and slurp where there is none.
    Screenshot,
    /// End the session.
    Quit,
    /// Start a program as the compositor's child.
    Spawn(Vec<String>),
    /// Keep the windows of these applications out of screen capture: in a
    /// shared screen or a screenshot they are a black box. Replaces the list.
    HideApps(Vec<String>),
    /// The focused window out of screen capture, or back in.
    ToggleHidden,
    /// Bring a window forward, by its [`Window::id`], switching workspace if need be.
    FocusWindow(String),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    Niri,
    Hyprland,
}

/// A compositor nested in another inherits the outer one's variable as well
/// as setting its own; only then does `XDG_CURRENT_DESKTOP` break the tie.
pub fn detect() -> Option<Kind> {
    let set = |name| std::env::var_os(name).is_some_and(|value| !value.is_empty());
    let desktop = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default().to_lowercase();
    match (set("HYPRLAND_INSTANCE_SIGNATURE"), set("NIRI_SOCKET")) {
        (true, false) => Some(Kind::Hyprland),
        (true, true) if desktop.contains("hyprland") => Some(Kind::Hyprland),
        (_, true) => Some(Kind::Niri),
        _ => None,
    }
}

/// Start the backend; `None` when there is no compositor to talk to.
pub fn spawn(snapshots: Sender<Snapshot>) -> Option<mpsc::Sender<Command>> {
    let kind = detect();
    log::info!("compositor: {kind:?}");
    match kind? {
        Kind::Hyprland => Some(hyprland::spawn(snapshots)),
        Kind::Niri => Some(niri::spawn(snapshots)),
    }
}

/// Reconnect delays, in milliseconds; the last one repeats.
const BACKOFF: [u64; 5] = [250, 500, 1000, 2000, 5000];

fn backoff(attempt: usize) -> std::time::Duration {
    std::time::Duration::from_millis(BACKOFF[attempt.min(BACKOFF.len() - 1)])
}
