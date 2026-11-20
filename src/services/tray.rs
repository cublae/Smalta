//! The system tray: StatusNotifierItem over the session bus.
//!
//! Items register with a watcher. If nothing else runs one, the shell serves
//! `org.kde.StatusNotifierWatcher` itself; either way it then registers as a
//! host and reads the items' properties. Polling every two seconds keeps
//! this simple and is cheap: there are rarely more than a handful of items.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::Duration;

use smithay_client_toolkit::reexports::calloop::channel::Sender;
use zbus::blocking::Connection;
use zbus::message::Header;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Structure, Value};

use super::{Update, proxy};
use crate::appicons;

const WATCHER: &str = "org.kde.StatusNotifierWatcher";
const WATCHER_PATH: &str = "/StatusNotifierWatcher";
const ITEM: &str = "org.kde.StatusNotifierItem";
const MENU: &str = "com.canonical.dbusmenu";

/// An icon as premultiplied ARGB, row-major.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Pixmap {
    pub width: i32,
    pub height: i32,
    pub pixels: Vec<u32>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Item {
    /// `bus/path`, unique per item.
    pub key: String,
    pub title: String,
    pub icon: Option<Pixmap>,
    pub needs_attention: bool,
    pub has_menu: bool,
    /// The item only has a menu; a click shows it rather than activating.
    pub is_menu: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct MenuEntry {
    pub id: i32,
    pub label: String,
    pub enabled: bool,
    pub separator: bool,
    /// `Some(on)` for check and radio items.
    pub checked: Option<bool>,
    pub depth: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    Activate(String),
    Secondary(String),
    Scroll(String, i32),
    /// Read the item's menu; the answer comes back as [`super::Update::TrayMenu`].
    Menu(String),
    Click(String, i32),
}

#[derive(Default)]
struct Registry {
    items: Vec<String>,
}

struct Watcher {
    registry: Arc<Mutex<Registry>>,
}

#[zbus::interface(name = "org.kde.StatusNotifierWatcher")]
impl Watcher {
    fn register_status_notifier_item(&self, service: String, #[zbus(header)] header: Header<'_>) {
        // Some items pass their object path and leave the bus name implied.
        let sender = header.sender().map(|s| s.to_string()).unwrap_or_default();
        let key = if service.starts_with('/') {
            format!("{sender}{service}")
        } else {
            format!("{service}/StatusNotifierItem")
        };
        let mut registry = self.registry.lock().unwrap();
        if !registry.items.contains(&key) {
            registry.items.push(key);
        }
    }

    fn register_status_notifier_host(&self, _service: String) {}

    #[zbus(property)]
    fn registered_status_notifier_items(&self) -> Vec<String> {
        self.registry.lock().unwrap().items.clone()
    }

    #[zbus(property)]
    fn is_status_notifier_host_registered(&self) -> bool {
        true
    }

    #[zbus(property)]
    fn protocol_version(&self) -> i32 {
        0
    }
}

fn split(key: &str) -> (String, String) {
    match key.find('/') {
        Some(at) => (key[..at].to_owned(), key[at..].to_owned()),
        None => (key.to_owned(), "/StatusNotifierItem".to_owned()),
    }
}

/// The largest icon not over 32 pixels, or the smallest there is, in ARGB.
fn pixmap(value: &OwnedValue) -> Option<Pixmap> {
    let list: Vec<(i32, i32, Vec<u8>)> = value.try_clone().ok()?.try_into().ok()?;
    let best = list
        .iter()
        .filter(|(w, h, data)| *w > 0 && *h > 0 && data.len() >= (*w * *h * 4) as usize)
        .min_by_key(|(w, _, _)| if *w >= 16 { (*w - 16).abs() } else { 1000 - *w })?;
    let (width, height, data) = best;
    // Network byte order: A, R, G, B. Premultiplied on the way through.
    let pixels = data
        .as_chunks::<4>()
        .0
        .iter()
        .map(|p| {
            let a = p[0] as u32;
            let pre = |c: u8| c as u32 * a / 255;
            (a << 24) | (pre(p[1]) << 16) | (pre(p[2]) << 8) | pre(p[3])
        })
        .collect();
    Some(Pixmap { width: *width, height: *height, pixels })
}

fn read_item(connection: &Connection, key: &str) -> Option<Item> {
    let (bus, path) = split(key);
    let item = proxy(connection, bus, path, ITEM).ok()?;
    let props =
        proxy(connection, item.destination().to_string(), item.path().to_string(), "org.freedesktop.DBus.Properties")
            .ok()?;
    let all: HashMap<String, OwnedValue> = props.call("GetAll", &(ITEM,)).ok()?;
    let text = |name: &str| all.get(name).and_then(|v| <&str>::try_from(v).ok()).unwrap_or("").to_owned();
    let status = text("Status");
    let attention = status == "NeedsAttention";
    // The picture the item sends, or else the icon it names, looked up in
    // its own directory and then the theme. Spotify, for one, only names.
    let named = |name: &str| appicons::load_from(&text("IconThemePath"), &text(name), crate::bar::ICON);
    let icon = if attention {
        all.get("AttentionIconPixmap").and_then(pixmap).or_else(|| named("AttentionIconName"))
    } else {
        None
    }
    .or_else(|| all.get("IconPixmap").and_then(pixmap))
    .or_else(|| named("IconName"));
    let title = [text("Title"), text("Id"), text("IconName")].into_iter().find(|t| !t.is_empty()).unwrap_or_default();
    let menu = all.get("Menu").and_then(|v| OwnedObjectPath::try_from(v.try_clone().ok()?).ok());
    Some(Item {
        key: key.to_owned(),
        title,
        icon,
        needs_attention: attention,
        has_menu: menu.is_some_and(|m| m.as_str() != "/"),
        is_menu: all.get("ItemIsMenu").and_then(|v| bool::try_from(v).ok()).unwrap_or(false),
    })
}

/// Item keys from whichever watcher runs, after dropping those whose owner left.
fn item_keys(connection: &Connection, registry: &Arc<Mutex<Registry>>, ours: bool) -> Vec<String> {
    let names: Vec<String> = proxy(connection, "org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus")
        .and_then(|p| p.call("ListNames", &()))
        .unwrap_or_default();
    if ours {
        let mut registry = registry.lock().unwrap();
        registry.items.retain(|key| names.contains(&split(key).0));
        return registry.items.clone();
    }
    proxy(connection, WATCHER, WATCHER_PATH, WATCHER)
        .and_then(|p| p.get_property::<Vec<String>>("RegisteredStatusNotifierItems"))
        .unwrap_or_default()
        .into_iter()
        .map(|entry| if entry.contains('/') { entry } else { format!("{entry}/StatusNotifierItem") })
        .collect()
}

fn menu_proxy<'a>(connection: &Connection, key: &str) -> Option<zbus::blocking::Proxy<'a>> {
    let (bus, path) = split(key);
    let item = proxy(connection, bus.clone(), path, ITEM).ok()?;
    let menu: OwnedObjectPath = item.get_property("Menu").ok()?;
    proxy(connection, bus, menu.to_string(), MENU).ok()
}

type Layout = (i32, HashMap<String, OwnedValue>, Vec<OwnedValue>);

fn flatten(node: &Layout, depth: usize, out: &mut Vec<MenuEntry>) {
    let (_, _, children) = node;
    for child in children {
        let Ok(structure) =
            Structure::try_from(Value::from(child.try_clone().unwrap_or_else(|_| OwnedValue::from(0u8))))
        else {
            continue;
        };
        let fields = structure.into_fields();
        let [id, props, kids] = &fields[..] else { continue };
        let id = i32::try_from(id).unwrap_or(0);
        let props: HashMap<String, OwnedValue> =
            props.try_clone().ok().and_then(|p| p.try_into().ok()).unwrap_or_default();
        let get = |name: &str| props.get(name);
        if get("visible").and_then(|v| bool::try_from(v).ok()) == Some(false) {
            continue;
        }
        let separator = get("type").and_then(|v| <&str>::try_from(v).ok()) == Some("separator");
        let label = get("label")
            .and_then(|v| <&str>::try_from(v).ok())
            .unwrap_or("")
            .replace("__", "\u{0}")
            .replace('_', "")
            .replace('\u{0}', "_");
        let toggle = get("toggle-type").and_then(|v| <&str>::try_from(v).ok()).unwrap_or("");
        let state = get("toggle-state").and_then(|v| i32::try_from(v).ok()).unwrap_or(0);
        out.push(MenuEntry {
            id,
            label,
            enabled: get("enabled").and_then(|v| bool::try_from(v).ok()).unwrap_or(true),
            separator,
            checked: (!toggle.is_empty()).then_some(state == 1),
            depth,
        });
        let children: Vec<OwnedValue> = kids.try_clone().ok().and_then(|k| k.try_into().ok()).unwrap_or_default();
        if !children.is_empty() {
            flatten(&(id, HashMap::new(), children), depth + 1, out);
        }
    }
}

fn read_menu(connection: &Connection, key: &str) -> Vec<MenuEntry> {
    let Some(menu) = menu_proxy(connection, key) else { return Vec::new() };
    let _ = menu.call_method("AboutToShow", &(0i32,));
    let reply: zbus::Result<(u32, Layout)> = menu.call("GetLayout", &(0i32, -1i32, Vec::<String>::new()));
    let mut out = Vec::new();
    if let Ok((_, root)) = reply {
        flatten(&root, 0, &mut out);
    }
    out
}

pub fn spawn(updates: Sender<Update>) -> mpsc::Sender<Command> {
    let (commands, received) = mpsc::channel::<Command>();
    thread::spawn(move || {
        let connection = match Connection::session() {
            Ok(connection) => connection,
            Err(error) => {
                log::warn!("session bus: {error}; no tray");
                return;
            }
        };
        let registry = Arc::new(Mutex::new(Registry::default()));
        // Serve the watcher unless someone already does.
        let ours = connection.object_server().at(WATCHER_PATH, Watcher { registry: registry.clone() }).is_ok()
            && connection.request_name(WATCHER).is_ok();
        if !ours {
            let _ = connection.object_server().remove::<Watcher, _>(WATCHER_PATH);
        }
        let host = format!("org.kde.StatusNotifierHost-{}", std::process::id());
        let _ = connection.request_name(host.as_str());
        if let Ok(watcher) = proxy(&connection, WATCHER, WATCHER_PATH, WATCHER) {
            let _ = watcher.call_method("RegisterStatusNotifierHost", &(host.as_str(),));
        }
        log::info!("tray: {}", if ours { "serving the watcher" } else { "using a watcher already running" });

        let mut last = None;
        loop {
            let items: Vec<Item> =
                item_keys(&connection, &registry, ours).iter().filter_map(|key| read_item(&connection, key)).collect();
            if last.as_ref() != Some(&items) {
                if updates.send(Update::Tray(items.clone())).is_err() {
                    return;
                }
                last = Some(items);
            }
            let Ok(command) = received.recv_timeout(Duration::from_secs(2)) else { continue };
            let item = |key: &str| {
                let (bus, path) = split(key);
                proxy(&connection, bus, path, ITEM)
            };
            let result = match command {
                Command::Activate(key) => item(&key).and_then(|p| p.call_method("Activate", &(0i32, 0i32)).map(drop)),
                Command::Secondary(key) => {
                    item(&key).and_then(|p| p.call_method("SecondaryActivate", &(0i32, 0i32)).map(drop))
                }
                Command::Scroll(key, delta) => {
                    item(&key).and_then(|p| p.call_method("Scroll", &(delta, "vertical")).map(drop))
                }
                Command::Menu(key) => {
                    let entries = read_menu(&connection, &key);
                    let _ = updates.send(Update::TrayMenu(key, entries));
                    Ok(())
                }
                Command::Click(key, id) => match menu_proxy(&connection, &key) {
                    Some(menu) => {
                        let stamp = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map_or(0, |d| d.as_secs() as u32);
                        menu.call_method("Event", &(id, "clicked", Value::from(0i32), stamp)).map(drop)
                    }
                    None => Ok(()),
                },
            };
            if let Err(error) = result {
                log::warn!("tray: {error}");
            }
        }
    });
    commands
}
