//! `smalta toggle launcher` and friends, over a unix socket.
//!
//! The running shell listens on `$XDG_RUNTIME_DIR/smalta-$WAYLAND_DISPLAY.sock`;
//! running `smalta` with arguments connects, writes one line, prints the reply
//! and exits. Keybindings pay for a process spawn and nothing more.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;

pub fn path() -> PathBuf {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    let display = std::env::var("WAYLAND_DISPLAY").unwrap_or_else(|_| "wayland-0".into());
    runtime.join(format!("smalta-{}.sock", display.replace('/', "_")))
}

/// Bind the socket, taking it over from a shell that died without cleaning up.
/// `None` when another shell is alive and answering on it.
pub fn listen() -> Option<UnixListener> {
    let path = path();
    if UnixStream::connect(&path).is_ok() {
        return None;
    }
    let _ = std::fs::remove_file(&path);
    match UnixListener::bind(&path) {
        Ok(listener) => {
            let _ = listener.set_nonblocking(true);
            Some(listener)
        }
        Err(error) => {
            log::error!("{}: {error}; `smalta` commands will not reach this shell", path.display());
            None
        }
    }
}

/// Read one request line from a fresh connection.
pub fn read_request(stream: &UnixStream) -> Option<String> {
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(std::time::Duration::from_millis(500)));
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line).ok()?;
    Some(line.trim().to_owned())
}

pub fn reply(mut stream: &UnixStream, text: &str) {
    let _ = writeln!(stream, "{text}");
}

/// The client side: send `args` to the running shell, print what it says.
pub fn send(args: &[String]) -> i32 {
    let Ok(mut stream) = UnixStream::connect(path()) else {
        eprintln!("smalta: not running");
        return 1;
    };
    if writeln!(stream, "{}", args.join(" ")).is_err() {
        eprintln!("smalta: could not talk to the shell");
        return 1;
    }
    let mut reply = String::new();
    let _ = BufReader::new(&stream).read_line(&mut reply);
    let reply = reply.trim();
    if let Some(error) = reply.strip_prefix("error: ") {
        eprintln!("smalta: {error}");
        return 2;
    }
    if !reply.is_empty() {
        println!("{reply}");
    }
    0
}

pub const USAGE: &str = "\
smalta — a pixel desktop shell for niri and Hyprland

Running `smalta` with no arguments starts the shell. With arguments, it
forwards a command to the one already running:

  smalta toggle launcher
  smalta toggle control-center
  smalta toggle calendar
  smalta toggle clipboard
  smalta toggle notification-center
  smalta toggle weather
  smalta toggle settings
  smalta toggle session   lock, log out, suspend, restart, power off, full screen
  smalta screenshot       freeze the screens, select, mark up, copy or save
  smalta toggle windows   the window switcher, chosen with Enter or a click
  smalta switch           the window switcher; again for the next window,
                          letting go of Alt for the chosen one
  smalta switch back      the same, backwards
  smalta close            close whatever is open
  smalta reload           read the config again and apply it
  smalta toggle-hidden    the focused window out of screen sharing, or back in
  smalta quit             stop the shell";
