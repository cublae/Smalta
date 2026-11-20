//! Battery, power profile and screen brightness.
//!
//! Battery and profile come from UPower and power-profiles-daemon on the
//! system bus. Brightness is read from sysfs, often, because brightness keys
//! change it behind the shell's back and the OSD should follow at once; it is
//! written through logind, which lets a session set its own backlight
//! without being root.

use std::path::PathBuf;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use smithay_client_toolkit::reexports::calloop::channel::Sender;
use zbus::blocking::{Connection, Proxy};

use super::Update;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct State {
    pub battery: Option<Battery>,
    /// `power-saver`, `balanced` or `performance`.
    pub profile: Option<String>,
    pub profiles: Vec<String>,
    /// 0..1, when there is a backlight.
    pub brightness: Option<f32>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Battery {
    pub percent: f32,
    pub charging: bool,
    /// Minutes until empty, or until full while charging, once there is
    /// enough of a trend to tell.
    pub minutes: Option<u32>,
}

impl Battery {
    /// Below this, unplugged, it shows in the urgent colour and warns once.
    pub const LOW: f32 = 0.15;

    pub fn low(&self) -> bool {
        self.percent < Self::LOW && !self.charging
    }

    /// How long it has left, or how long until full: "2 h 05 min".
    pub fn left(&self) -> Option<String> {
        let minutes = self.minutes?;
        let (h, m) = (minutes / 60, minutes % 60);
        let min = crate::i18n::tr("min");
        Some(if h > 0 { format!("{h} {} {m:02} {min}", crate::i18n::tr("h")) } else { format!("{m} {min}") })
    }
}

#[derive(Debug)]
pub enum Command {
    Profile(String),
    Brightness(f32),
}

struct Backlight {
    name: String,
    dir: PathBuf,
    max: u32,
}

fn backlight() -> Option<Backlight> {
    let entry = std::fs::read_dir("/sys/class/backlight").ok()?.flatten().next()?;
    let dir = entry.path();
    let max = std::fs::read_to_string(dir.join("max_brightness")).ok()?.trim().parse().ok()?;
    Some(Backlight { name: entry.file_name().to_string_lossy().into_owned(), dir, max })
}

fn brightness(light: &Backlight) -> Option<f32> {
    let now: u32 = std::fs::read_to_string(light.dir.join("brightness")).ok()?.trim().parse().ok()?;
    Some(now as f32 / light.max.max(1) as f32)
}

/// From UPower when it runs, else straight from the kernel.
fn battery(connection: Option<&Connection>) -> Option<Battery> {
    connection.and_then(upower_battery).or_else(sysfs_battery)
}

fn sysfs_battery() -> Option<Battery> {
    let entry = std::fs::read_dir("/sys/class/power_supply")
        .ok()?
        .flatten()
        .find(|e| std::fs::read_to_string(e.path().join("type")).is_ok_and(|t| t.trim() == "Battery"))?;
    let read = |name: &str| std::fs::read_to_string(entry.path().join(name)).ok().map(|s| s.trim().to_owned());
    let percent: f32 = read("capacity")?.parse().ok()?;
    let status = read("status").unwrap_or_default();
    let number = |name: &str| read(name).and_then(|v| v.parse::<f64>().ok());
    // In energy (µWh, µW) or in charge (µAh, µA), whichever the battery reports.
    let (now, full, rate) = match number("energy_now") {
        Some(now) => (now, number("energy_full"), number("power_now")),
        None => (number("charge_now").unwrap_or(0.0), number("charge_full"), number("current_now")),
    };
    let left = match (status.as_str(), full, rate) {
        (_, _, None) => None,
        (_, _, Some(rate)) if rate <= 0.0 => None,
        ("Discharging", _, Some(rate)) => Some(now / rate * 60.0),
        ("Charging", Some(full), Some(rate)) => Some((full - now).max(0.0) / rate * 60.0),
        _ => None,
    };
    Some(Battery {
        percent: percent / 100.0,
        charging: matches!(status.as_str(), "Charging" | "Full"),
        minutes: left.map(|m| m.round() as u32).filter(|m| *m > 0),
    })
}

fn upower_battery(connection: &Connection) -> Option<Battery> {
    let device = super::proxy(
        connection,
        "org.freedesktop.UPower",
        "/org/freedesktop/UPower/devices/DisplayDevice",
        "org.freedesktop.UPower.Device",
    )
    .ok()?;
    let present: bool = device.get_property("IsPresent").ok()?;
    if !present {
        return None;
    }
    let percent: f64 = device.get_property("Percentage").ok()?;
    let state: u32 = device.get_property("State").unwrap_or(0);
    // 1 charging, 4 fully charged, 5 pending charge.
    let charging = matches!(state, 1 | 4 | 5);
    // Zero while UPower has not seen enough of the trend yet.
    let left: i64 = device.get_property(if state == 1 { "TimeToFull" } else { "TimeToEmpty" }).unwrap_or(0);
    let minutes = (matches!(state, 1 | 2) && left >= 60).then_some((left / 60) as u32);
    Some(Battery { percent: percent as f32 / 100.0, charging, minutes })
}

fn profiles(connection: &Connection) -> Option<Proxy<'static>> {
    super::proxy(
        connection,
        "org.freedesktop.UPower.PowerProfiles",
        "/org/freedesktop/UPower/PowerProfiles",
        "org.freedesktop.UPower.PowerProfiles",
    )
    .ok()
}

fn profile_names(proxy: &Proxy<'_>) -> Vec<String> {
    type Profiles = Vec<std::collections::HashMap<String, zbus::zvariant::OwnedValue>>;
    let list: Profiles = proxy.get_property("Profiles").unwrap_or_default();
    list.iter().filter_map(|p| super::bluetooth::get::<&str>(p, "Profile").map(str::to_owned)).collect()
}

pub fn spawn(updates: Sender<Update>) -> mpsc::Sender<Command> {
    let (commands, received) = mpsc::channel::<Command>();
    thread::spawn(move || {
        let system = Connection::system().map_err(|e| log::warn!("system bus: {e}")).ok();
        let light = backlight();
        let mut state = State::default();
        let mut slow = Instant::now() - Duration::from_secs(60);
        loop {
            let mut next = state.clone();
            next.brightness = light.as_ref().and_then(brightness);
            if slow.elapsed() >= Duration::from_secs(5) {
                slow = Instant::now();
                next.battery = battery(system.as_ref());
                if let Some(system) = &system
                    && let Some(proxy) = profiles(system)
                {
                    next.profile = proxy.get_property::<String>("ActiveProfile").ok();
                    next.profiles = profile_names(&proxy);
                }
            }
            if next != state {
                state = next;
                if updates.send(Update::Power(state.clone())).is_err() {
                    return;
                }
            }
            // A command forces the slow properties to be read again at once.
            let Ok(command) = received.recv_timeout(Duration::from_millis(250)) else { continue };
            slow = Instant::now() - Duration::from_secs(60);
            let Some(system) = &system else { continue };
            match command {
                Command::Profile(name) => {
                    if let Some(proxy) = profiles(system)
                        && let Err(error) = proxy.set_property("ActiveProfile", name)
                    {
                        log::warn!("power profile: {error}");
                    }
                }
                Command::Brightness(value) => {
                    let Some(light) = &light else { continue };
                    let raw = (value.clamp(0.01, 1.0) * light.max as f32).round() as u32;
                    let session = super::proxy(
                        system,
                        "org.freedesktop.login1",
                        "/org/freedesktop/login1/session/auto",
                        "org.freedesktop.login1.Session",
                    );
                    let result =
                        session.and_then(|s| s.call_method("SetBrightness", &("backlight", light.name.as_str(), raw)));
                    if let Err(error) = result {
                        log::warn!("brightness: {error}");
                    }
                }
            }
        }
    });
    commands
}
