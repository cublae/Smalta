//! Audio through PipeWire's PulseAudio server, by way of `pactl`.
//!
//! `pactl subscribe` says when anything changed; `pactl -f json list` says
//! what it is now. Talking to PipeWire directly would mean its C library.

use std::io::{BufRead, BufReader};
use std::process::{Command as Process, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use serde_json::Value;
use smithay_client_toolkit::reexports::calloop::channel::Sender;

use super::{Update, run, settle};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Device {
    pub name: String,
    pub description: String,
    pub volume: f32,
    pub muted: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Stream {
    pub index: u32,
    pub app: String,
    pub volume: f32,
    pub muted: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct State {
    pub available: bool,
    pub sinks: Vec<Device>,
    pub sources: Vec<Device>,
    pub default_sink: String,
    pub default_source: String,
    /// Applications playing.
    pub streams: Vec<Stream>,
    /// Applications listening to a microphone.
    pub recording: Vec<String>,
}

impl State {
    pub fn speaker(&self) -> Option<&Device> {
        self.sinks.iter().find(|d| d.name == self.default_sink).or(self.sinks.first())
    }

    pub fn microphone(&self) -> Option<&Device> {
        self.sources.iter().find(|d| d.name == self.default_source).or(self.sources.first())
    }
}

#[derive(Debug)]
pub enum Command {
    /// `true` for the speaker, `false` for the microphone.
    Volume(bool, f32),
    Mute(bool),
    Default(bool, String),
    StreamVolume(u32, f32),
    StreamMute(u32),
}

pub fn spawn(updates: Sender<Update>) -> mpsc::Sender<Command> {
    let (stale, refresh) = mpsc::channel::<()>();

    let marker = stale.clone();
    thread::spawn(move || {
        loop {
            let _ = marker.send(());
            let child = Process::new("pactl").arg("subscribe").stdout(Stdio::piped()).stderr(Stdio::null()).spawn();
            match child {
                Ok(mut child) => {
                    if let Some(stdout) = child.stdout.take() {
                        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                            // Clients come and go constantly and change nothing shown.
                            if !line.contains("client") && marker.send(()).is_err() {
                                let _ = child.kill();
                                return;
                            }
                        }
                    }
                    let _ = child.wait();
                }
                Err(error) => log::warn!("pactl: {error}; no audio controls"),
            }
            thread::sleep(Duration::from_secs(3));
        }
    });

    thread::spawn(move || {
        let mut last = None;
        while settle(&refresh, Duration::from_millis(40)) {
            let state = read();
            if last.as_ref() != Some(&state) {
                if updates.send(Update::Audio(state.clone())).is_err() {
                    return;
                }
                last = Some(state);
            }
        }
    });

    let (commands, received) = mpsc::channel();
    thread::spawn(move || {
        for command in received {
            let percent = |v: f32| format!("{}%", (v.clamp(0.0, 1.5) * 100.0).round() as i32);
            let target = |sink: bool| if sink { "@DEFAULT_SINK@" } else { "@DEFAULT_SOURCE@" };
            let kind = |sink: bool| if sink { "sink" } else { "source" };
            let args: Vec<String> = match command {
                Command::Volume(sink, v) => vec![format!("set-{}-volume", kind(sink)), target(sink).into(), percent(v)],
                Command::Mute(sink) => vec![format!("set-{}-mute", kind(sink)), target(sink).into(), "toggle".into()],
                Command::Default(sink, name) => vec![format!("set-default-{}", kind(sink)), name],
                Command::StreamVolume(i, v) => vec!["set-sink-input-volume".into(), i.to_string(), percent(v)],
                Command::StreamMute(i) => vec!["set-sink-input-mute".into(), i.to_string(), "toggle".into()],
            };
            let args: Vec<&str> = args.iter().map(String::as_str).collect();
            if run("pactl", &args).is_none() {
                log::warn!("pactl {} failed", args.join(" "));
            }
        }
    });
    commands
}

fn list(what: &str) -> Vec<Value> {
    run("pactl", &["-f", "json", "list", what])
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|value| value.as_array().cloned())
        .unwrap_or_default()
}

/// Mean of the channel volumes, 1.0 being 100%.
fn volume(value: &Value) -> f32 {
    let channels: Vec<f32> = value["volume"]
        .as_object()
        .into_iter()
        .flat_map(|map| map.values())
        .filter_map(|channel| channel["value"].as_f64())
        .map(|raw| raw as f32 / 65536.0)
        .collect();
    if channels.is_empty() { 0.0 } else { channels.iter().sum::<f32>() / channels.len() as f32 }
}

fn device(value: &Value) -> Device {
    Device {
        name: value["name"].as_str().unwrap_or("").to_owned(),
        description: value["description"].as_str().unwrap_or("").to_owned(),
        volume: volume(value),
        muted: value["mute"].as_bool().unwrap_or(false),
    }
}

fn app_name(value: &Value) -> String {
    let properties = &value["properties"];
    ["application.name", "media.name", "application.process.binary"]
        .iter()
        .find_map(|key| properties[*key].as_str())
        .unwrap_or("?")
        .to_owned()
}

fn read() -> State {
    let Some(info) = run("pactl", &["-f", "json", "info"]).and_then(|text| serde_json::from_str::<Value>(&text).ok())
    else {
        return State::default();
    };
    State {
        available: true,
        sinks: list("sinks").iter().map(device).collect(),
        // A monitor is a sink's output seen as a source: not a microphone.
        sources: list("sources")
            .iter()
            .filter(|s| !s["name"].as_str().unwrap_or("").ends_with(".monitor"))
            .map(device)
            .collect(),
        default_sink: info["default_sink_name"].as_str().unwrap_or("").to_owned(),
        default_source: info["default_source_name"].as_str().unwrap_or("").to_owned(),
        streams: list("sink-inputs")
            .iter()
            .map(|s| Stream {
                index: s["index"].as_u64().unwrap_or(0) as u32,
                app: app_name(s),
                volume: volume(s),
                muted: s["mute"].as_bool().unwrap_or(false),
            })
            .collect(),
        recording: list("source-outputs")
            .iter()
            // A level meter watching a monitor is not someone listening in.
            .filter(|s| !s["properties"]["stream.monitor"].as_str().is_some_and(|v| v == "true"))
            .map(app_name)
            .collect(),
    }
}
