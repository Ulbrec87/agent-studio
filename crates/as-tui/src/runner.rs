//! The event loop: terminal setup/teardown, crossterm events, daemon events, render tick.

use std::io::{self, Write};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use as_core::paths::Paths;
use as_proto::lifecycle::ensure_daemon;
use as_proto::{DaemonClient, Event};
use futures::StreamExt;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{Event as TermEvent, EventStream};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{BeginSynchronizedUpdate, EndSynchronizedUpdate, SetTitle};
use tokio::sync::mpsc;

use crate::app::{App, Effect, Msg, ToastKind};
use crate::bindings_file::Watcher;
use crate::client::Io;
use crate::state::TuiState;
use crate::suspend::{
    ExternalCommand, enter_terminal, restore_terminal, run_blocking, run_on_terminal,
};
use crate::theme::ColorMode;
use crate::ui;

const FRAME_INTERVAL: Duration = Duration::from_millis(33);
/// How often the keybindings file and the UI state are checked.
const HOUSEKEEPING_INTERVAL: Duration = Duration::from_secs(1);
const RECONNECT_MAX_DELAY: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy, Default)]
pub struct TuiOptions {
    /// Capture the mouse (wheel scrolls the scrollback, clicks pick agents). Off by default so
    /// the terminal's own text selection keeps working.
    pub mouse: bool,
}

impl TuiOptions {
    pub fn from_env() -> Self {
        Self {
            mouse: std::env::var("AGENT_STUDIO_MOUSE").is_ok_and(|v| v != "0" && !v.is_empty()),
        }
    }
}

/// Puts the terminal into TUI mode and restores it on drop.
struct TerminalGuard {
    mouse: bool,
}

impl TerminalGuard {
    fn enter(mouse: bool) -> Result<Self> {
        enter_terminal(mouse)?;
        Ok(Self { mouse })
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        restore_terminal(self.mouse);
    }
}

fn install_panic_hook(mouse: bool) {
    let main_thread = std::thread::current().id();
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if std::thread::current().id() == main_thread {
            restore_terminal(mouse);
        }
        tracing::error!(%info, "panic");
        previous(info);
    }));
}

/// Run the TUI with options from the environment.
pub fn run(paths: Paths) -> Result<()> {
    run_with(paths, TuiOptions::from_env())
}

pub fn run_with(paths: Paths, options: TuiOptions) -> Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(2)
        .thread_name("as-tui")
        .build()
        .context("building the tokio runtime")?;
    runtime.block_on(run_async(paths, options))
}

enum ConnEvent {
    Failed,
    Up(Arc<DaemonClient>, mpsc::UnboundedReceiver<Event>),
}

fn spawn_reconnect(paths: Paths, tx: mpsc::UnboundedSender<ConnEvent>) {
    tokio::spawn(async move {
        let mut delay = Duration::from_millis(500);
        loop {
            tokio::time::sleep(delay).await;
            match ensure_daemon(&paths).await {
                Ok(client) => match client.take_events() {
                    Some(events) => {
                        let _ = tx.send(ConnEvent::Up(Arc::new(client), events));
                        return;
                    }
                    None => tracing::error!("new client without an event stream"),
                },
                Err(err) => tracing::warn!(%err, "reconnect failed"),
            }
            if tx.send(ConnEvent::Failed).is_err() {
                return;
            }
            delay = (delay * 2).min(RECONNECT_MAX_DELAY);
        }
    });
}

/// Resolves on SIGTERM/SIGHUP so the terminal is restored instead of left raw.
#[cfg(unix)]
async fn termination() {
    use tokio::signal::unix::{SignalKind, signal};
    let (Ok(mut term), Ok(mut hup)) = (
        signal(SignalKind::terminate()),
        signal(SignalKind::hangup()),
    ) else {
        return std::future::pending().await;
    };
    tokio::select! {
        _ = term.recv() => {},
        _ = hup.recv() => {},
    }
}

#[cfg(not(unix))]
async fn termination() {
    std::future::pending::<()>().await;
}

async fn run_async(paths: Paths, options: TuiOptions) -> Result<()> {
    // Talk to the daemon before touching the terminal, so a failure prints normally.
    let client = Arc::new(ensure_daemon(&paths).await.context("starting the daemon")?);
    let mut events = client.take_events();
    let config = client.get_config().await.context("reading the config")?;
    // `--mouse`, AGENT_STUDIO_MOUSE=1 or `"mouse": true` in config.json (read once, at start).
    let options = TuiOptions {
        mouse: options.mouse || config.mouse,
    };
    let agents = client.list_agents().await.context("listing the agents")?;
    let cwd = std::env::current_dir().unwrap_or_default();
    let mut app = App::new(config, agents, ColorMode::detect(), cwd);
    let state_path = paths.tui_state_file();
    let mut saved_state = TuiState::load(&state_path);
    app.restore_state(saved_state.clone());
    app.set_keybindings_path(paths.keybindings_file());
    let mut bindings = Watcher::new(paths.keybindings_file());
    app.apply_keybindings(bindings.read().as_deref(), false);

    install_panic_hook(options.mouse);
    let _guard = TerminalGuard::enter(options.mouse)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
    let size = terminal.size()?;

    let (msg_tx, mut msg_rx) = mpsc::unbounded_channel::<Msg>();
    let (conn_tx, mut conn_rx) = mpsc::unbounded_channel::<ConnEvent>();
    let mut io = Some(Io::new(client, msg_tx.clone()));
    let mut term_events = Some(EventStream::new());
    let mut last_housekeeping = Instant::now();
    let mut tick = tokio::time::interval(FRAME_INTERVAL);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut shutdown = Box::pin(termination());
    let mut last_title = String::new();
    let mut reconnecting = false;

    let mut effects = app.on_resize(size.width, size.height);
    effects.extend(app.start());
    if execute_effects(&mut app, io.as_ref(), effects) {
        return Ok(());
    }

    loop {
        let mut effects = Vec::new();
        tokio::select! {
            event = next_terminal_event(&mut term_events) => match event {
                Some(Ok(event)) => effects = handle_terminal_event(&mut app, event),
                Some(Err(err)) => {
                    tracing::error!(%err, "terminal event stream failed");
                    break;
                }
                None => break,
            },
            event = recv_daemon(&mut events) => match event {
                Some(event) => effects = app.handle_event(event),
                None => {
                    events = None;
                    io = None;
                    app.on_disconnected();
                    if !reconnecting && !app.quit {
                        reconnecting = true;
                        spawn_reconnect(paths.clone(), conn_tx.clone());
                    }
                }
            },
            Some(msg) = msg_rx.recv() => effects = app.handle_msg(msg),
            Some(conn) = conn_rx.recv() => match conn {
                ConnEvent::Failed => app.on_disconnected(),
                ConnEvent::Up(client, new_events) => {
                    events = Some(new_events);
                    io = Some(Io::new(client, msg_tx.clone()));
                    reconnecting = false;
                    effects = app.on_reconnected();
                }
            },
            _ = tick.tick() => {
                let now = Instant::now();
                app.tick(now);
                effects = app.poll_effects(now);
                if now.duration_since(last_housekeeping) >= HOUSEKEEPING_INTERVAL {
                    last_housekeeping = now;
                    housekeeping(&mut app, &mut bindings, &state_path, &mut saved_state);
                }
            }
            () = &mut shutdown => break,
        }
        let (suspends, effects): (Vec<_>, Vec<_>) = effects
            .into_iter()
            .partition(|e| matches!(e, Effect::Suspend(_)));
        if execute_effects(&mut app, io.as_ref(), effects) {
            break;
        }
        for effect in suspends {
            if let Effect::Suspend(cmd) = effect {
                run_external(
                    &mut app,
                    &mut terminal,
                    options.mouse,
                    &mut term_events,
                    &cmd,
                );
                // The editor may have changed the keybindings: apply them now.
                if let Some(text) = bindings.poll() {
                    app.apply_keybindings(text.as_deref(), true);
                }
            }
        }
        if app.dirty {
            app.dirty = false;
            let _ = execute!(io::stdout(), BeginSynchronizedUpdate);
            terminal.draw(|frame| ui::draw(frame, &mut app))?;
            let _ = execute!(io::stdout(), EndSynchronizedUpdate);
            let title = app.title();
            if title != last_title {
                let _ = execute!(io::stdout(), SetTitle(&title));
                last_title = title;
            }
        }
    }
    housekeeping(&mut app, &mut bindings, &state_path, &mut saved_state);
    Ok(())
}

/// Hot-reload `keybindings.json` and persist the UI state when it changed.
fn housekeeping(
    app: &mut App,
    bindings: &mut Watcher,
    state_path: &std::path::Path,
    saved: &mut TuiState,
) {
    if let Some(text) = bindings.poll() {
        app.apply_keybindings(text.as_deref(), true);
    }
    let snapshot = app.state_snapshot();
    if snapshot != *saved {
        match snapshot.save(state_path) {
            Ok(()) => *saved = snapshot,
            Err(err) => tracing::warn!(%err, "could not save the UI state"),
        }
    }
}

/// Run an external program (the editor) on the real terminal, then redraw the TUI.
fn run_external(
    app: &mut App,
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    mouse: bool,
    term_events: &mut Option<EventStream>,
    cmd: &ExternalCommand,
) {
    // Stop reading the keyboard so the program gets every key.
    *term_events = None;
    std::thread::sleep(Duration::from_millis(150));
    let result = run_on_terminal(mouse, || run_blocking(cmd));
    *term_events = Some(EventStream::new());
    // A fresh terminal object forgets what it believes is on screen, so the next frame repaints
    // everything on the new alternate screen (`Terminal::clear` asks the terminal for the cursor
    // position instead, which fights with the event reader).
    if let Ok(fresh) = Terminal::new(CrosstermBackend::new(io::stdout())) {
        *terminal = fresh;
    }
    app.dirty = true;
    match result {
        Ok(Ok(())) => {}
        Ok(Err(why)) => app.toast(ToastKind::Error, "Editor", &why),
        Err(err) => {
            app.toast(ToastKind::Error, "Terminal", &format!("{err:#}"));
        }
    }
}

async fn next_terminal_event(
    events: &mut Option<EventStream>,
) -> Option<std::io::Result<TermEvent>> {
    match events {
        Some(stream) => stream.next().await,
        None => std::future::pending().await,
    }
}

async fn recv_daemon(events: &mut Option<mpsc::UnboundedReceiver<Event>>) -> Option<Event> {
    match events {
        Some(rx) => rx.recv().await,
        None => std::future::pending().await,
    }
}

fn handle_terminal_event(app: &mut App, event: TermEvent) -> Vec<Effect> {
    match event {
        TermEvent::Key(key) => app.handle_key(key),
        TermEvent::Paste(text) => app.handle_paste(&text),
        TermEvent::Resize(w, h) => app.on_resize(w, h),
        TermEvent::FocusGained => app.on_focus(true),
        TermEvent::FocusLost => app.on_focus(false),
        TermEvent::Mouse(mouse) => app.handle_mouse(mouse),
    }
}

/// Run the effects; returns true when the app should exit.
fn execute_effects(app: &mut App, io: Option<&Io>, effects: Vec<Effect>) -> bool {
    let mut quit = false;
    for effect in effects {
        match effect {
            Effect::Quit => quit = true,
            Effect::Bell => {
                let mut out = io::stdout();
                let _ = out.write_all(b"\x07");
                let _ = out.flush();
            }
            Effect::Clipboard(text) => {
                // OSC 52: the terminal puts the text on the clipboard.
                use base64::Engine as _;
                let b64 = base64::engine::general_purpose::STANDARD.encode(text.as_bytes());
                let mut out = io::stdout();
                let _ = out.write_all(format!("\x1b]52;c;{b64}\x07").as_bytes());
                let _ = out.flush();
                app.toast(ToastKind::Info, "Copied", &text);
            }
            other => match io {
                Some(io) => io.run(other),
                None => offline(app, &other),
            },
        }
    }
    quit
}

/// An effect that needs the daemon while there is no connection.
fn offline(app: &mut App, effect: &Effect) {
    match effect {
        // Typing into a dead connection must not flood the screen with toasts.
        Effect::SendRaw(..)
        | Effect::Resize(..)
        | Effect::SetFocus(_)
        | Effect::Subscribe(_)
        | Effect::TermInput(..)
        | Effect::TermResize(..) => {}
        Effect::ShutdownAndQuit => {
            let _ = app.handle_msg(Msg::ShutdownDone(Err("not connected to the daemon".into())));
        }
        _ => app.toast(
            ToastKind::Warn,
            "Not connected",
            "The daemon is unreachable; reconnecting\u{2026}",
        ),
    }
}
