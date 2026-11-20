//! Who has a camera open.
//!
//! Every program that films -- a browser, OBS, PipeWire on someone's behalf
//! -- holds a `/dev/video*` device open, and `/proc` says which. Only the
//! user's own processes can be looked into, which are the ones that matter.
//! A look every couple of seconds costs a few milliseconds.

use std::thread;
use std::time::Duration;

use smithay_client_toolkit::reexports::calloop::channel::Sender;

use super::Update;

/// The names of the programs with a camera open, each once.
pub type State = Vec<String>;

fn read() -> State {
    let mut users = Vec::new();
    let Ok(processes) = std::fs::read_dir("/proc") else { return users };
    for process in processes.flatten() {
        let pid = process.file_name();
        if !pid.to_string_lossy().bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        let Ok(fds) = std::fs::read_dir(process.path().join("fd")) else { continue };
        let filming = fds.flatten().any(|fd| {
            std::fs::read_link(fd.path()).is_ok_and(|target| target.to_string_lossy().starts_with("/dev/video"))
        });
        if filming {
            let name = std::fs::read_to_string(process.path().join("comm")).unwrap_or_default().trim().to_owned();
            // PipeWire opens the camera for whoever asked it; the asker is
            // not visible from here, so the camera is at least shown as on.
            if !name.is_empty() && !users.contains(&name) {
                users.push(name);
            }
        }
    }
    users
}

pub fn spawn(updates: Sender<Update>) {
    thread::spawn(move || {
        let mut last = None;
        loop {
            let state = read();
            if last.as_ref() != Some(&state) {
                if updates.send(Update::Camera(state.clone())).is_err() {
                    return;
                }
                last = Some(state);
            }
            thread::sleep(Duration::from_secs(2));
        }
    });
}
