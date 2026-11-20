//! The launcher: applications by fuzzy name, a calculator, a converter of
//! units and currencies (`10 km in mi`), emoji after `:`, a shell command
//! after `>`, files in the home directory after `/`, the shell's own settings
//! after `?`, open windows after `#` and the clipboard history after `@`.

use std::collections::HashMap;

use crate::appicons;
use crate::apps::{self, App, Launches};
use crate::calc;
use crate::canvas::Rect;
use crate::clipboard;
use crate::compositor::Window;
use crate::convert;
use crate::files;
use crate::i18n::tr;
use crate::services::tray::Pixmap;
use crate::ui::{Align, Field, Key, Ui};

use super::{Ctx, Outcome, Request};

const EMOJI: &str = include_str!("../../assets/emoji.tsv");
const ROWS: usize = 12;
/// Icons, in shell pixels: the height of a line of text.
const ICON: i32 = 16;

#[derive(Clone)]
enum Item {
    App(usize),
    Answer(String),
    Emoji(&'static str, &'static str),
    Run(String),
    File(usize),
    /// One of `settings::topics`, by its place in `Launcher::topics`.
    Setting(usize),
    /// An open window, as it was when the query was typed.
    Window(Window),
    /// An entry of the clipboard history, by its place in `Launcher::clips`.
    Clip(usize),
    /// A conversion, answered as it is drawn: rates may arrive meanwhile.
    Convert(convert::Query),
}

#[derive(Default)]
pub struct Launcher {
    field: Field,
    apps: Vec<App>,
    launches: Option<Launches>,
    results: Vec<Item>,
    selected: usize,
    scroll: usize,
    /// By `Icon=` value, kept across openings; `None` is an icon not found.
    icons: HashMap<String, Option<Pixmap>>,
    /// The home directory, walked on the first `/`.
    files: files::Index,
    topics: Vec<(String, String, usize)>,
    /// The open windows, from the compositor's last word.
    windows: Vec<Window>,
    /// The clipboard history, read on the first `@` of an opening.
    clips: Option<Vec<clipboard::Entry>>,
}

impl Launcher {
    pub fn opened(&mut self) {
        // Read afresh every time: an install since the last opening shows up,
        // and the cost is a few milliseconds of small files.
        self.apps = apps::load();
        self.launches = Some(Launches::load());
        self.topics = super::settings::topics();
        self.clips = None;
        self.field = Field::default();
        self.search();
    }

    fn search(&mut self) {
        self.selected = 0;
        self.scroll = 0;
        let query = self.field.text.trim().to_owned();
        self.results.clear();

        if let Some(command) = query.strip_prefix('>') {
            if !command.trim().is_empty() {
                self.results.push(Item::Run(command.trim().to_owned()));
            }
            return;
        }
        if let Some(name) = query.strip_prefix('/') {
            // By name, or by path once the query has a slash in it.
            let name = name.trim();
            let by_path = name.contains('/');
            let mut found: Vec<(i32, usize)> = self
                .files
                .files()
                .iter()
                .enumerate()
                .filter_map(|(i, f)| {
                    let text = if by_path { &f.relative } else { &f.name };
                    // Nearer the top of the home directory ranks higher.
                    score(name, text).map(|s| (s * 4 - f.relative.matches('/').count() as i32, i))
                })
                .collect();
            found.sort_by_key(|f| std::cmp::Reverse(f.0));
            self.results.extend(found.into_iter().take(200).map(|(_, i)| Item::File(i)));
            return;
        }
        if let Some(name) = query.strip_prefix('?') {
            let name = name.trim();
            let mut found: Vec<(i32, usize)> = self
                .topics
                .iter()
                .enumerate()
                .filter_map(|(i, (topic, page, _))| {
                    let on_page = score(name, page).map(|s| s / 2);
                    score(name, topic).max(on_page).map(|s| (s, i))
                })
                .collect();
            found.sort_by_key(|f| std::cmp::Reverse(f.0));
            self.results.extend(found.into_iter().map(|(_, i)| Item::Setting(i)));
            return;
        }
        if let Some(name) = query.strip_prefix('#') {
            let name = name.trim();
            let mut found: Vec<(i32, &Window)> = self
                .windows
                .iter()
                .filter_map(|w| score(name, &w.title).max(score(name, &w.app)).map(|s| (s, w)))
                .collect();
            // Most recently used first among equals: the list comes in that order.
            found.sort_by_key(|f| std::cmp::Reverse(f.0));
            self.results.extend(found.into_iter().map(|(_, w)| Item::Window(w.clone())));
            return;
        }
        if let Some(name) = query.strip_prefix('@') {
            let name = name.trim();
            let clips = self.clips.get_or_insert_with(clipboard::list);
            // Newest first, as cliphist keeps them; a search keeps that order.
            let found = clips.iter().enumerate().filter(|(_, c)| score(name, &c.preview).is_some());
            self.results.extend(found.take(200).map(|(i, _)| Item::Clip(i)));
            return;
        }
        if let Some(name) = query.strip_prefix(':') {
            let name = name.to_lowercase();
            let mut found: Vec<(i32, &'static str, &'static str)> = EMOJI
                .lines()
                .filter_map(|line| {
                    let mut parts = line.split('\t');
                    let (glyph, label) = (parts.next()?, parts.next()?);
                    score(&name, label).map(|s| (s, glyph, label))
                })
                .collect();
            found.sort_by_key(|f| std::cmp::Reverse(f.0));
            self.results.extend(found.into_iter().take(200).map(|(_, glyph, label)| Item::Emoji(glyph, label)));
            return;
        }
        if let Some(conversion) = convert::parse(&query).filter(|q| convert::answer(q, None).is_some()) {
            self.results.push(Item::Convert(conversion));
        } else if calc::looks_like_math(&query)
            && let Some(value) = calc::eval(&query)
        {
            self.results.push(Item::Answer(calc::format(value)));
        }

        let counts = self.launches.as_ref().map(|l| &l.counts);
        let mut scored: Vec<(f32, usize)> = self
            .apps
            .iter()
            .enumerate()
            .filter_map(|(index, app)| {
                let base = if query.is_empty() {
                    0
                } else {
                    let name = score(&query, &app.name).map(|s| s + 20);
                    let rest =
                        [&app.generic, &app.keywords, &app.id].iter().filter_map(|text| score(&query, text)).max();
                    name.into_iter().chain(rest).max()?
                };
                let habit = counts.and_then(|c| c.get(&app.id)).map_or(0.0, |n| (*n as f32 + 1.0).ln() * 8.0);
                Some((base as f32 + habit, index))
            })
            .collect();
        // Stable: equal scores stay alphabetical.
        scored.sort_by(|a, b| b.0.total_cmp(&a.0));
        self.results.extend(scored.into_iter().map(|(_, index)| Item::App(index)));
    }

    fn activate(&mut self, item: Item, cx: &mut Ctx) {
        match item {
            Item::App(index) => {
                let app = &self.apps[index];
                cx.requests.push(Request::Spawn(apps::argv(app)));
                if let Some(launches) = &mut self.launches {
                    launches.bump(&app.id);
                }
            }
            Item::Answer(text) => cx.requests.push(Request::Copy(text)),
            Item::Convert(query) => match convert::answer(&query, cx.services.rates.as_ref()) {
                Some(convert::Answer::Value(text)) => cx.requests.push(Request::Copy(text)),
                // Nothing to copy yet: stay open until the rates come.
                _ => return,
            },
            Item::Window(window) => cx.requests.push(Request::FocusWindow(window.id)),
            Item::Clip(i) => {
                if let Some(entry) = self.clips.as_ref().and_then(|c| c.get(i)) {
                    clipboard::copy_entry(entry);
                }
            }
            Item::Emoji(glyph, _) => cx.requests.push(Request::Copy(glyph.to_owned())),
            Item::Run(command) => cx.requests.push(Request::Spawn(vec!["sh".into(), "-c".into(), command])),
            Item::File(i) => {
                let path = self.files.files[i].path.display().to_string();
                cx.requests.push(Request::Spawn(vec!["xdg-open".into(), path]));
            }
            // Opening the settings closes the launcher on its own.
            Item::Setting(i) => {
                cx.requests.push(Request::Settings(self.topics[i].2));
                return;
            }
        }
        cx.requests.push(Request::Close);
    }

    pub fn key(&mut self, key: &Key, cx: &mut Ctx) -> Outcome {
        let count = self.results.len();
        match key {
            Key::Escape => return Outcome::Close,
            Key::Enter => {
                if let Some(item) = self.results.get(self.selected).cloned() {
                    self.activate(item, cx);
                }
            }
            Key::Down | Key::Tab => self.selected = (self.selected + 1).min(count.saturating_sub(1)),
            Key::Up | Key::BackTab => self.selected = self.selected.saturating_sub(1),
            Key::PageDown => self.selected = (self.selected + ROWS).min(count.saturating_sub(1)),
            Key::PageUp => self.selected = self.selected.saturating_sub(ROWS),
            _ => {
                if self.field.key(key) {
                    self.windows = cx.snapshot.windows.clone();
                    self.search();
                }
            }
        }
        // Keep the selection on screen.
        if self.selected < self.scroll {
            self.scroll = self.selected;
        } else if self.selected >= self.scroll + ROWS {
            self.scroll = self.selected + 1 - ROWS;
        }
        Outcome::Redraw
    }

    pub fn draw(&mut self, ui: &mut Ui, area: Rect, cx: &mut Ctx) -> i32 {
        let t = ui.theme;
        let line = ui.line();
        let (field, rest) = area.take_top(line + 10, 8);
        ui.field(field, &self.field, tr("Search…"), true, false);

        let row_h = line + 12;
        let list = Rect::new(rest.x, rest.y, rest.w, row_h * ROWS as i32);
        let steps = ui.scrolled(list);
        if steps != 0 {
            let max = self.results.len().saturating_sub(ROWS);
            self.scroll = (self.scroll as i32 + steps).clamp(0, max as i32) as usize;
        }
        if self.results.is_empty() {
            ui.text(Rect::new(list.x, list.y, list.w, row_h), tr("No results"), t.dim, Align::Center);
        }

        let mut chosen = None;
        for (slot, index) in (self.scroll..self.results.len().min(self.scroll + ROWS)).enumerate() {
            let rect = Rect::new(list.x, list.y + slot as i32 * row_h, list.w, row_h - 2);
            let current = index == self.selected;
            if current {
                ui.fill(rect, t.hover);
                ui.fill(Rect::new(rect.x, rect.y, 2, rect.h), t.accent);
            } else if ui.hovered(rect) {
                ui.fill(rect, t.hover);
            }
            let (badge, body) = Rect::new(rect.x + 8, rect.y, rect.w - 16, rect.h).take_left(ICON, 10);
            let (title, detail, mark, icon) = match &self.results[index] {
                Item::App(i) => {
                    let app = &self.apps[*i];
                    let detail = if !app.comment.is_empty() { &app.comment } else { &app.generic };
                    // Loaded as rows come into view, not all at opening:
                    // an SVG takes a millisecond or two to render.
                    // Without `Icon=`, a theme may still have one under the entry id.
                    let name = if app.icon.is_empty() { &app.id } else { &app.icon };
                    let icon = self.icons.entry(name.clone()).or_insert_with(|| appicons::load(name, ICON)).as_ref();
                    (app.name.clone(), detail.clone(), monogram(&app.name), icon)
                }
                // The "=" in the icon column says it is an answer.
                Item::Answer(text) => (text.clone(), String::new(), "=".into(), None),
                Item::Convert(query) => {
                    if convert::wants_rates(query) && cx.services.rates.is_none() {
                        let _ = cx.handles.rates.send(());
                    }
                    let title = match convert::answer(query, cx.services.rates.as_ref()) {
                        Some(convert::Answer::Value(text)) => text,
                        Some(convert::Answer::Waiting) => tr("Fetching rates…").to_owned(),
                        _ => tr("Unknown currency").to_owned(),
                    };
                    (title, String::new(), "=".into(), None)
                }
                Item::Window(window) => {
                    let detail = format!("{} {}", tr("workspace"), window.workspace);
                    let title = if window.title.is_empty() { window.app.clone() } else { window.title.clone() };
                    let icon = self
                        .icons
                        .entry(window.app.clone())
                        .or_insert_with(|| {
                            appicons::load(&window.app, ICON)
                                .or_else(|| appicons::load(&window.app.to_lowercase(), ICON))
                        })
                        .as_ref();
                    (title, detail, monogram(&window.app), icon)
                }
                Item::Clip(i) => {
                    let entry = &self.clips.as_ref().expect("read with the query")[*i];
                    let when =
                        entry.time.map(|t| crate::i18n::ago(chrono::Local::now().timestamp() - t)).unwrap_or_default();
                    let (title, mark) = match entry.dimensions() {
                        Some(size) => (format!("{} {size}", tr("Image")), "▣"),
                        None => (entry.preview.clone(), "¶"),
                    };
                    (title, when, mark.into(), None)
                }
                Item::Emoji(glyph, name) => (name.to_string(), String::new(), glyph.to_string(), None),
                Item::Run(command) => (command.clone(), String::new(), ">".into(), None),
                Item::File(i) => {
                    let file = &self.files.files[*i];
                    // Where it is, from the home directory.
                    let parent = file.relative.rsplit_once('/').map_or("~".to_owned(), |(dir, _)| format!("~/{dir}"));
                    (file.name.clone(), parent, if file.dir { "/".into() } else { "·".into() }, None)
                }
                Item::Setting(i) => {
                    let (topic, page, _) = &self.topics[*i];
                    (topic.to_lowercase(), page.to_lowercase(), "?".into(), None)
                }
            };
            match icon {
                Some(icon) => ui.canvas.image(badge.x, badge.y + (badge.h - ICON) / 2, ICON, icon),
                // No icon: the first letter, where it would be.
                None => ui.text(badge, &mark, if current { t.accent } else { t.fg }, Align::Center),
            }
            let width = ui.text_width(&title);
            let (name, rest) = body.take_left(width, 10);
            ui.text(name, &title, t.fg, Align::Left);
            ui.text(rest, &detail, t.dim, Align::Left);
            if ui.clicked(rect) {
                chosen = Some(index);
            }
        }
        if let Some(index) = chosen {
            self.selected = index;
            let item = self.results[index].clone();
            self.activate(item, cx);
        }
        // What the prefixes do, for whoever has not read this file.
        let first = Rect::new(list.x, list.bottom() + 6, list.w, line);
        ui.text(first, tr("= math · 10 km in mi · > command · / files"), t.dim, Align::Left);
        let second = Rect::new(list.x, first.bottom(), list.w, line);
        ui.text(second, tr("# windows · @ clipboard · : emoji · ? settings"), t.dim, Align::Left);
        second.bottom() - area.y
    }
}

fn monogram(name: &str) -> String {
    name.chars().find(|c| c.is_alphanumeric()).map(|c| c.to_uppercase().collect()).unwrap_or_else(|| "?".into())
}

/// How well `query` matches `text`, or `None`: a prefix beats the start of a
/// word, which beats anywhere inside, which beats letters in order.
pub fn score(query: &str, text: &str) -> Option<i32> {
    if query.is_empty() {
        return Some(0);
    }
    let query = query.to_lowercase();
    let text = text.to_lowercase();
    if text.starts_with(&query) {
        return Some(100 - text.chars().count().min(40) as i32 / 4);
    }
    if text.split(|c: char| !c.is_alphanumeric()).any(|word| word.starts_with(&query)) {
        return Some(80);
    }
    if text.contains(&query) {
        return Some(60);
    }
    // Letters in order, scored by how close together they sit.
    let mut wanted = query.chars().filter(|c| !c.is_whitespace()).peekable();
    let mut gaps = 0;
    let mut started = false;
    for c in text.chars() {
        match wanted.peek() {
            Some(w) if *w == c => {
                wanted.next();
                started = true;
            }
            Some(_) if started => gaps += 1,
            _ => {}
        }
    }
    wanted.peek().is_none().then(|| (40 - gaps).max(1))
}
