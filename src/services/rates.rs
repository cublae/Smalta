//! Exchange rates for the launcher's converter, per US dollar, from
//! open.er-api.com, which needs no key.
//!
//! Nothing is fetched until a conversion between currencies is typed; the
//! answer is kept on disk for half a day, so most of them are not fetched at all.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, SystemTime};

use smithay_client_toolkit::reexports::calloop::channel::Sender;

use super::Update;

pub type Rates = HashMap<String, f64>;

const URL: &str = "https://open.er-api.com/v6/latest/USD";
const FRESH: Duration = Duration::from_secs(12 * 3600);

fn path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    let cache = std::env::var("XDG_CACHE_HOME").unwrap_or_else(|_| format!("{home}/.cache"));
    PathBuf::from(cache).join("smalta/rates.json")
}

fn parse(text: &str) -> Option<Rates> {
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    let rates = value["rates"].as_object()?;
    Some(rates.iter().filter_map(|(code, rate)| Some((code.clone(), rate.as_f64()?))).collect())
}

/// The kept rates, and whether they are recent enough to use without asking again.
fn cached() -> Option<(Rates, bool)> {
    let path = path();
    let age = std::fs::metadata(&path).and_then(|m| m.modified()).ok()?.elapsed().unwrap_or(Duration::MAX);
    let rates = parse(&std::fs::read_to_string(&path).ok()?)?;
    Some((rates, age < FRESH))
}

fn fetch() -> Option<Rates> {
    let text = super::run("curl", &["-sf", "--max-time", "10", URL])?;
    let rates = parse(&text)?;
    let path = path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(&path, text);
    Some(rates)
}

/// Waits to be asked; then sends the rates, from the cache or the network.
pub fn spawn(updates: Sender<Update>) -> mpsc::Sender<()> {
    let (asks, asked) = mpsc::channel::<()>();
    thread::spawn(move || {
        let mut last: Option<SystemTime> = None;
        while asked.recv().is_ok() {
            while asked.try_recv().is_ok() {}
            // Once a minute at most, whatever the launcher asks.
            if last.is_some_and(|t| t.elapsed().unwrap_or_default() < Duration::from_secs(60)) {
                continue;
            }
            last = Some(SystemTime::now());
            let rates = match cached() {
                Some((rates, true)) => Some(rates),
                // Stale is better than none when the network is down.
                Some((stale, false)) => fetch().or(Some(stale)),
                None => fetch(),
            };
            match rates {
                Some(rates) => {
                    if updates.send(Update::Rates(rates)).is_err() {
                        return;
                    }
                }
                None => log::warn!("no exchange rates from {URL}"),
            }
        }
    });
    asks
}
