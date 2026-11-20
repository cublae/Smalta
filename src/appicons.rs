//! Application icons from the icon theme, brought down to pixel art.
//!
//! The theme is the one GTK is set to, followed through `Inherits=` to
//! hicolor, and then the loose `pixmaps` directories. An icon is rendered or
//! shrunk to exactly `size` shell pixels, and its edges are cut to whole
//! pixels: half-transparent fringes are what make a small icon look smeared
//! next to the bitmap font.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use resvg::{tiny_skia, usvg};

use crate::services::tray::Pixmap;

struct Dir {
    path: String,
    min: i32,
    max: i32,
}

struct Theme {
    /// Every place the theme lives: `~/.icons/Tela`, `/usr/share/icons/Tela`, …
    roots: Vec<PathBuf>,
    dirs: Vec<Dir>,
}

fn icon_bases() -> Vec<PathBuf> {
    let home = std::env::var("HOME").unwrap_or_default();
    let user = std::env::var("XDG_DATA_HOME").unwrap_or_else(|_| format!("{home}/.local/share"));
    let system = std::env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".into());
    std::iter::once(format!("{home}/.icons"))
        .chain(std::iter::once(user).chain(system.split(':').map(str::to_owned)).map(|d| format!("{d}/icons")))
        .filter(|d| Path::new(d).is_dir())
        .map(PathBuf::from)
        .collect()
}

fn pixmap_dirs() -> Vec<PathBuf> {
    let system = std::env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".into());
    system.split(':').filter(|d| !d.is_empty()).map(|d| Path::new(d).join("pixmaps")).collect()
}

/// The theme GTK uses, from its settings files.
fn theme_name() -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    let config = std::env::var("XDG_CONFIG_HOME").unwrap_or_else(|_| format!("{home}/.config"));
    for file in ["gtk-4.0/settings.ini", "gtk-3.0/settings.ini"] {
        let Ok(text) = std::fs::read_to_string(Path::new(&config).join(file)) else { continue };
        for line in text.lines() {
            if let Some((key, value)) = line.split_once('=')
                && key.trim() == "gtk-icon-theme-name"
            {
                return value.trim().trim_matches('"').to_owned();
            }
        }
    }
    "hicolor".into()
}

/// A theme and the ones it inherits from, hicolor last.
fn themes() -> &'static [Theme] {
    static THEMES: OnceLock<Vec<Theme>> = OnceLock::new();
    THEMES.get_or_init(|| {
        let bases = icon_bases();
        let mut chain = Vec::new();
        let mut queue = vec![theme_name()];
        let mut seen = std::collections::HashSet::new();
        while let Some(name) = queue.pop() {
            if !seen.insert(name.clone()) {
                continue;
            }
            let roots: Vec<PathBuf> = bases.iter().map(|b| b.join(&name)).filter(|p| p.is_dir()).collect();
            let Some(index) = roots.iter().find_map(|r| std::fs::read_to_string(r.join("index.theme")).ok()) else {
                continue;
            };
            let (dirs, inherits) = parse_index(&index);
            chain.push(Theme { roots, dirs });
            // Depth first, in the order given: the queue is a stack.
            queue.extend(inherits.into_iter().rev());
        }
        if !seen.contains("hicolor") {
            let roots: Vec<PathBuf> = bases.iter().map(|b| b.join("hicolor")).filter(|p| p.is_dir()).collect();
            if let Some(index) = roots.iter().find_map(|r| std::fs::read_to_string(r.join("index.theme")).ok()) {
                chain.push(Theme { roots, dirs: parse_index(&index).0 });
            }
        }
        chain
    })
}

fn parse_index(text: &str) -> (Vec<Dir>, Vec<String>) {
    let mut sections: HashMap<String, HashMap<String, String>> = HashMap::new();
    let mut current = String::new();
    for line in text.lines() {
        let line = line.trim();
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            current = name.to_owned();
        } else if let Some((key, value)) = line.split_once('=') {
            sections.entry(current.clone()).or_default().insert(key.trim().to_owned(), value.trim().to_owned());
        }
    }
    let main = sections.get("Icon Theme");
    let list = |key: &str| -> Vec<String> {
        main.and_then(|m| m.get(key))
            .map(|v| v.split(',').map(|s| s.trim().to_owned()).filter(|s| !s.is_empty()).collect())
            .unwrap_or_default()
    };
    let mut names = list("Directories");
    names.extend(list("ScaledDirectories"));
    let dirs = names
        .into_iter()
        .filter_map(|path| {
            let section = sections.get(&path)?;
            let number = |key: &str| section.get(key).and_then(|v| v.parse::<i32>().ok());
            let size = number("Size")?;
            if number("Scale").unwrap_or(1) != 1 {
                return None;
            }
            let (min, max) = match section.get("Type").map(String::as_str) {
                Some("Scalable") => (number("MinSize").unwrap_or(size), number("MaxSize").unwrap_or(size)),
                Some("Fixed") => (size, size),
                _ => {
                    let threshold = number("Threshold").unwrap_or(2);
                    (size - threshold, size + threshold)
                }
            };
            Some(Dir { path, min, max })
        })
        .collect();
    (dirs, list("Inherits"))
}

/// The file for `name` closest to `size`, the spec's lookup in short.
fn find(name: &str, size: i32) -> Option<PathBuf> {
    if name.starts_with('/') {
        return Path::new(name).is_file().then(|| PathBuf::from(name));
    }
    let name = name.strip_suffix(".png").or_else(|| name.strip_suffix(".svg")).unwrap_or(name);
    for theme in themes() {
        let mut best: Option<(i32, PathBuf)> = None;
        for dir in &theme.dirs {
            // How far the directory's sizes are from the one wanted; a
            // larger image shrinks better than a smaller one grows.
            let distance = if size < dir.min {
                dir.min - size
            } else if size > dir.max {
                (size - dir.max) * 4
            } else {
                0
            };
            if best.as_ref().is_some_and(|(d, _)| *d <= distance) {
                continue;
            }
            for root in &theme.roots {
                let found = ["svg", "png"].iter().map(|ext| root.join(&dir.path).join(format!("{name}.{ext}")));
                if let Some(path) = found.into_iter().find(|p| p.is_file()) {
                    best = Some((distance, path));
                    break;
                }
            }
        }
        if let Some((_, path)) = best {
            return Some(path);
        }
    }
    pixmap_dirs()
        .into_iter()
        .flat_map(|dir| ["svg", "png"].map(|ext| dir.join(format!("{name}.{ext}"))))
        .find(|p| p.is_file())
}

/// `name` from `Icon=`, as a `size` x `size` image, or `None` if there is
/// no such icon or it would not decode.
pub fn load(name: &str, size: i32) -> Option<Pixmap> {
    if name.is_empty() || size <= 0 {
        return None;
    }
    render(&find(name, size)?, size)
}

/// Like [`load`], looking first in `dir`, a directory of an app's own icons
/// (what a tray item names as its `IconThemePath`). Remembered by all three,
/// since the tray asks again every time it polls.
pub fn load_from(dir: &str, name: &str, size: i32) -> Option<Pixmap> {
    type Cache = std::sync::Mutex<HashMap<(String, String, i32), Option<Pixmap>>>;
    static CACHE: OnceLock<Cache> = OnceLock::new();
    let mut cache = CACHE.get_or_init(Default::default).lock().ok()?;
    cache
        .entry((dir.to_owned(), name.to_owned(), size))
        .or_insert_with(|| {
            let own = (!dir.is_empty())
                .then(|| find_in(Path::new(dir), name, 0))
                .flatten()
                .and_then(|path| render(&path, size));
            own.or_else(|| load(name, size))
        })
        .clone()
}

/// `name.svg` or `name.png` anywhere under `dir`, a few levels down at most:
/// some apps keep a flat folder, others a small theme of their own.
fn find_in(dir: &Path, name: &str, depth: usize) -> Option<PathBuf> {
    let direct = ["svg", "png"].iter().map(|ext| dir.join(format!("{name}.{ext}"))).find(|p| p.is_file());
    if direct.is_some() || depth >= 4 {
        return direct;
    }
    let mut subdirs: Vec<PathBuf> =
        std::fs::read_dir(dir).ok()?.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
    subdirs.sort();
    subdirs.into_iter().find_map(|sub| find_in(&sub, name, depth + 1))
}

fn render(path: &Path, size: i32) -> Option<Pixmap> {
    let data = std::fs::read(path).ok()?;
    let image = if path.extension().is_some_and(|e| e == "svg") {
        render_svg(&data, size)?
    } else {
        let image = crate::image::decode_png(&data)?;
        // Fit the longer side, centred.
        let side = image.width.max(image.height) as f32;
        let (x, y) = ((image.width as f32 - side) / 2.0, (image.height as f32 - side) / 2.0);
        crate::image::resample(&image, (x, y, side, side), size, size)
    };
    Some(crate::image::crisp(image))
}

pub fn render_svg(data: &[u8], size: i32) -> Option<Pixmap> {
    let tree = usvg::Tree::from_data(data, &usvg::Options::default()).ok()?;
    let mut canvas = tiny_skia::Pixmap::new(size as u32, size as u32)?;
    let view = tree.size();
    let scale = size as f32 / view.width().max(view.height());
    let dx = (size as f32 - view.width() * scale) / 2.0;
    let dy = (size as f32 - view.height() * scale) / 2.0;
    resvg::render(&tree, tiny_skia::Transform::from_row(scale, 0.0, 0.0, scale, dx, dy), &mut canvas.as_mut());
    // tiny-skia keeps premultiplied RGBA bytes; the shell wants ARGB words.
    let pixels =
        canvas.data().as_chunks::<4>().0.iter().map(|[r, g, b, a]| u32::from_be_bytes([*a, *r, *g, *b])).collect();
    Some(Pixmap { width: size, height: size, pixels })
}
