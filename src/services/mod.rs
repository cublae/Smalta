//! Everything the control center shows, each piece on its own thread.
//!
//! A service owns its connection to the system -- a D-Bus proxy, a command
//! line tool and its event stream -- and sends a whole new state whenever
//! something changed. The UI only ever reads the latest state and sends
//! commands back; it never waits on a service.

pub mod audio;
pub mod bluetooth;
pub mod camera;
pub mod locks;
pub mod media;
pub mod network;
pub mod notifications;
pub mod power;
pub mod rates;
pub mod tray;
pub mod weather;

use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use smithay_client_toolkit::reexports::calloop::channel::Sender;

pub enum Update {
    Audio(audio::State),
    Network(network::State),
    Bluetooth(bluetooth::State),
    Power(power::State),
    Media(media::State),
    Tray(Vec<tray::Item>),
    /// A menu that was asked for: the item it belongs to, and its entries.
    TrayMenu(String, Vec<tray::MenuEntry>),
    Weather(weather::State),
    Camera(camera::State),
    Rates(rates::Rates),
    Locks(locks::State),
}

#[derive(Default)]
pub struct Services {
    pub audio: audio::State,
    pub network: network::State,
    pub bluetooth: bluetooth::State,
    pub power: power::State,
    pub media: media::State,
    pub tray: Vec<tray::Item>,
    pub tray_menu: Option<(String, Vec<tray::MenuEntry>)>,
    pub weather: weather::State,
    /// Programs with a camera open.
    pub camera: camera::State,
    /// Exchange rates per US dollar, once the converter has asked for them.
    pub rates: Option<rates::Rates>,
    pub locks: locks::State,
}

impl Services {
    pub fn apply(&mut self, update: Update) {
        match update {
            Update::Audio(state) => self.audio = state,
            Update::Network(state) => self.network = state,
            Update::Bluetooth(state) => self.bluetooth = state,
            Update::Power(state) => self.power = state,
            Update::Media(state) => self.media = state,
            Update::Tray(items) => self.tray = items,
            Update::TrayMenu(key, entries) => self.tray_menu = Some((key, entries)),
            Update::Weather(state) => self.weather = state,
            Update::Camera(state) => self.camera = state,
            Update::Rates(rates) => self.rates = Some(rates),
            Update::Locks(state) => self.locks = state,
        }
    }
}

/// Where commands for each service go.
pub struct Handles {
    pub audio: mpsc::Sender<audio::Command>,
    pub network: mpsc::Sender<network::Command>,
    pub bluetooth: mpsc::Sender<bluetooth::Command>,
    pub power: mpsc::Sender<power::Command>,
    pub media: mpsc::Sender<media::Command>,
    pub tray: mpsc::Sender<tray::Command>,
    pub weather: mpsc::Sender<weather::Command>,
    pub rates: mpsc::Sender<()>,
}

pub fn spawn(updates: Sender<Update>, config: &crate::config::Config) -> Handles {
    Handles {
        audio: audio::spawn(updates.clone()),
        network: network::spawn(updates.clone()),
        bluetooth: bluetooth::spawn(updates.clone()),
        power: power::spawn(updates.clone()),
        media: media::spawn(updates.clone()),
        rates: rates::spawn(updates.clone()),
        // Off, commands go nowhere and the tray module stays empty.
        tray: if config.tray.enabled { tray::spawn(updates.clone()) } else { mpsc::channel().0 },
        weather: {
            camera::spawn(updates.clone());
            locks::spawn(updates.clone());
            weather::spawn(updates, config.weather.clone())
        },
    }
}

/// Run a program to completion and return its standard output, or `None`
/// when it is missing or fails.
pub fn run(program: &str, args: &[&str]) -> Option<String> {
    let output = Command::new(program).args(args).stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
    output.status.success().then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Like [`run`], but keeps standard error, for messages worth showing.
pub fn run_reporting(program: &str, args: &[&str]) -> Result<String, String> {
    match Command::new(program).args(args).stdin(Stdio::null()).output() {
        Ok(output) if output.status.success() => Ok(String::from_utf8_lossy(&output.stdout).into_owned()),
        Ok(output) => {
            let message = String::from_utf8_lossy(&output.stderr).trim().to_owned();
            Err(if message.is_empty() { format!("{program} failed") } else { message })
        }
        Err(error) => Err(format!("{program}: {error}")),
    }
}

/// Start a program and forget about it.
pub fn launch(argv: &[&str]) {
    let Some((program, args)) = argv.split_first() else { return };
    let child =
        Command::new(program).args(args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
    match child {
        // Reap it in the background so it does not linger as a zombie.
        Ok(mut child) => {
            std::thread::spawn(move || child.wait());
        }
        Err(error) => log::error!("{program}: {error}"),
    }
}

/// Collapses a burst of "something changed" signals into one re-read: waits
/// for the first, lets the rest arrive, then drains them.
pub fn settle(stale: &mpsc::Receiver<()>, wait: Duration) -> bool {
    if stale.recv().is_err() {
        return false;
    }
    std::thread::sleep(wait);
    while stale.try_recv().is_ok() {}
    true
}

/// A D-Bus proxy that does not prefetch properties. zbus would otherwise ask
/// for all of them up front, and log a warning every time the service is not
/// there -- which, for UPower on a desktop, is every poll.
pub fn proxy<'a>(
    connection: &zbus::blocking::Connection,
    destination: impl Into<String>,
    path: impl Into<String>,
    interface: &'static str,
) -> zbus::Result<zbus::blocking::Proxy<'a>> {
    zbus::blocking::proxy::Builder::new(connection)
        .destination(destination.into())?
        .path(path.into())?
        .interface(interface)?
        .cache_properties(zbus::proxy::CacheProperties::No)
        .build()
}
