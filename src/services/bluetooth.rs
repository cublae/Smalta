//! BlueZ over the system bus.
//!
//! Pairing goes through an agent of the shell's own, registered on its own
//! connection rather than as the default: BlueZ asks it only about pairings
//! this connection started, which the user did by clicking a device, so it
//! agrees to them. A passkey to type on the device is passed on to be shown.
//!
//! BlueZ publishes the adapter and every device it knows as objects;
//! `GetManagedObjects` returns all of them at once, cheaply enough to ask
//! every couple of seconds instead of following each property signal.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::Duration;

use smithay_client_toolkit::reexports::calloop::channel::Sender;
use zbus::blocking::Connection;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

use super::Update;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Device {
    pub path: String,
    pub name: String,
    pub icon: String,
    pub paired: bool,
    pub connected: bool,
    pub battery: Option<u8>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct State {
    pub available: bool,
    pub powered: bool,
    pub discovering: bool,
    pub devices: Vec<Device>,
    /// The device being paired, and a passkey it wants typed on it.
    pub pairing: Option<String>,
    pub passkey: Option<String>,
}

/// A device's charge, in percent, at which it is shown in red and warned about.
pub const LOW: u8 = 15;

impl State {
    pub fn connected(&self) -> Vec<&Device> {
        self.devices.iter().filter(|d| d.connected).collect()
    }

    /// Connected devices that are running low, by path.
    pub fn low(&self) -> Vec<(String, String, u8)> {
        self.connected()
            .into_iter()
            .filter_map(|d| d.battery.filter(|b| *b <= LOW).map(|b| (d.path.clone(), d.name.clone(), b)))
            .collect()
    }
}

#[derive(Debug)]
pub enum Command {
    Power(bool),
    Scan(bool),
    Connect(String),
    Disconnect(String),
    /// Pair a device found by a scan, trust it and connect.
    Pair(String),
}

/// What a pairing in progress shares with the agent and the reading loop.
#[derive(Default)]
struct Pairing {
    device: Option<String>,
    passkey: Option<String>,
}

const AGENT: &str = "/org/smalta/agent";

struct Agent {
    pairing: Arc<Mutex<Pairing>>,
}

#[zbus::interface(name = "org.bluez.Agent1")]
impl Agent {
    fn release(&self) {}
    fn cancel(&self) {}
    fn request_confirmation(&self, _device: OwnedObjectPath, _passkey: u32) {}
    fn request_authorization(&self, _device: OwnedObjectPath) {}
    fn authorize_service(&self, _device: OwnedObjectPath, _uuid: String) {}
    // Devices that want a code entered on the computer's side: the
    // well-known ones most such devices accept.
    fn request_pin_code(&self, _device: OwnedObjectPath) -> String {
        "0000".into()
    }
    fn request_passkey(&self, _device: OwnedObjectPath) -> u32 {
        0
    }
    // Devices with keys, a keyboard: the code is typed on them.
    fn display_pin_code(&self, _device: OwnedObjectPath, pin: String) {
        self.pairing.lock().unwrap().passkey = Some(pin);
    }
    fn display_passkey(&self, _device: OwnedObjectPath, passkey: u32, _entered: u16) {
        self.pairing.lock().unwrap().passkey = Some(format!("{passkey:06}"));
    }
}

type Objects = HashMap<OwnedObjectPath, HashMap<String, HashMap<String, OwnedValue>>>;

pub fn get<'a, T: TryFrom<&'a Value<'a>>>(map: &'a HashMap<String, OwnedValue>, key: &str) -> Option<T> {
    map.get(key).and_then(|value| T::try_from(value).ok())
}

fn read(connection: &Connection, pairing: &Mutex<Pairing>) -> zbus::Result<(State, Option<String>)> {
    let manager = super::proxy(connection, "org.bluez", "/", "org.freedesktop.DBus.ObjectManager")?;
    let objects: Objects = manager.call("GetManagedObjects", &())?;

    let mut state = State { available: true, ..State::default() };
    let mut adapter = None;
    for (path, interfaces) in &objects {
        if let Some(properties) = interfaces.get("org.bluez.Adapter1")
            && adapter.is_none()
        {
            adapter = Some(path.to_string());
            state.powered = get(properties, "Powered").unwrap_or(false);
            state.discovering = get(properties, "Discovering").unwrap_or(false);
        }
        if let Some(properties) = interfaces.get("org.bluez.Device1") {
            let name: Option<String> = get::<&str>(properties, "Alias").map(str::to_owned);
            let device = Device {
                path: path.to_string(),
                name: name.unwrap_or_else(|| path.to_string()),
                icon: get::<&str>(properties, "Icon").unwrap_or("").to_owned(),
                paired: get(properties, "Paired").unwrap_or(false),
                connected: get(properties, "Connected").unwrap_or(false),
                battery: interfaces.get("org.bluez.Battery1").and_then(|b| get(b, "Percentage")),
            };
            // Unnamed strangers from a scan are noise.
            if device.paired || device.connected || get::<&str>(properties, "Name").is_some() {
                state.devices.push(device);
            }
        }
    }
    state.devices.sort_by(|a, b| b.connected.cmp(&a.connected).then(b.paired.cmp(&a.paired)).then(a.name.cmp(&b.name)));
    let pairing = pairing.lock().unwrap();
    state.pairing = pairing.device.clone();
    state.passkey = pairing.passkey.clone();
    Ok((state, adapter))
}

pub fn spawn(updates: Sender<Update>) -> mpsc::Sender<Command> {
    let (commands, received) = mpsc::channel::<Command>();
    thread::spawn(move || {
        let connection = match Connection::system() {
            Ok(connection) => connection,
            Err(error) => {
                log::warn!("system bus: {error}; no Bluetooth");
                return;
            }
        };
        let pairing = Arc::new(Mutex::new(Pairing::default()));
        let agent = Agent { pairing: pairing.clone() };
        let registered = connection.object_server().at(AGENT, agent).is_ok()
            && super::proxy(&connection, "org.bluez", "/org/bluez", "org.bluez.AgentManager1")
                .and_then(|m| {
                    m.call_method("RegisterAgent", &(OwnedObjectPath::try_from(AGENT).unwrap(), "NoInputNoOutput"))
                })
                .is_ok();
        if !registered {
            log::info!("no Bluetooth agent of our own; pairing new devices is left to another");
        }
        let mut last = None;
        let mut adapter = None;
        loop {
            let state = match read(&connection, &pairing) {
                Ok((state, path)) => {
                    adapter = path;
                    state
                }
                Err(_) => State::default(),
            };
            if last.as_ref() != Some(&state) {
                if updates.send(Update::Bluetooth(state.clone())).is_err() {
                    return;
                }
                last = Some(state);
            }
            // Commands wake the loop early, so a click shows at once.
            let Ok(command) = received.recv_timeout(Duration::from_secs(2)) else { continue };
            if let Err(error) = execute(&connection, adapter.as_deref(), command, &pairing) {
                log::warn!("bluetooth: {error}");
            }
        }
    });
    commands
}

fn execute(
    connection: &Connection,
    adapter: Option<&str>,
    command: Command,
    pairing: &Arc<Mutex<Pairing>>,
) -> zbus::Result<()> {
    let adapter_proxy =
        || super::proxy(connection, "org.bluez", adapter.unwrap_or("/org/bluez/hci0").to_owned(), "org.bluez.Adapter1");
    match command {
        Command::Power(on) => adapter_proxy()?.set_property("Powered", on).map_err(zbus::Error::from),
        Command::Scan(true) => adapter_proxy()?.call_method("StartDiscovery", &()).map(drop),
        Command::Scan(false) => adapter_proxy()?.call_method("StopDiscovery", &()).map(drop),
        Command::Connect(path) => {
            let device = super::proxy(connection, "org.bluez", path, "org.bluez.Device1")?;
            // Connecting can take seconds; it runs on its own thread so the
            // list keeps updating meanwhile.
            thread::spawn(move || {
                if let Err(error) = device.call_method("Connect", &()) {
                    log::warn!("bluetooth connect: {error}");
                }
            });
            Ok(())
        }
        Command::Disconnect(path) => {
            super::proxy(connection, "org.bluez", path, "org.bluez.Device1")?.call_method("Disconnect", &()).map(drop)
        }
        Command::Pair(path) => {
            let device = super::proxy(connection, "org.bluez", path.clone(), "org.bluez.Device1")?;
            *pairing.lock().unwrap() = Pairing { device: Some(path), passkey: None };
            let pairing = pairing.clone();
            // Pairing waits on the device, for as long as it takes a person
            // to type a code into it.
            thread::spawn(move || {
                match device.call_method("Pair", &()) {
                    Ok(_) => {
                        let _ = device.set_property("Trusted", true);
                        if let Err(error) = device.call_method("Connect", &()) {
                            log::warn!("bluetooth connect after pairing: {error}");
                        }
                    }
                    Err(error) => log::warn!("bluetooth pair: {error}"),
                }
                *pairing.lock().unwrap() = Pairing::default();
            });
            Ok(())
        }
    }
}
