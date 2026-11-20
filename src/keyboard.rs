//! The keyboard, without libxkbcommon: the compositor's keymap is read by kbvm.
//!
//! Only popups take keyboard focus, and only for a few keys and plain text, so
//! this is the client side of XKB and no more: look up the keysym for a key
//! with the current modifiers and group, and repeat it while it is held.

use std::os::unix::fs::FileExt;
use std::time::Duration;

use kbvm::lookup::LookupTable;
use kbvm::xkb::diagnostic::WriteToLog;
use kbvm::{Components, GroupIndex, Keycode, ModifierMask, syms};
use smithay_client_toolkit::dispatch2::Dispatch2;
use smithay_client_toolkit::reexports::calloop::{
    LoopHandle, RegistrationToken,
    timer::{TimeoutAction, Timer},
};
use smithay_client_toolkit::reexports::client::{
    Connection, QueueHandle, WEnum,
    protocol::{wl_keyboard, wl_seat},
};

use crate::App;
use crate::ui::Key;

/// Marks the `wl_keyboard` as ours for dispatch.
pub struct KeyboardData;

#[derive(Default)]
pub struct Keyboard {
    keyboard: Option<wl_keyboard::WlKeyboard>,
    table: Option<LookupTable>,
    components: Components,
    /// Characters per second and the wait before the first repeat, from the compositor.
    rate: i32,
    delay: i32,
    repeating: Option<(u32, RegistrationToken)>,
}

impl Keyboard {
    pub fn bound(&self) -> bool {
        self.keyboard.is_some()
    }

    pub fn bind(&mut self, seat: &wl_seat::WlSeat, qh: &QueueHandle<App>) {
        self.keyboard = Some(seat.get_keyboard(qh, KeyboardData));
        self.rate = 25;
        self.delay = 600;
    }

    pub fn release(&mut self) {
        if let Some(keyboard) = self.keyboard.take() {
            keyboard.release();
        }
    }

    pub fn stop_repeat(&mut self, handle: &LoopHandle<'static, App>) {
        if let Some((_, token)) = self.repeating.take() {
            handle.remove(token);
        }
    }

    /// A modifier that a switcher keybinding is held with: Alt, Super or Ctrl.
    pub fn held(&self) -> bool {
        let pressed = self.components.mods_pressed;
        [ModifierMask::ALT, ModifierMask::SUPER, ModifierMask::CONTROL].iter().any(|m| pressed.contains(*m))
    }

    fn load_keymap(&mut self, fd: std::os::fd::OwnedFd, size: u32) {
        // Read from the start whatever the descriptor's offset: the compositor
        // may hand out one it has already written through.
        let mut bytes = vec![0; size as usize];
        if let Err(error) = std::fs::File::from(fd).read_exact_at(&mut bytes, 0) {
            log::error!("could not read the keymap: {error}");
            return;
        }
        while bytes.last() == Some(&0) {
            bytes.pop();
        }
        let started = std::time::Instant::now();
        let context = kbvm::xkb::Context::default();
        match context.keymap_from_bytes(WriteToLog, None, &bytes) {
            Ok(keymap) => {
                self.table = Some(keymap.to_builder().build_lookup_table());
                log::debug!("keymap read in {:?}", started.elapsed());
            }
            Err(error) => log::error!("could not read the keymap: {error}"),
        }
    }

    fn translate(&self, code: u32) -> Option<Key> {
        let table = self.table.as_ref()?;
        let mods = self.components.mods;
        let control = mods.contains(ModifierMask::CONTROL);
        let lookup = table.lookup(self.components.group, mods, Keycode::from_evdev(code));
        let props = lookup.into_iter().next()?;
        let keysym = props.keysym();
        let key = match keysym {
            syms::BackSpace if control => Key::BackspaceWord,
            syms::BackSpace => Key::Backspace,
            syms::Delete => Key::Delete,
            syms::Return | syms::KP_Enter => Key::Enter,
            syms::Escape => Key::Escape,
            syms::Tab => Key::Tab,
            syms::ISO_Left_Tab => Key::BackTab,
            syms::Up => Key::Up,
            syms::Down => Key::Down,
            syms::Left => Key::Left,
            syms::Right => Key::Right,
            syms::Home => Key::Home,
            syms::End => Key::End,
            syms::Page_Up => Key::PageUp,
            syms::Page_Down => Key::PageDown,
            _ if control => return None,
            _ => Key::Char(props.char().filter(|c| !c.is_control())?),
        };
        Some(key)
    }
}

impl Dispatch2<wl_keyboard::WlKeyboard, App> for KeyboardData {
    fn event(
        &self,
        app: &mut App,
        _: &wl_keyboard::WlKeyboard,
        event: wl_keyboard::Event,
        _: &Connection,
        _: &QueueHandle<App>,
    ) {
        use wl_keyboard::Event;
        match event {
            Event::Keymap { fd, size, .. } => app.keyboard.load_keymap(fd, size),
            Event::Modifiers { mods_depressed, mods_latched, mods_locked, group, .. } => {
                let components = &mut app.keyboard.components;
                components.mods_pressed = ModifierMask(mods_depressed);
                components.mods_latched = ModifierMask(mods_latched);
                components.mods_locked = ModifierMask(mods_locked);
                components.group_locked = GroupIndex(group);
                components.update_effective();
                let held = app.keyboard.held();
                app.modifiers(held);
            }
            Event::RepeatInfo { rate, delay } => {
                app.keyboard.rate = rate;
                app.keyboard.delay = delay;
            }
            Event::Leave { .. } => {
                let handle = app.handle.clone();
                app.keyboard.stop_repeat(&handle);
            }
            Event::Key { key, state: WEnum::Value(state), .. } => {
                let handle = app.handle.clone();
                if state == wl_keyboard::KeyState::Released {
                    if app.keyboard.repeating.as_ref().is_some_and(|(code, _)| *code == key) {
                        app.keyboard.stop_repeat(&handle);
                    }
                    return;
                }
                let Some(translated) = app.keyboard.translate(key) else { return };
                app.keyboard.stop_repeat(&handle);
                let repeats = !matches!(translated, Key::Escape | Key::Enter | Key::Tab | Key::BackTab);
                app.key(translated.clone());
                if repeats && app.keyboard.rate > 0 {
                    let interval = Duration::from_millis(1000 / app.keyboard.rate.max(1) as u64);
                    let timer = Timer::from_duration(Duration::from_millis(app.keyboard.delay.max(0) as u64));
                    let token = handle.insert_source(timer, move |_, _, app: &mut App| {
                        app.key(translated.clone());
                        TimeoutAction::ToDuration(interval)
                    });
                    if let Ok(token) = token {
                        app.keyboard.repeating = Some((key, token));
                    }
                }
            }
            _ => {}
        }
    }
}
