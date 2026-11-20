//! The weather from Open-Meteo, which needs no key: fetched with curl every
//! half hour, for the configured place.

use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use serde_json::Value;
use smithay_client_toolkit::reexports::calloop::channel::Sender;

use super::{Update, run};
use crate::config;
use crate::icons::{self, Icon};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Now {
    pub temperature: f64,
    pub feels: f64,
    pub humidity: f64,
    pub wind: f64,
    pub code: u32,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Day {
    /// Monday is 0.
    pub weekday: u32,
    pub low: f64,
    pub high: f64,
    pub code: u32,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct State {
    pub place: String,
    pub current: Option<Now>,
    /// Hour of day and temperature, for the next twelve hours.
    pub hours: Vec<(u32, f64, u32)>,
    pub days: Vec<Day>,
    pub error: Option<String>,
    pub configured: bool,
}

#[derive(Debug)]
pub enum Command {
    Refresh,
}

pub fn icon(code: u32) -> Icon {
    match code {
        0 | 1 => icons::SUN,
        2 => icons::PARTLY,
        3 => icons::CLOUD,
        45 | 48 => icons::FOG,
        51..=67 | 80..=82 => icons::RAIN,
        71..=77 | 85 | 86 => icons::SNOW,
        95..=99 => icons::STORM,
        _ => icons::CLOUD,
    }
}

pub fn describe(code: u32) -> &'static str {
    let russian = crate::i18n::russian();
    let (en, ru) = match code {
        0 => ("clear", "ясно"),
        1 => ("mostly clear", "малооблачно"),
        2 => ("partly cloudy", "переменная облачность"),
        3 => ("overcast", "пасмурно"),
        45 | 48 => ("fog", "туман"),
        51..=57 => ("drizzle", "морось"),
        61..=67 => ("rain", "дождь"),
        71..=77 => ("snow", "снег"),
        80..=82 => ("showers", "ливень"),
        85 | 86 => ("snow showers", "снегопад"),
        95..=99 => ("thunderstorm", "гроза"),
        _ => ("unknown", "неизвестно"),
    };
    if russian { ru } else { en }
}

fn get(url: &str) -> Option<Value> {
    let text = run("curl", &["-sf", "--max-time", "15", url])?;
    serde_json::from_str(&text).ok()
}

fn encode(text: &str) -> String {
    text.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

fn read(config: &config::Weather) -> State {
    let mut state = State { configured: true, ..State::default() };
    let (latitude, longitude, place) = if config.latitude != 0.0 || config.longitude != 0.0 {
        (config.latitude, config.longitude, config.location.clone())
    } else {
        let language = if crate::i18n::russian() { "ru" } else { "en" };
        let url = format!(
            "https://geocoding-api.open-meteo.com/v1/search?count=1&language={language}&name={}",
            encode(&config.location)
        );
        let Some(found) = get(&url).and_then(|v| v["results"].get(0).cloned()) else {
            state.error = Some(format!("{}?", config.location));
            return state;
        };
        (
            found["latitude"].as_f64().unwrap_or(0.0),
            found["longitude"].as_f64().unwrap_or(0.0),
            found["name"].as_str().unwrap_or("").to_owned(),
        )
    };
    state.place = place;
    let units = if config.units == "imperial" { "&temperature_unit=fahrenheit&wind_speed_unit=mph" } else { "" };
    let url = format!(
        "https://api.open-meteo.com/v1/forecast?latitude={latitude}&longitude={longitude}&timezone=auto&forecast_days=5\
         &current=temperature_2m,apparent_temperature,relative_humidity_2m,weather_code,wind_speed_10m\
         &hourly=temperature_2m,weather_code&daily=weather_code,temperature_2m_max,temperature_2m_min{units}"
    );
    let Some(data) = get(&url) else {
        state.error = Some("open-meteo.com?".into());
        return state;
    };
    let c = &data["current"];
    state.current = Some(Now {
        temperature: c["temperature_2m"].as_f64().unwrap_or(0.0),
        feels: c["apparent_temperature"].as_f64().unwrap_or(0.0),
        humidity: c["relative_humidity_2m"].as_f64().unwrap_or(0.0),
        wind: c["wind_speed_10m"].as_f64().unwrap_or(0.0),
        code: c["weather_code"].as_u64().unwrap_or(0) as u32,
    });

    // Hourly entries are local times as text; start from the current hour.
    let now = c["time"].as_str().unwrap_or("").get(..13).unwrap_or("").to_owned();
    let times = data["hourly"]["time"].as_array().cloned().unwrap_or_default();
    let start = times.iter().position(|t| t.as_str().is_some_and(|t| t.starts_with(&now))).unwrap_or(0);
    for i in start..(start + 12).min(times.len()) {
        let hour = times[i].as_str().and_then(|t| t.get(11..13)).and_then(|h| h.parse().ok()).unwrap_or(0);
        let temp = data["hourly"]["temperature_2m"][i].as_f64().unwrap_or(0.0);
        let code = data["hourly"]["weather_code"][i].as_u64().unwrap_or(0) as u32;
        state.hours.push((hour, temp, code));
    }
    let daily = &data["daily"];
    for (i, date) in daily["time"].as_array().cloned().unwrap_or_default().iter().enumerate() {
        let weekday = date
            .as_str()
            .and_then(|d| chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").ok())
            .map_or(0, |d| chrono::Datelike::weekday(&d).num_days_from_monday());
        state.days.push(Day {
            weekday,
            low: daily["temperature_2m_min"][i].as_f64().unwrap_or(0.0),
            high: daily["temperature_2m_max"][i].as_f64().unwrap_or(0.0),
            code: daily["weather_code"][i].as_u64().unwrap_or(0) as u32,
        });
    }
    state
}

pub fn spawn(updates: Sender<Update>, config: config::Weather) -> mpsc::Sender<Command> {
    let (commands, received) = mpsc::channel::<Command>();
    thread::spawn(move || {
        if config.location.is_empty() && config.latitude == 0.0 && config.longitude == 0.0 {
            return;
        }
        let interval = Duration::from_secs(config.interval.max(5) as u64 * 60);
        loop {
            let state = read(&config);
            // A failed fetch keeps showing the last good one.
            let retry = state.current.is_none();
            if updates.send(Update::Weather(state)).is_err() {
                return;
            }
            let wait = if retry { Duration::from_secs(120) } else { interval };
            let _ = received.recv_timeout(wait);
        }
    });
    commands
}
