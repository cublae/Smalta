//! `~/.config/smalta/config.json`, in the same shape as Tessera's.
//!
//! Keys Smalta does not understand yet are ignored rather than rejected, so a
//! Tessera config can be pointed at directly while the rest is being written.

use std::path::PathBuf;

use serde::Deserialize;

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Config {
    pub bar: Bar,
    pub workspaces: Workspaces,
    pub clock: Clock,
    pub theme: Theme,
    pub interface: Interface,
    pub weather: Weather,
    pub desktop: Desktop,
    pub notifications: NotificationSettings,
    pub osd: Osd,
    pub tray: Tray,
    pub control_center: ControlCenter,
    pub screenshare: Screenshare,
    pub wallpaper: Wallpaper,
    pub media: Media,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Bar {
    /// `top` or `bottom`. The side positions come later.
    pub position: String,
    /// Thickness in logical pixels.
    pub size: u32,
    pub modules: Modules,
    /// Off the screen's edge with room around it, framed and shadowed,
    /// rather than across the whole edge.
    pub floating: bool,
    /// A floating bar's width, in percent of the screen's.
    pub width: u32,
    /// A floating bar's distance from the screen's edge.
    pub margin: u32,
    /// Room before the first module and after the last.
    pub edge_start: u32,
    pub edge_end: u32,
    /// Room between modules.
    pub gap: u32,
}

impl Default for Bar {
    fn default() -> Self {
        Self {
            position: "bottom".into(),
            size: 30,
            modules: Modules::default(),
            floating: false,
            width: 100,
            margin: 6,
            edge_start: 6,
            edge_end: 6,
            gap: 4,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct Modules {
    pub start: Vec<String>,
    pub center: Vec<String>,
    pub end: Vec<String>,
}

impl Default for Modules {
    fn default() -> Self {
        let list = |names: &[&str]| names.iter().map(|name| name.to_string()).collect();
        Self {
            start: list(&["launcher", "workspaces"]),
            center: list(&["clock"]),
            end: list(&["resources", "keyboard-layout"]),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Workspaces {
    pub per_monitor: bool,
    pub show_empty: bool,
}

impl Default for Workspaces {
    fn default() -> Self {
        Self { per_monitor: true, show_empty: false }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct Clock {
    /// strftime, as chrono understands it.
    pub format: String,
}

impl Default for Clock {
    fn default() -> Self {
        Self { format: "%H:%M".into() }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct Theme {
    /// `#rrggbb`. Unset, the accent is the foreground: the whole shell is grey.
    pub accent: Option<String>,
    /// Path to a BDF font. Unset, the Spleen the package was built with.
    pub font: Option<String>,
    /// How many screen pixels make one of the shell's pixels: 2 doubles
    /// everything, font included, and keeps it crisp; 1.5 is allowed too,
    /// its pixels alternating one and two screen pixels wide.
    pub pixel: f32,
    /// App and tray icons recoloured along the theme: dark to the
    /// background, middle tones to the accent, light to the text.
    #[serde(rename = "tintIcons")]
    pub tint_icons: bool,
    /// Tessera's wallpaper, used when `wallpaper.path` is not set.
    pub wallpaper: Option<String>,
}

pub fn dir() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .unwrap_or_else(|| PathBuf::from("/etc/xdg"))
}

/// Smalta's own config, or else Tessera's, or else the defaults.
pub fn load() -> Config {
    let dir = dir();
    for path in [dir.join("smalta/config.json"), dir.join("tessera/config.json")] {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        match serde_json::from_str(&text) {
            Ok(config) => {
                log::info!("config from {}", path.display());
                return config;
            }
            Err(error) => log::error!("{}: {error}; using the defaults", path.display()),
        }
    }
    Config::default()
}

impl Default for Theme {
    fn default() -> Self {
        Self { accent: None, font: None, pixel: 1.0, tint_icons: false, wallpaper: None }
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Interface {
    /// `auto`, `en` or `ru`.
    pub language: String,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(default)]
pub struct Weather {
    /// A place name to look up, or a label for the coordinates below.
    pub location: String,
    pub latitude: f64,
    pub longitude: f64,
    /// `metric` or `imperial`.
    pub units: String,
    /// Minutes between fetches.
    pub interval: u32,
}

impl Default for Weather {
    fn default() -> Self {
        Self { location: String::new(), latitude: 0.0, longitude: 0.0, units: "metric".into(), interval: 30 }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Desktop {
    pub enabled: bool,
    pub clock_format: String,
    pub show_date: bool,
    pub show_calendar: bool,
    pub show_graphs: bool,
    pub show_media: bool,
}

impl Default for Desktop {
    fn default() -> Self {
        Self {
            enabled: false,
            clock_format: "%H:%M".into(),
            show_date: true,
            show_calendar: true,
            show_graphs: true,
            show_media: true,
        }
    }
}

/// Toasts, and the history behind them.
#[derive(Clone, Debug, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct NotificationSettings {
    /// Serve `org.freedesktop.Notifications`. Off, mako or dunst can.
    pub server: bool,
    /// Seconds a toast stays.
    pub timeout: u32,
    /// A toast under the pointer does not go.
    pub pause_on_hover: bool,
    /// Critical toasts stay until closed.
    pub critical_sticky: bool,
    /// Keep to the time an app asks for, up to a minute.
    pub app_timeout: bool,
    /// Toasts on screen at once.
    pub max_toasts: u32,
    /// Notifications kept in the center.
    pub history: u32,
    /// `top-right`, `top-left`, `bottom-right` or `bottom-left`; `auto`
    /// is the right-hand corner by the bar.
    pub position: String,
    /// While the screen is being shared, straight to the center.
    pub quiet_when_sharing: bool,
}

impl Default for NotificationSettings {
    fn default() -> Self {
        Self {
            server: true,
            timeout: 6,
            pause_on_hover: true,
            critical_sticky: true,
            app_timeout: true,
            max_toasts: 4,
            history: 100,
            position: "auto".into(),
            quiet_when_sharing: true,
        }
    }
}

/// The little window that shows a level or a layout as it changes.
#[derive(Clone, Debug, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Osd {
    pub volume: bool,
    pub microphone: bool,
    pub brightness: bool,
    pub layout: bool,
    /// Caps Lock and Num Lock, turned on or off.
    pub locks: bool,
    /// Milliseconds it stays.
    pub timeout: u32,
    /// `top`, `center` or `bottom`; `auto` is the bar's edge.
    pub position: String,
}

impl Default for Osd {
    fn default() -> Self {
        Self {
            volume: true,
            microphone: true,
            brightness: true,
            layout: true,
            locks: true,
            timeout: 1500,
            position: "auto".into(),
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct Tray {
    /// Host the StatusNotifierItem tray. Off, another program can.
    pub enabled: bool,
}

impl Default for Tray {
    fn default() -> Self {
        Self { enabled: true }
    }
}

/// What the control center shows, by name.
#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct ControlCenter {
    pub tiles: Vec<String>,
}

/// Every control-center tile, in the order they are drawn.
pub const TILES: [&str; 9] =
    ["wifi", "bluetooth", "dnd", "profiles", "volume", "microphone", "brightness", "actions", "media"];

impl Default for ControlCenter {
    fn default() -> Self {
        Self { tiles: TILES.iter().map(|t| t.to_string()).collect() }
    }
}

impl ControlCenter {
    pub fn has(&self, tile: &str) -> bool {
        self.tiles.iter().any(|t| t == tile)
    }
}

/// What a shared screen does not show.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Screenshare {
    /// Application classes whose windows are a black box in any capture of
    /// the screen -- a shared screen, a screenshot -- and as usual on it.
    pub hide: Vec<String>,
}

/// A wallpaper drawn by the shell, in big pixels.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(default)]
pub struct Wallpaper {
    /// Off, the shell draws none and whatever else does stays.
    pub enabled: bool,
    /// An image file. Empty, Tessera's `theme.wallpaper`.
    pub path: String,
    /// In big pixels and a few shades, as the rest of the shell; off, the
    /// picture as it is, at the screen's own resolution.
    pub pixelate: bool,
    /// Screen pixels to one of the wallpaper's: 1 is the picture as it is.
    pub cell: u32,
    /// Shades per colour channel, or of grey when tinted: few is posterised.
    pub levels: u32,
    /// Ordered dithering between the shades rather than flat bands.
    pub dither: bool,
    /// `none`, `mono` -- the theme's greys -- or `duotone`, through the accent.
    pub tint: String,
    /// How much darker, 0 to 0.9: text on top stays readable.
    pub dim: f32,
    /// Take the accent from the picture when the theme names none.
    pub accent: bool,
}

impl Default for Wallpaper {
    fn default() -> Self {
        Self {
            enabled: false,
            path: String::new(),
            pixelate: true,
            cell: 3,
            levels: 6,
            dither: true,
            tint: "none".into(),
            dim: 0.0,
            accent: false,
        }
    }
}

impl Wallpaper {
    /// Screen pixels to one of the wallpaper's: one when it is not pixelated.
    pub fn cell(&self) -> u32 {
        if self.pixelate { self.cell.clamp(1, 32) } else { 1 }
    }

    /// The image to draw: this section's, or Tessera's.
    pub fn file(&self, theme: &Theme) -> Option<String> {
        Some(self.path.clone()).filter(|p| !p.is_empty()).or_else(|| theme.wallpaper.clone())
    }
}

/// What plays, on the bar.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(default)]
pub struct Media {
    /// Bars that move with the sound beside the song, through cava.
    pub visualizer: bool,
    /// How many.
    pub bars: u32,
}

impl Default for Media {
    fn default() -> Self {
        Self { visualizer: true, bars: 10 }
    }
}
