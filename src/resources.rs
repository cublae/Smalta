//! CPU, memory and temperature, read from /proc and /sys, with a short history
//! for the desktop graphs.

use std::collections::VecDeque;

/// Samples kept for the graphs: two minutes at one every two seconds.
pub const HISTORY: usize = 60;

#[derive(Default)]
pub struct Resources {
    pub cpu: f32,
    pub memory: f32,
    /// Used and total memory, in GiB.
    pub memory_gib: (f32, f32),
    /// Degrees Celsius, when there is a sensor worth reading.
    pub temperature: Option<f32>,
    pub cpu_history: VecDeque<f32>,
    pub memory_history: VecDeque<f32>,
    pub temperature_history: VecDeque<f32>,
    last: Option<(u64, u64)>,
}

fn push(history: &mut VecDeque<f32>, value: f32) {
    if history.len() == HISTORY {
        history.pop_front();
    }
    history.push_back(value);
}

impl Resources {
    pub fn sample(&mut self) {
        if let Some((busy, total)) = cpu_times() {
            if let Some((last_busy, last_total)) = self.last {
                let span = total.saturating_sub(last_total);
                if span > 0 {
                    self.cpu = busy.saturating_sub(last_busy) as f32 / span as f32;
                }
            }
            self.last = Some((busy, total));
        }
        if let Some((used, total)) = memory() {
            self.memory = used / total;
            self.memory_gib = (used / 1048576.0, total / 1048576.0);
        }
        self.temperature = temperature();
        push(&mut self.cpu_history, self.cpu);
        push(&mut self.memory_history, self.memory);
        if let Some(t) = self.temperature {
            // Graphed against 100 °C.
            push(&mut self.temperature_history, t / 100.0);
        }
    }
}

/// Busy and total jiffies across every CPU.
fn cpu_times() -> Option<(u64, u64)> {
    let stat = std::fs::read_to_string("/proc/stat").ok()?;
    let line = stat.lines().next()?;
    let values: Vec<u64> = line.split_whitespace().skip(1).filter_map(|v| v.parse().ok()).collect();
    let total: u64 = values.iter().sum();
    // idle and iowait.
    let idle = values.get(3).copied().unwrap_or(0) + values.get(4).copied().unwrap_or(0);
    Some((total - idle, total))
}

/// Used and total, in KiB.
fn memory() -> Option<(f32, f32)> {
    let info = std::fs::read_to_string("/proc/meminfo").ok()?;
    let field = |name: &str| {
        info.lines()
            .find(|line| line.starts_with(name))
            .and_then(|line| line.split_whitespace().nth(1))
            .and_then(|v| v.parse::<f32>().ok())
    };
    let total = field("MemTotal:")?;
    let available = field("MemAvailable:")?;
    Some((total - available, total))
}

/// The CPU package sensor, in order of how much it means what it says.
fn temperature() -> Option<f32> {
    let sensors: Vec<_> = std::fs::read_dir("/sys/class/hwmon").ok()?.flatten().map(|e| e.path()).collect();
    for wanted in ["k10temp", "zenpower", "coretemp", "cpu_thermal", "acpitz"] {
        for dir in &sensors {
            let name = std::fs::read_to_string(dir.join("name")).unwrap_or_default();
            if name.trim() == wanted
                && let Some(milli) =
                    std::fs::read_to_string(dir.join("temp1_input")).ok().and_then(|t| t.trim().parse::<f32>().ok())
            {
                return Some(milli / 1000.0);
            }
        }
    }
    None
}
