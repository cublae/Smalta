//! Media players over MPRIS on the session bus.

use std::collections::HashMap;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use smithay_client_toolkit::reexports::calloop::channel::Sender;
use zbus::blocking::Connection;
use zbus::zvariant::OwnedValue;

use super::Update;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Player {
    pub bus: String,
    pub identity: String,
    pub title: String,
    pub artist: String,
    pub playing: bool,
    /// Seconds.
    pub position: f64,
    pub length: f64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct State {
    pub players: Vec<Player>,
}

impl State {
    /// The one worth showing: whichever is playing, else the first.
    pub fn current(&self) -> Option<&Player> {
        self.players.iter().find(|p| p.playing).or(self.players.first())
    }
}

#[derive(Debug)]
pub enum Command {
    PlayPause(String),
    Next(String),
    Previous(String),
}

fn read(connection: &Connection) -> zbus::Result<State> {
    let bus = super::proxy(connection, "org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus")?;
    let names: Vec<String> = bus.call("ListNames", &())?;
    let mut players = Vec::new();
    for name in names.into_iter().filter(|n| n.starts_with("org.mpris.MediaPlayer2.")) {
        let root = super::proxy(connection, name.clone(), "/org/mpris/MediaPlayer2", "org.mpris.MediaPlayer2")?;
        let player =
            super::proxy(connection, name.clone(), "/org/mpris/MediaPlayer2", "org.mpris.MediaPlayer2.Player")?;
        let metadata: HashMap<String, OwnedValue> = player.get_property("Metadata").unwrap_or_default();
        let status: String = player.get_property("PlaybackStatus").unwrap_or_default();
        let position: i64 = player.get_property("Position").unwrap_or(0);
        let artist = metadata
            .get("xesam:artist")
            .and_then(|v| <Vec<String>>::try_from(v.try_clone().ok()?).ok())
            .map(|list| list.join(", "))
            .unwrap_or_default();
        let length = metadata
            .get("mpris:length")
            .and_then(|v| i64::try_from(v).ok().or_else(|| u64::try_from(v).ok().map(|u| u as i64)))
            .unwrap_or(0);
        players.push(Player {
            identity: root
                .get_property("Identity")
                .unwrap_or_else(|_| name.trim_start_matches("org.mpris.MediaPlayer2.").to_owned()),
            title: super::bluetooth::get::<&str>(&metadata, "xesam:title").unwrap_or("").to_owned(),
            artist,
            playing: status == "Playing",
            position: position as f64 / 1e6,
            length: length as f64 / 1e6,
            bus: name,
        });
    }
    Ok(State { players })
}

pub fn spawn(updates: Sender<Update>) -> mpsc::Sender<Command> {
    let (commands, received) = mpsc::channel::<Command>();
    thread::spawn(move || {
        let connection = match Connection::session() {
            Ok(connection) => connection,
            Err(error) => {
                log::warn!("session bus: {error}; no media controls");
                return;
            }
        };
        let mut last = None;
        loop {
            let state = read(&connection).unwrap_or_default();
            if last.as_ref() != Some(&state) {
                if updates.send(Update::Media(state.clone())).is_err() {
                    return;
                }
                last = Some(state);
            }
            let Ok(command) = received.recv_timeout(Duration::from_secs(1)) else { continue };
            let (bus, method) = match command {
                Command::PlayPause(bus) => (bus, "PlayPause"),
                Command::Next(bus) => (bus, "Next"),
                Command::Previous(bus) => (bus, "Previous"),
            };
            let result = super::proxy(&connection, bus, "/org/mpris/MediaPlayer2", "org.mpris.MediaPlayer2.Player")
                .and_then(|p| p.call_method(method, &()));
            if let Err(error) = result {
                log::warn!("media {method}: {error}");
            }
        }
    });
    commands
}
