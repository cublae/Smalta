//! Bars that move with the music, from cava.
//!
//! cava listens to whatever the sound server plays and writes each frame as a
//! line of levels, `3;7;1;…`. It runs only while something plays and the bar
//! has a media module to show the bars in; otherwise there is no process and
//! nothing redraws.

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};

use smithay_client_toolkit::reexports::calloop::channel::Sender;

/// The tallest a bar gets.
pub const TOP: u8 = 7;

#[derive(Default)]
pub struct Visualizer {
    child: Option<Child>,
    /// The latest frame: one level, 0 to [`TOP`], per bar.
    pub levels: Vec<u8>,
}

impl Visualizer {
    /// cava running when `wanted`, with `bars` bars, sending its frames to `frames`.
    pub fn want(&mut self, wanted: bool, bars: u32, frames: &Sender<Vec<u8>>) {
        match (wanted, self.child.is_some()) {
            (true, false) => self.start(bars, frames),
            (false, true) => self.stop(),
            _ => {}
        }
    }

    fn start(&mut self, bars: u32, frames: &Sender<Vec<u8>>) {
        let runtime = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".into());
        let path = std::path::Path::new(&runtime).join("smalta-cava.conf");
        let config = format!(
            "[general]\nbars = {}\nframerate = 30\nsleep_timer = 1\n\n[output]\nmethod = raw\nraw_target = /dev/stdout\n\
             data_format = ascii\nascii_max_range = {TOP}\nbar_delimiter = 59\nframe_delimiter = 10\n",
            bars.clamp(4, 64)
        );
        if let Err(error) = std::fs::write(&path, config) {
            log::warn!("{}: {error}", path.display());
            return;
        }
        let child = Command::new("cava")
            .arg("-p")
            .arg(&path)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn();
        let mut child = match child {
            Ok(child) => child,
            Err(error) => {
                log::info!("cava: {error}; no visualizer");
                return;
            }
        };
        let Some(stdout) = child.stdout.take() else { return };
        let frames = frames.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                let levels = line.split(';').filter_map(|v| v.trim().parse::<u8>().ok()).map(|v| v.min(TOP)).collect();
                if frames.send(levels).is_err() {
                    break;
                }
            }
        });
        self.child = Some(child);
    }

    pub fn running(&self) -> bool {
        self.child.is_some()
    }

    pub fn stop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            std::thread::spawn(move || child.wait());
        }
        self.levels.clear();
    }
}

impl Drop for Visualizer {
    fn drop(&mut self) {
        self.stop();
    }
}
