//! Desktop entries: what the launcher can start.
//!
//! Every `applications` directory on `XDG_DATA_DIRS` is read, the user's own
//! first, and an entry id seen once hides any later file of the same id --
//! the way the spec has a user override a system entry.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct App {
    pub id: String,
    pub name: String,
    pub generic: String,
    pub comment: String,
    pub keywords: String,
    pub exec: String,
    pub terminal: bool,
    pub icon: String,
    pub path: PathBuf,
}

fn data_dirs() -> Vec<PathBuf> {
    let home = std::env::var("HOME").unwrap_or_default();
    let user = std::env::var("XDG_DATA_HOME").unwrap_or_else(|_| format!("{home}/.local/share"));
    let system = std::env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".into());
    std::iter::once(user.as_str())
        .chain(system.split(':'))
        .filter(|d| !d.is_empty())
        .map(|d| Path::new(d).join("applications"))
        .collect()
}

fn walk(dir: &Path, prefix: &str, out: &mut Vec<(String, PathBuf)>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if path.is_dir() {
            walk(&path, &format!("{prefix}{name}-"), out);
        } else if let Some(stem) = name.strip_suffix(".desktop") {
            out.push((format!("{prefix}{stem}"), path));
        }
    }
}

pub fn load() -> Vec<App> {
    let russian = crate::i18n::russian();
    let mut seen = HashSet::new();
    let mut apps = Vec::new();
    for dir in data_dirs() {
        let mut files = Vec::new();
        walk(&dir, "", &mut files);
        for (id, path) in files {
            if !seen.insert(id.clone()) {
                continue;
            }
            if let Some(app) = parse(&id, &path, russian) {
                apps.push(app);
            }
        }
    }
    apps.sort_by_key(|a| a.name.to_lowercase());
    apps
}

fn parse(id: &str, path: &Path, russian: bool) -> Option<App> {
    let text = std::fs::read_to_string(path).ok()?;
    let mut fields: HashMap<String, String> = HashMap::new();
    let mut in_entry = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_entry = line == "[Desktop Entry]";
            continue;
        }
        if !in_entry || line.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            fields.entry(key.trim().to_owned()).or_insert_with(|| value.trim().to_owned());
        }
    }
    let get = |key: &str| {
        let localized = if russian {
            fields.get(&format!("{key}[ru]")).or_else(|| fields.get(&format!("{key}[ru_RU]")))
        } else {
            None
        };
        localized.or_else(|| fields.get(key)).cloned().unwrap_or_default()
    };
    let flag = |key: &str| fields.get(key).is_some_and(|v| v == "true");
    if get("Type") != "Application" || flag("NoDisplay") || flag("Hidden") || get("Exec").is_empty() {
        return None;
    }
    // OnlyShowIn and NotShowIn name desktops this shell is not.
    if fields.contains_key("OnlyShowIn") {
        return None;
    }
    Some(App {
        id: id.to_owned(),
        name: get("Name"),
        generic: get("GenericName"),
        comment: get("Comment"),
        keywords: get("Keywords").replace(';', " "),
        exec: get("Exec"),
        terminal: flag("Terminal"),
        icon: get("Icon"),
        path: path.to_owned(),
    })
}

/// The command line from `Exec=`, split the way the spec quotes it, with the
/// field codes for files and URLs dropped: the launcher opens nothing.
pub fn argv(app: &App) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    let mut has = false;
    let mut chars = app.exec.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                quoted = !quoted;
                has = true;
            }
            '\\' if quoted => {
                if let Some(next) = chars.next() {
                    current.push(next);
                }
            }
            ' ' if !quoted => {
                if has || !current.is_empty() {
                    args.push(std::mem::take(&mut current));
                    has = false;
                }
            }
            '%' => match chars.next() {
                Some('%') => current.push('%'),
                Some('c') => current.push_str(&app.name),
                Some('k') => current.push_str(&app.path.to_string_lossy()),
                Some('i') if !app.icon.is_empty() => {
                    args.push("--icon".into());
                    current.push_str(&app.icon);
                }
                _ => {}
            },
            _ => current.push(c),
        }
    }
    if has || !current.is_empty() {
        args.push(current);
    }
    if app.terminal {
        let terminal = std::env::var("TERMINAL").unwrap_or_else(|_| "kitty".into());
        args.splice(0..0, [terminal, "-e".into()]);
    }
    args
}

/// How many times each entry was started, to put the habitual ones first.
pub struct Launches {
    path: PathBuf,
    pub counts: HashMap<String, u32>,
}

impl Launches {
    pub fn load() -> Self {
        let home = std::env::var("HOME").unwrap_or_default();
        let state = std::env::var("XDG_STATE_HOME").unwrap_or_else(|_| format!("{home}/.local/state"));
        let path = Path::new(&state).join("smalta/launches.json");
        let counts =
            std::fs::read_to_string(&path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        Self { path, counts }
    }

    pub fn bump(&mut self, id: &str) {
        *self.counts.entry(id.to_owned()).or_default() += 1;
        if let Some(dir) = self.path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(text) = serde_json::to_string(&self.counts) {
            let _ = std::fs::write(&self.path, text);
        }
    }
}
