//! Caps Lock and Num Lock, read off the keyboards' LEDs.
//!
//! A layer-shell surface hears the modifiers only while it has the keyboard,
//! which the bar never does. The compositor lights the LEDs for every
//! keyboard on the seat, and sysfs lets anyone read them, so the lights say
//! what the keyboard is doing wherever the focus is.

use std::path::PathBuf;
use std::thread;
use std::time::Duration;

use smithay_client_toolkit::reexports::calloop::channel::Sender;

use super::Update;

/// Each `None` when no keyboard has that light.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct State {
    pub caps: Option<bool>,
    pub num: Option<bool>,
}

/// The `brightness` files of every LED whose name ends in `suffix`.
fn leds(suffix: &str) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir("/sys/class/leds") else { return Vec::new() };
    entries
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().ends_with(suffix))
        .map(|e| e.path().join("brightness"))
        .collect()
}

/// Lit on any keyboard; `None` with no such light at all.
fn lit(paths: &[PathBuf]) -> Option<bool> {
    let values: Vec<bool> = paths
        .iter()
        .filter_map(|p| std::fs::read_to_string(p).ok())
        .map(|v| v.trim().parse::<u32>().is_ok_and(|v| v > 0))
        .collect();
    (!values.is_empty()).then(|| values.contains(&true))
}

pub fn spawn(updates: Sender<Update>) {
    thread::spawn(move || {
        let mut state = State::default();
        let mut tick = 0u32;
        let (mut caps, mut num) = (Vec::new(), Vec::new());
        loop {
            // Keyboards come and go: look for their lights again now and then.
            if tick.is_multiple_of(40) {
                caps = leds("::capslock");
                num = leds("::numlock");
            }
            tick = tick.wrapping_add(1);
            let next = State { caps: lit(&caps), num: lit(&num) };
            if next != state {
                state = next;
                if updates.send(Update::Locks(state)).is_err() {
                    return;
                }
            }
            thread::sleep(Duration::from_millis(150));
        }
    });
}
