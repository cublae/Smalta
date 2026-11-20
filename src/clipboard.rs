//! Clipboard history through cliphist, with what cliphist does not keep:
//! when each entry was copied, and pins.
//!
//! cliphist owns the storage and refuses entries marked as passwords; it only
//! needs `wl-paste --watch cliphist store` running, which the shell starts
//! unless something already does. A second watcher of the shell's own notes
//! the time of every copy against the id cliphist gave it. Pinned entries are
//! copied out to files of their own, so neither `cliphist wipe` nor its limit
//! on entries takes them.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug)]
pub struct Entry {
    /// The line cliphist printed, which is also what it wants back to act on
    /// it; empty for a pin cliphist no longer has.
    pub raw: String,
    pub preview: String,
    pub image: bool,
    /// Unix seconds, if the copy was seen.
    pub time: Option<i64>,
    /// The pin's file, under [`pins_dir`].
    pub pin: Option<String>,
}

impl Entry {
    /// What the cache of decoded previews knows this entry by.
    pub fn key(&self) -> &str {
        self.pin.as_deref().unwrap_or(&self.raw)
    }

    /// `646x1080` from an image's preview.
    pub fn dimensions(&self) -> Option<&str> {
        self.image.then(|| self.preview.split_whitespace().last()).flatten()
    }
}

#[derive(Default)]
pub struct Clipboard {
    watchers: Vec<Child>,
}

impl Clipboard {
    pub fn start(&mut self) {
        if Command::new("cliphist").arg("version").stdout(Stdio::null()).stderr(Stdio::null()).status().is_err() {
            log::info!("no cliphist; clipboard history is off");
            return;
        }
        self.watch_times();
        let running = crate::services::run("pgrep", &["-f", "wl-paste.*--watch cliphist store"])
            .is_some_and(|out| !out.trim().is_empty());
        if running {
            return;
        }
        for kind in ["text", "image"] {
            let child = Command::new("wl-paste")
                .args(["--type", kind, "--watch", "cliphist", "store"])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn();
            match child {
                Ok(child) => self.watchers.push(child),
                Err(error) => log::warn!("wl-paste: {error}"),
            }
        }
    }

    /// A line on our pipe for every copy; a moment later cliphist has stored
    /// it, and its newest id gets the time.
    fn watch_times(&mut self) {
        let child = Command::new("wl-paste")
            .args(["--watch", "sh", "-c", "cat >/dev/null; echo"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn();
        let mut child = match child {
            Ok(child) => child,
            Err(error) => return log::warn!("wl-paste: {error}"),
        };
        let Some(stdout) = child.stdout.take() else { return };
        self.watchers.push(child);
        std::thread::spawn(move || {
            // The first line is wl-paste reporting what was there when it started.
            for _ in BufReader::new(stdout).lines().skip(1) {
                std::thread::sleep(Duration::from_millis(300));
                let Some(list) = crate::services::run("cliphist", &["list"]) else { continue };
                let ids: Vec<&str> = list.lines().filter_map(|l| l.split_once('\t')).map(|(id, _)| id).collect();
                let Some(newest) = ids.first() else { continue };
                let mut times = load_times();
                times.entry((*newest).to_owned()).or_insert_with(now);
                // Forget the ids cliphist has let go of.
                times.retain(|id, _| ids.contains(&id.as_str()));
                save(&times_path(), &times);
            }
        });
    }

    pub fn stop(&mut self) {
        for mut child in self.watchers.drain(..) {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

fn home_dir(variable: &str, fallback: &str) -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    PathBuf::from(std::env::var(variable).unwrap_or_else(|_| format!("{home}/{fallback}"))).join("smalta")
}

fn times_path() -> PathBuf {
    home_dir("XDG_STATE_HOME", ".local/state").join("clipboard-times.json")
}

fn pins_dir() -> PathBuf {
    home_dir("XDG_DATA_HOME", ".local/share").join("pins")
}

fn load_times() -> HashMap<String, i64> {
    std::fs::read_to_string(times_path()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

fn save<T: Serialize>(path: &PathBuf, value: &T) {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(text) = serde_json::to_string(value) {
        let _ = std::fs::write(path, text);
    }
}

#[derive(Clone, Serialize, Deserialize)]
struct Pin {
    file: String,
    preview: String,
    image: bool,
    time: Option<i64>,
}

fn load_pins() -> Vec<Pin> {
    std::fs::read_to_string(pins_dir().join("index.json"))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

fn save_pins(pins: &[Pin]) {
    save(&pins_dir().join("index.json"), &pins);
}

/// Pins first, newest pinned first, then the history newest first. A pin
/// stands in for the history entry it was made from.
pub fn list() -> Vec<Entry> {
    let times = load_times();
    let history: Vec<Entry> = crate::services::run("cliphist", &["list"])
        .unwrap_or_default()
        .lines()
        .filter_map(|line| {
            let (id, preview) = line.split_once('\t')?;
            let image = preview.starts_with("[[ binary data ");
            let preview = if image {
                preview.trim_start_matches("[[ binary data ").trim_end_matches(" ]]").to_owned()
            } else {
                // cliphist cuts long text at a byte count, sometimes inside a letter.
                preview.trim_end_matches(char::REPLACEMENT_CHARACTER).split_whitespace().collect::<Vec<_>>().join(" ")
            };
            Some(Entry { raw: line.to_owned(), preview, image, time: times.get(id).copied(), pin: None })
        })
        .collect();
    let mut entries: Vec<Entry> = load_pins()
        .into_iter()
        .rev()
        .map(|pin| {
            let raw = history.iter().find(|e| e.preview == pin.preview).map(|e| e.raw.clone()).unwrap_or_default();
            Entry { raw, preview: pin.preview, image: pin.image, time: pin.time, pin: Some(pin.file) }
        })
        .collect();
    let pinned: Vec<String> = entries.iter().map(|p| p.raw.clone()).collect();
    entries.extend(history.into_iter().filter(|e| !pinned.contains(&e.raw)));
    entries
}

fn pipe(program: &str, args: &[&str], input: &[u8]) -> Option<Vec<u8>> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    child.stdin.take()?.write_all(input).ok()?;
    let output = child.wait_with_output().ok()?;
    output.status.success().then_some(output.stdout)
}

/// What was copied, as it was copied.
pub fn contents(entry: &Entry) -> Option<Vec<u8>> {
    match &entry.pin {
        Some(file) => std::fs::read(pins_dir().join(file)).ok(),
        None => pipe("cliphist", &["decode"], entry.raw.as_bytes()),
    }
}

pub fn copy_entry(entry: &Entry) {
    if let Some(bytes) = contents(entry) {
        copy_bytes(&bytes);
    }
}

/// Gone from the history and from the pins both.
pub fn delete(entry: &Entry) {
    if entry.pin.is_some() {
        unpin(entry);
    }
    if !entry.raw.is_empty() {
        let _ = pipe("cliphist", &["delete"], entry.raw.as_bytes());
    }
}

pub fn pin(entry: &Entry) {
    let Some(bytes) = contents(entry) else { return };
    let id = entry.raw.split('\t').next().unwrap_or("0");
    let file = format!("{}-{id}", now());
    let _ = std::fs::create_dir_all(pins_dir());
    if std::fs::write(pins_dir().join(&file), bytes).is_err() {
        return;
    }
    let mut pins = load_pins();
    pins.push(Pin { file, preview: entry.preview.clone(), image: entry.image, time: entry.time });
    save_pins(&pins);
}

pub fn unpin(entry: &Entry) {
    let Some(file) = &entry.pin else { return };
    let _ = std::fs::remove_file(pins_dir().join(file));
    let mut pins = load_pins();
    pins.retain(|p| &p.file != file);
    save_pins(&pins);
}

/// The whole history; pins stay.
pub fn wipe() {
    let _ = crate::services::run("cliphist", &["wipe"]);
}

/// wl-copy forks itself into the background to keep serving the selection,
/// so this returns as soon as it has the data.
pub fn copy_bytes(bytes: &[u8]) {
    if pipe("wl-copy", &[], bytes).is_none() {
        log::warn!("wl-copy failed");
    }
}

/// Like [`copy_bytes`], with the media type said rather than guessed.
pub fn copy_typed(bytes: &[u8], mime: &str) {
    if pipe("wl-copy", &["--type", mime], bytes).is_none() {
        log::warn!("wl-copy failed");
    }
}
