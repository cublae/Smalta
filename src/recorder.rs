//! Screen recording through gpu-screen-recorder.
//!
//! Recording stops with SIGINT: the recorder writes the container's index on
//! interrupt, where killing it leaves an unplayable file. Capture goes
//! through the desktop portal, and its session token is kept so the picker
//! appears once rather than for every recording.

use std::process::{Child, Command, Stdio};
use std::time::Instant;

#[derive(Default)]
pub struct Recorder {
    child: Option<Child>,
    pub started: Option<Instant>,
    pub path: String,
}

impl Recorder {
    pub fn recording(&mut self) -> bool {
        if let Some(child) = &mut self.child
            && let Ok(Some(status)) = child.try_wait()
        {
            if !status.success() {
                log::warn!("gpu-screen-recorder exited with {status}");
            }
            self.child = None;
            self.started = None;
        }
        self.child.is_some()
    }

    pub fn toggle(&mut self) {
        if self.recording() { self.stop() } else { self.start() }
    }

    fn start(&mut self) {
        let home = std::env::var("HOME").unwrap_or_default();
        let directory = format!("{home}/Videos");
        let _ = std::fs::create_dir_all(&directory);
        self.path = format!("{directory}/{}.mp4", chrono::Local::now().format("%Y-%m-%d %H-%M-%S"));
        let config = std::env::var("XDG_CONFIG_HOME").unwrap_or_else(|_| format!("{home}/.config"));
        let _ = std::fs::create_dir_all(format!("{config}/smalta"));
        let token = format!("{config}/smalta/portal-session-token");
        let child = Command::new("gpu-screen-recorder")
            .args(["-w", "portal", "-f", "60", "-q", "very_high", "-k", "auto", "-a", "default_output"])
            .args(["-o", &self.path, "-restore-portal-session", "yes", "-portal-session-token-filepath", &token])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        match child {
            Ok(child) => {
                self.child = Some(child);
                self.started = Some(Instant::now());
            }
            Err(error) => log::error!("gpu-screen-recorder: {error}"),
        }
    }

    fn stop(&mut self) {
        if let Some(child) = &self.child {
            let _ = Command::new("kill").args(["-INT", &child.id().to_string()]).status();
        }
    }

    pub fn elapsed(&self) -> String {
        let secs = self.started.map_or(0, |at| at.elapsed().as_secs());
        format!("{:02}:{:02}", secs / 60, secs % 60)
    }
}
