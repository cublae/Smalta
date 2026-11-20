//! niri, through its own IPC crate.
//!
//! `EventStream` replays the whole state before sending deltas, and
//! `EventStreamState` keeps the two consistent, so this only has to turn it
//! into a snapshot after every event.

use std::sync::mpsc;
use std::thread;

use niri_ipc::socket::Socket;
use niri_ipc::state::{EventStreamState, EventStreamStatePart};
use niri_ipc::{Action, LayoutSwitchTarget, Request, Response, WorkspaceReferenceArg};
use smithay_client_toolkit::reexports::calloop::channel::Sender;

use super::{Command, Snapshot, Window, Workspace};

pub fn spawn(snapshots: Sender<Snapshot>) -> mpsc::Sender<Command> {
    thread::spawn(move || {
        let mut attempt = 0;
        loop {
            match stream(&snapshots, &mut attempt) {
                Ok(()) => return,
                Err(error) => log::error!("niri event stream: {error}"),
            }
            thread::sleep(super::backoff(attempt));
            attempt += 1;
        }
    });

    let (commands, received) = mpsc::channel();
    thread::spawn(move || {
        for command in received {
            let action = match command {
                Command::FocusWorkspace(id) => {
                    Action::FocusWorkspace { reference: WorkspaceReferenceArg::Id(id as u64) }
                }
                Command::SwitchLayout => Action::SwitchLayout { layout: LayoutSwitchTarget::Next },
                Command::Screenshot => Action::Screenshot { show_pointer: false, path: None },
                Command::Quit => Action::Quit { skip_confirmation: true },
                Command::Spawn(command) => Action::Spawn { command },
                Command::FocusWindow(id) => match id.parse() {
                    Ok(id) => Action::FocusWindow { id },
                    Err(_) => continue,
                },
                // niri keeps windows out of a capture only by a rule in its own
                // config (`block-out-from "screencast"`), not on request.
                Command::HideApps(apps) => {
                    if !apps.is_empty() {
                        log::info!("niri hides windows from capture only by its own window rules");
                    }
                    continue;
                }
                Command::ToggleHidden => {
                    log::info!("niri hides windows from capture only by its own window rules");
                    continue;
                }
            };
            match Socket::connect().and_then(|mut socket| socket.send(Request::Action(action))) {
                Ok(Ok(_)) => {}
                Ok(Err(message)) => log::error!("niri refused an action: {message}"),
                Err(error) => log::error!("niri action: {error}"),
            }
        }
    });
    commands
}

/// Returns `Ok` only when the receiving side is gone and there is no one left to tell.
fn stream(snapshots: &Sender<Snapshot>, attempt: &mut usize) -> std::io::Result<()> {
    let mut socket = Socket::connect()?;
    match socket.send(Request::EventStream)? {
        Ok(Response::Handled) => {}
        Ok(other) => log::warn!("unexpected reply to EventStream: {other:?}"),
        Err(message) => return Err(std::io::Error::other(message)),
    }
    *attempt = 0;

    let mut read = socket.read_events();
    let mut state = EventStreamState::default();
    let mut last = Snapshot::default();
    loop {
        state.apply(read()?);
        let snapshot = snapshot(&state);
        if snapshot != last {
            if snapshots.send(snapshot.clone()).is_err() {
                return Ok(());
            }
            last = snapshot;
        }
    }
}

fn snapshot(state: &EventStreamState) -> Snapshot {
    let mut workspaces: Vec<_> = state.workspaces.workspaces.values().collect();
    workspaces.sort_by(|a, b| (&a.output, a.idx).cmp(&(&b.output, b.idx)));
    let windows = &state.windows.windows;

    let layouts = state.keyboard_layouts.keyboard_layouts.as_ref();
    Snapshot {
        workspaces: workspaces
            .into_iter()
            .map(|w| Workspace {
                id: w.id as i64,
                label: w.idx.to_string(),
                output: w.output.clone().unwrap_or_default(),
                active: w.is_active,
                focused: w.is_focused,
                urgent: w.is_urgent,
                windows: windows.values().filter(|window| window.workspace_id == Some(w.id)).count(),
            })
            .collect(),
        layout: layouts.and_then(|l| l.names.get(l.current_idx as usize).cloned()).unwrap_or_default(),
        layouts: layouts.map_or(0, |l| l.names.len()),
        title: windows.values().find(|w| w.is_focused).and_then(|w| w.title.clone()).unwrap_or_default(),
        app: windows.values().find(|w| w.is_focused).and_then(|w| w.app_id.clone()).unwrap_or_default(),
        // A paused cast exists without watching anything.
        casting: state.casts.casts.values().any(|cast| cast.is_active),
        apps: {
            let mut apps: Vec<String> = windows.values().filter_map(|w| w.app_id.clone()).collect();
            apps.sort();
            apps.dedup();
            apps
        },
        hidden: 0,
        windows: windows_of(state),
    }
}

/// Open windows, the most recently focused first; those on a workspace
/// that is on screen know where.
fn windows_of(state: &EventStreamState) -> Vec<Window> {
    let workspaces = &state.workspaces.workspaces;
    let mut windows: Vec<_> = state.windows.windows.values().collect();
    windows.sort_by_key(|w| std::cmp::Reverse(w.focus_timestamp.map(|t| (t.secs, t.nanos))));
    windows
        .into_iter()
        .map(|w| {
            let workspace = w.workspace_id.and_then(|id| workspaces.get(&id));
            let shown = workspace.is_some_and(|ws| ws.is_active);
            let layout = &w.layout;
            let rect = layout.tile_pos_in_workspace_view.filter(|_| shown).map(|(x, y)| {
                let (w, h) = layout.tile_size;
                (x.round() as i32, y.round() as i32, w.round() as i32, h.round() as i32)
            });
            Window {
                id: w.id.to_string(),
                title: w.title.clone().unwrap_or_default(),
                app: w.app_id.clone().unwrap_or_default(),
                workspace: workspace
                    .map(|ws| ws.name.clone().unwrap_or_else(|| ws.idx.to_string()))
                    .unwrap_or_default(),
                output: workspace.and_then(|ws| ws.output.clone()).unwrap_or_default(),
                focused: w.is_focused,
                rect,
            }
        })
        .collect()
}
