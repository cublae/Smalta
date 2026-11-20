//! Hyprland, over its two sockets in `$XDG_RUNTIME_DIR/hypr/$SIGNATURE`.
//!
//! `.socket2.sock` streams `EVENT>>DATA` lines that say something changed but
//! rarely what the result is, so a relevant event only marks the state stale
//! and the query socket is asked again. `.socket.sock` takes one command per
//! connection, answers and hangs up.

use std::collections::HashSet;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::Duration;

use serde_json::Value;
use smithay_client_toolkit::reexports::calloop::channel::Sender;

use super::{Command, Snapshot, Window, Workspace};

/// Events after which the state is read again.
const STRUCTURAL: &[&str] = &[
    "workspacev2",
    "focusedmonv2",
    "createworkspacev2",
    "destroyworkspacev2",
    "moveworkspacev2",
    "renameworkspace",
    "openwindow",
    "closewindow",
    "movewindowv2",
    "monitoraddedv2",
    "monitorremovedv2",
    "activelayout",
    "configreloaded",
    "urgent",
    "activewindowv2",
    "windowtitlev2",
    "screencast",
];

fn socket_dir() -> PathBuf {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_default();
    let signature = std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").unwrap_or_default();
    runtime.join("hypr").join(signature)
}

fn request(command: &str) -> std::io::Result<String> {
    let mut socket = UnixStream::connect(socket_dir().join(".socket.sock"))?;
    socket.set_read_timeout(Some(Duration::from_secs(2)))?;
    socket.write_all(command.as_bytes())?;
    let mut reply = String::new();
    socket.read_to_string(&mut reply)?;
    Ok(reply)
}

fn query(name: &str) -> std::io::Result<Value> {
    let reply = request(&format!("j/{name}"))?;
    serde_json::from_str(&reply).map_err(std::io::Error::other)
}

/// Windows kept out of screen capture: those of the listed applications,
/// hidden as they open, and any hidden by hand. Hyprland draws them as black
/// boxes in whatever captures the screen and as usual on it.
#[derive(Default)]
struct Hider {
    /// Application classes, lower-cased.
    apps: Vec<String>,
    /// Addresses hidden for their application, and by hand.
    auto: HashSet<String>,
    manual: HashSet<String>,
}

impl Hider {
    fn listed(&self, class: &str) -> bool {
        let class = class.to_lowercase();
        self.apps.contains(&class)
    }

    fn hidden(&self, address: &str) -> bool {
        self.auto.contains(address) || self.manual.contains(address)
    }
}

/// A window in or out of screen capture, by its `0x…` address.
fn set_hidden(address: &str, hidden: bool) {
    let value = if hidden { "1" } else { "0" };
    let lua = format!(
        "/dispatch hl.dsp.window.set_prop({{ prop = \"no_screen_share\", value = \"{value}\", window = \"address:{address}\" }})"
    );
    match request(&lua) {
        Ok(reply) if reply.trim() == "ok" => {}
        // Hyprland configured with hyprlang rather than Lua.
        _ => match request(&format!("/dispatch setprop address:{address} noscreenshare {value}")) {
            Ok(reply) if reply.trim() == "ok" => {}
            other => log::error!("Hyprland would not hide {address} from capture: {other:?}"),
        },
    }
}

/// Open windows as `(address, class)`.
fn clients() -> Vec<(String, String)> {
    query("clients")
        .ok()
        .and_then(|c| c.as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|c| Some((c["address"].as_str()?.to_owned(), c["class"].as_str().unwrap_or("").to_owned())))
        .collect()
}

pub fn spawn(snapshots: Sender<Snapshot>) -> mpsc::Sender<Command> {
    let (marker, refresh) = mpsc::channel::<()>();
    // Urgency is announced once and cannot be read back, so this is the only
    // record of it: addresses that asked for attention and have not had focus since.
    let urgent = Arc::new(Mutex::new(Vec::<String>::new()));
    // Captures running: `screencast>>1,…` starts one and `>>0,…` ends one. One
    // that began before the shell did ends without having been counted.
    let casts = Arc::new(Mutex::new(0usize));
    let counter = casts.clone();
    let hider = Arc::new(Mutex::new(Hider::default()));
    let opened = hider.clone();
    let stale = marker.clone();

    // Events -> "stale".
    let record = urgent.clone();
    thread::spawn(move || {
        let mut attempt = 0;
        loop {
            match UnixStream::connect(socket_dir().join(".socket2.sock")) {
                Ok(stream) => {
                    attempt = 0;
                    let _ = marker.send(());
                    for line in BufReader::new(stream).lines() {
                        let Ok(line) = line else { break };
                        let (name, data) = line.split_once(">>").unwrap_or((&line, ""));
                        let address = format!("0x{data}");
                        match name {
                            "urgent" => record.lock().unwrap().push(address),
                            "activewindowv2" => record.lock().unwrap().retain(|a| *a != address),
                            "screencast" => {
                                let mut count = counter.lock().unwrap();
                                *count = if data.starts_with('1') { *count + 1 } else { count.saturating_sub(1) };
                            }
                            // `ADDRESS,WORKSPACE,CLASS,TITLE`: hidden before it shows, if listed.
                            "openwindow" => {
                                let address = format!("0x{}", data.split(',').next().unwrap_or(""));
                                let class = data.split(',').nth(2).unwrap_or("");
                                let mut hider = opened.lock().unwrap();
                                if hider.listed(class) {
                                    set_hidden(&address, true);
                                    hider.auto.insert(address);
                                }
                            }
                            "closewindow" => {
                                let address = format!("0x{data}");
                                let mut hider = opened.lock().unwrap();
                                hider.auto.remove(&address);
                                hider.manual.remove(&address);
                            }
                            _ => {}
                        }
                        if STRUCTURAL.contains(&name) && marker.send(()).is_err() {
                            return;
                        }
                    }
                    log::error!("Hyprland event stream dropped");
                }
                Err(error) => log::error!("Hyprland event stream: {error}"),
            }
            thread::sleep(super::backoff(attempt));
            attempt += 1;
        }
    });

    let counted = hider.clone();
    // "Stale" -> one re-read, however many events arrived meanwhile.
    thread::spawn(move || {
        let mut last = Snapshot::default();
        while refresh.recv().is_ok() {
            thread::sleep(Duration::from_millis(30));
            while refresh.try_recv().is_ok() {}
            let casting = *casts.lock().unwrap() > 0;
            let hidden = {
                let h = counted.lock().unwrap();
                h.auto.union(&h.manual).count()
            };
            match read_state(&mut urgent.lock().unwrap(), casting, hidden) {
                Ok(snapshot) if snapshot != last => {
                    if snapshots.send(snapshot.clone()).is_err() {
                        return;
                    }
                    last = snapshot;
                }
                Ok(_) => {}
                Err(error) => log::error!("Hyprland query: {error}"),
            }
        }
    });

    let (commands, received) = mpsc::channel();
    thread::spawn(move || {
        // Hyprland configured in Lua evaluates what it is asked to dispatch; one
        // on hyprlang wants `dispatcher args`. Lua is tried first and the answer kept.
        let mut legacy = false;
        for command in received {
            let (lua, old) = match command {
                Command::FocusWindow(address) => (
                    format!("hl.dsp.focus({{ window = \"address:{address}\" }})"),
                    format!("focuswindow address:{address}"),
                ),
                Command::FocusWorkspace(id) => {
                    let reference = id.to_string();
                    (format!("hl.dsp.focus({{ workspace = \"{reference}\" }})"), format!("workspace {reference}"))
                }
                Command::Screenshot => {
                    screenshot();
                    continue;
                }
                Command::Quit => {
                    // hyprshutdown closes applications gracefully first; the
                    // dispatcher is the fallback.
                    if std::process::Command::new("hyprshutdown").spawn().is_ok() {
                        continue;
                    }
                    ("hl.dsp.exit()".to_owned(), "exit".to_owned())
                }
                Command::Spawn(argv) => {
                    let line = argv.iter().map(|arg| shell_quote(arg)).collect::<Vec<_>>().join(" ");
                    let lua = serde_json::to_string(&line).unwrap_or_default();
                    (format!("hl.dsp.exec_cmd({lua})"), format!("exec {line}"))
                }
                Command::HideApps(apps) => {
                    let mut hider = hider.lock().unwrap();
                    hider.apps = apps.iter().map(|a| a.to_lowercase()).collect();
                    for (address, class) in clients() {
                        let listed = hider.listed(&class);
                        if listed && !hider.hidden(&address) {
                            set_hidden(&address, true);
                            hider.auto.insert(address);
                        } else if !listed && hider.auto.contains(&address) && !hider.manual.contains(&address) {
                            set_hidden(&address, false);
                            hider.auto.remove(&address);
                        }
                    }
                    let _ = stale.send(());
                    continue;
                }
                Command::ToggleHidden => {
                    let Some(address) =
                        query("activewindow").ok().and_then(|w| w["address"].as_str().map(str::to_owned))
                    else {
                        continue;
                    };
                    let mut hider = hider.lock().unwrap();
                    if hider.hidden(&address) {
                        set_hidden(&address, false);
                        hider.auto.remove(&address);
                        hider.manual.remove(&address);
                    } else {
                        set_hidden(&address, true);
                        hider.manual.insert(address);
                    }
                    let _ = stale.send(());
                    continue;
                }
                Command::SwitchLayout => {
                    match request("switchxkblayout current next") {
                        Ok(reply) if reply.trim() == "ok" => {}
                        other => log::error!("Hyprland could not switch layout: {other:?}"),
                    }
                    continue;
                }
            };
            let send = |legacy: bool| request(&format!("/dispatch {}", if legacy { &old } else { &lua }));
            match send(legacy) {
                Ok(reply) if reply.trim() == "ok" => {}
                Ok(reply) if !legacy && !reply.contains("hl.dispatch") => {
                    legacy = true;
                    if let Ok(reply) = send(true).map(|r| r.trim().to_owned())
                        && reply != "ok"
                    {
                        log::error!("Hyprland refused \"{old}\": {reply}");
                    }
                }
                other => log::error!("Hyprland refused a dispatch: {other:?}"),
            }
        }
    });
    commands
}

fn read_state(urgent: &mut Vec<String>, casting: bool, hidden: usize) -> std::io::Result<Snapshot> {
    let window = query("activewindow")?;
    let workspaces = query("workspaces")?;
    let monitors = query("monitors")?;
    let active = query("activeworkspace")?;
    let devices = query("devices")?;
    let clients = query("clients")?;

    let active_ids: Vec<i64> =
        monitors.as_array().into_iter().flatten().filter_map(|m| m["activeWorkspace"]["id"].as_i64()).collect();
    let focused = active["id"].as_i64();
    let focused_window = window["address"].as_str();

    // A window's `urgent` flag is not in the JSON; this tracks only what is
    // still open, so a closed window does not leave its workspace lit.
    urgent.retain(|address| clients.as_array().into_iter().flatten().any(|c| c["address"] == **address));
    let urgent_workspaces: Vec<i64> = clients
        .as_array()
        .into_iter()
        .flatten()
        .filter(|c| urgent.iter().any(|a| c["address"] == **a))
        .filter_map(|c| c["workspace"]["id"].as_i64())
        .collect();

    let mut list: Vec<Workspace> = workspaces
        .as_array()
        .into_iter()
        .flatten()
        .filter(|w| !w["name"].as_str().unwrap_or("").starts_with("special:"))
        .filter_map(|w| {
            let id = w["id"].as_i64()?;
            let name = w["name"].as_str().unwrap_or("");
            Some(Workspace {
                id,
                label: if id > 0 { id.to_string() } else { name.to_owned() },
                output: w["monitor"].as_str().unwrap_or("").to_owned(),
                active: active_ids.contains(&id),
                focused: Some(id) == focused,
                urgent: urgent_workspaces.contains(&id),
                windows: w["windows"].as_u64().unwrap_or(0) as usize,
            })
        })
        .collect();
    // Numbered workspaces in order, then named ones, per output.
    list.sort_by_key(|w| (w.output.clone(), w.id <= 0, w.id.abs()));

    let keyboards = devices["keyboards"].as_array().cloned().unwrap_or_default();
    let main = keyboards.iter().find(|k| k["main"] == true).or(keyboards.first());
    Ok(Snapshot {
        workspaces: list,
        layout: main.and_then(|k| k["active_keymap"].as_str()).unwrap_or("").to_owned(),
        layouts: main.and_then(|k| k["layout"].as_str()).map_or(0, |l| l.split(',').count()),
        title: window["title"].as_str().unwrap_or("").to_owned(),
        app: window["class"].as_str().unwrap_or("").to_owned(),
        casting,
        apps: {
            let mut apps: Vec<String> = clients
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|c| c["class"].as_str().filter(|c| !c.is_empty()).map(str::to_owned))
                .collect();
            apps.sort();
            apps.dedup();
            apps
        },
        hidden,
        windows: windows_of(&clients, &monitors, &active_ids, focused_window),
    })
}

/// Hyprland has no screenshot UI of its own: slurp picks the region, grim
/// takes it, and the file is both saved and put on the clipboard.
fn screenshot() {
    let script = r#"
        region=$(slurp) || exit 0
        dir="${XDG_PICTURES_DIR:-$HOME/Pictures}/Screenshots"
        mkdir -p "$dir" || exit 1
        file="$dir/Screenshot $(date '+%Y-%m-%d %H-%M-%S').png"
        grim -g "$region" "$file" && wl-copy --type image/png < "$file"
    "#;
    crate::services::launch(&["sh", "-c", script]);
}

/// One argument for `sh`: single-quoted, with any single quote spelled out.
fn shell_quote(arg: &str) -> String {
    if !arg.is_empty() && arg.chars().all(|c| c.is_ascii_alphanumeric() || "-_./=:@%+,".contains(c)) {
        return arg.to_owned();
    }
    format!("'{}'", arg.replace('\'', r"'\''"))
}

/// Open windows, the most recently focused first; those on a workspace
/// that is on screen know where, from their monitor's corner.
fn windows_of(clients: &Value, monitors: &Value, active: &[i64], focused: Option<&str>) -> Vec<Window> {
    let monitors = monitors.as_array().cloned().unwrap_or_default();
    let mut list: Vec<&Value> = clients
        .as_array()
        .into_iter()
        .flatten()
        .filter(|c| c["mapped"].as_bool().unwrap_or(true) && !c["hidden"].as_bool().unwrap_or(false))
        .filter(|c| !c["workspace"]["name"].as_str().unwrap_or("").starts_with("special:"))
        .collect();
    list.sort_by_key(|c| c["focusHistoryID"].as_i64().unwrap_or(i64::MAX));
    list.into_iter()
        .map(|c| {
            let monitor = monitors.iter().find(|m| m["id"] == c["monitor"]);
            let shown = c["workspace"]["id"].as_i64().is_some_and(|id| active.contains(&id)) || c["pinned"] == true;
            let pair = |v: &Value| Some((v[0].as_i64()? as i32, v[1].as_i64()? as i32));
            let rect = monitor.filter(|_| shown).and_then(|m| {
                let (x, y) = pair(&c["at"])?;
                let (w, h) = pair(&c["size"])?;
                Some((x - m["x"].as_i64()? as i32, y - m["y"].as_i64()? as i32, w, h))
            });
            let id = c["workspace"]["id"].as_i64().unwrap_or(0);
            let name = c["workspace"]["name"].as_str().unwrap_or("");
            let address = c["address"].as_str().unwrap_or("").to_owned();
            Window {
                focused: focused == Some(address.as_str()),
                id: address,
                title: c["title"].as_str().unwrap_or("").to_owned(),
                app: c["class"].as_str().unwrap_or("").to_owned(),
                workspace: if id > 0 { id.to_string() } else { name.to_owned() },
                output: monitor.and_then(|m| m["name"].as_str()).unwrap_or("").to_owned(),
                rect,
            }
        })
        .collect()
}
