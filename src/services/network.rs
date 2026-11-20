//! NetworkManager, through `nmcli`: terse output to read, `nmcli monitor` to
//! learn when to read it again.

use std::io::{BufRead, BufReader};
use std::process::{Command as Process, Stdio};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::Duration;

use smithay_client_toolkit::reexports::calloop::channel::Sender;

use super::{Update, run, run_reporting, settle};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Wifi {
    pub ssid: String,
    pub signal: u8,
    pub secure: bool,
    pub active: bool,
    /// A connection profile exists, so no password is needed.
    pub saved: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct State {
    pub available: bool,
    pub wifi_enabled: bool,
    pub has_wifi: bool,
    pub wired: bool,
    /// SSID of the network in use.
    pub connected: Option<String>,
    pub networks: Vec<Wifi>,
    /// A connection being made right now.
    pub busy: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug)]
pub enum Command {
    SetWifi(bool),
    Connect { ssid: String, password: Option<String>, hidden: bool },
    Disconnect,
    Forget(String),
    Rescan,
}

/// Splits a line of `nmcli -t`, where `:` separates fields and `\:` is a colon.
fn fields(line: &str) -> Vec<String> {
    let mut out = vec![String::new()];
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                if let Some(next) = chars.next() {
                    out.last_mut().unwrap().push(next);
                }
            }
            ':' => out.push(String::new()),
            _ => out.last_mut().unwrap().push(c),
        }
    }
    out
}

fn read(busy: Option<String>, error: Option<String>) -> State {
    let Some(radio) = run("nmcli", &["-t", "radio", "wifi"]) else {
        return State::default();
    };
    let devices = run("nmcli", &["-t", "-f", "TYPE,STATE,CONNECTION", "device"]).unwrap_or_default();
    let mut has_wifi = false;
    let mut wired = false;
    for line in devices.lines() {
        let f = fields(line);
        match f.first().map(String::as_str) {
            Some("wifi") => has_wifi = true,
            Some("ethernet") if f.get(1).is_some_and(|s| s == "connected") => wired = true,
            _ => {}
        }
    }
    let saved: Vec<String> = run("nmcli", &["-t", "-f", "NAME,TYPE", "connection", "show"])
        .unwrap_or_default()
        .lines()
        .map(fields)
        .filter(|f| f.get(1).is_some_and(|t| t.contains("wireless")))
        .filter_map(|f| f.into_iter().next())
        .collect();

    let mut networks: Vec<Wifi> = Vec::new();
    let list = run("nmcli", &["-t", "-f", "IN-USE,SSID,SIGNAL,SECURITY", "device", "wifi", "list", "--rescan", "no"])
        .unwrap_or_default();
    for line in list.lines() {
        let f = fields(line);
        let [in_use, ssid, signal, security] = &f[..] else { continue };
        if ssid.is_empty() {
            continue;
        }
        let wifi = Wifi {
            ssid: ssid.clone(),
            signal: signal.parse().unwrap_or(0),
            secure: !security.is_empty() && security != "--",
            active: in_use == "*",
            saved: saved.contains(ssid),
        };
        // The same network on two bands is one line to a person.
        match networks.iter_mut().find(|n| n.ssid == wifi.ssid) {
            Some(known) => {
                known.active |= wifi.active;
                known.signal = known.signal.max(wifi.signal);
            }
            None => networks.push(wifi),
        }
    }
    networks.sort_by(|a, b| b.active.cmp(&a.active).then(b.saved.cmp(&a.saved)).then(b.signal.cmp(&a.signal)));

    State {
        available: true,
        wifi_enabled: radio.trim() == "enabled",
        has_wifi,
        wired,
        connected: networks.iter().find(|n| n.active).map(|n| n.ssid.clone()),
        networks,
        busy,
        error,
    }
}

pub fn spawn(updates: Sender<Update>) -> mpsc::Sender<Command> {
    let (stale, refresh) = mpsc::channel::<()>();
    // What the command thread is doing, shown alongside the state.
    let activity: Arc<Mutex<(Option<String>, Option<String>)>> = Arc::default();

    let marker = stale.clone();
    thread::spawn(move || {
        loop {
            let _ = marker.send(());
            match Process::new("nmcli").arg("monitor").stdout(Stdio::piped()).stderr(Stdio::null()).spawn() {
                Ok(mut child) => {
                    if let Some(stdout) = child.stdout.take() {
                        for _ in BufReader::new(stdout).lines().map_while(Result::ok) {
                            if marker.send(()).is_err() {
                                let _ = child.kill();
                                return;
                            }
                        }
                    }
                    let _ = child.wait();
                }
                Err(error) => log::warn!("nmcli: {error}; no network controls"),
            }
            thread::sleep(Duration::from_secs(5));
        }
    });

    let reading = activity.clone();
    thread::spawn(move || {
        let mut last = None;
        while settle(&refresh, Duration::from_millis(150)) {
            let (busy, error) = reading.lock().unwrap().clone();
            let state = read(busy, error);
            if last.as_ref() != Some(&state) {
                if updates.send(Update::Network(state.clone())).is_err() {
                    return;
                }
                last = Some(state);
            }
        }
    });

    let (commands, received) = mpsc::channel();
    thread::spawn(move || {
        for command in received {
            let report = |busy: Option<String>, error: Option<String>| {
                *activity.lock().unwrap() = (busy, error);
                let _ = stale.send(());
            };
            match command {
                Command::SetWifi(on) => {
                    let _ = run("nmcli", &["radio", "wifi", if on { "on" } else { "off" }]);
                }
                Command::Connect { ssid, password, hidden } => {
                    report(Some(ssid.clone()), None);
                    let mut args = vec!["--wait", "30", "device", "wifi", "connect", ssid.as_str()];
                    if let Some(password) = password.as_deref() {
                        args.extend(["password", password]);
                    }
                    if hidden {
                        args.extend(["hidden", "yes"]);
                    }
                    match run_reporting("nmcli", &args) {
                        Ok(_) => report(None, None),
                        Err(message) => report(None, Some(message.replace("Error: ", ""))),
                    }
                }
                Command::Disconnect => {
                    let device = run("nmcli", &["-t", "-f", "DEVICE,TYPE", "device"])
                        .unwrap_or_default()
                        .lines()
                        .map(fields)
                        .find(|f| f.get(1).is_some_and(|t| t == "wifi"))
                        .and_then(|f| f.into_iter().next());
                    if let Some(device) = device {
                        let _ = run("nmcli", &["device", "disconnect", &device]);
                    }
                }
                Command::Forget(ssid) => {
                    let _ = run("nmcli", &["connection", "delete", "id", &ssid]);
                    let _ = stale.send(());
                }
                Command::Rescan => {
                    let _ = run("nmcli", &["device", "wifi", "rescan"]);
                    thread::sleep(Duration::from_secs(3));
                    let _ = stale.send(());
                }
            }
        }
    });
    commands
}
