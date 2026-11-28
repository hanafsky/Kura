mod actions;
mod app;
mod cli;
mod clipboard;
mod command;
mod delete_confirm;
mod diff;
mod editor;
mod fs_utils;
mod git;
mod graphics;
mod input;
mod input_source;
mod ipc;
mod markdown;
mod media;
mod mode;
mod paste;
mod preview;
mod terminal;
mod text_view;
mod ui;
mod watch;

use app::App;
use clap::Parser;
use cli::{Cli, CliCommand, Message};
use input::run_app;
use ratatui::{backend::CrosstermBackend, Terminal};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;

/// Run the TUI. Returns the active pane's directory at exit.
fn run(
    left: PathBuf,
    right: PathBuf,
    launch_dir: PathBuf,
    ipc_rx: Receiver<ipc::Request>,
    ipc_error: Option<String>,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend)?;
    // Query the terminal for its graphics protocol and font size.
    // Keys typed meanwhile are kept and handled first.
    let detected = graphics::detect();
    let mut app = App::with_panes(detected.picker, left, right, launch_dir)?;
    app.graphics_info = detected.info;
    if let Some(e) = ipc_error {
        app.notify_error(e);
    }
    run_app(&mut terminal, &mut app, detected.typeahead, &ipc_rx)?;
    Ok(app.current_pane().current_dir.clone())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // `--help` / `--version` / usage errors exit here, before the TUI starts.
    let cli = Cli::parse();
    let cwd = std::env::current_dir()?;
    if let Some(CliCommand::Msg { socket, message }) = cli.command {
        if cli.left.is_some() || cli.last_dir_path.is_some() {
            eprintln!("kura: msg cannot be combined with directories or options");
            std::process::exit(2);
        }
        std::process::exit(send_message(socket, message, &cwd));
    }
    let (left, right) = match (
        cli::resolve_dir(cli.left.as_deref(), &cwd),
        cli::resolve_dir(cli.right.as_deref(), &cwd),
    ) {
        (Ok(l), Ok(r)) => (l, r),
        (Err(e), _) | (_, Err(e)) => {
            eprintln!("kura: {e}");
            std::process::exit(2);
        }
    };

    // Listen for `kura msg` before any thread starts, so that setting
    // KURA_SOCKET (inherited by the editor and other children) is safe.
    // Dropped at the end of main, which removes the socket.
    let (ipc_tx, ipc_rx) = std::sync::mpsc::channel();
    let ipc_server = match ipc::Server::start(ipc_tx, |r| r) {
        Ok(server) => {
            std::env::set_var(ipc::SOCKET_ENV, server.path());
            Ok(server)
        }
        Err(e) => Err(format!("kura msg disabled: {e}")),
    };

    terminal::install_panic_hook();
    // Start in ASCII (英数) input so Vim-like keys work immediately.
    input_source::select(input_source::ASCII_INPUT_SOURCE);
    terminal::enter()?;
    let res = run(left, right, cwd, ipc_rx, ipc_server.as_ref().err().cloned());
    terminal::restore();
    // Returning the error prints it to stderr after the terminal is restored.
    drop(ipc_server);
    let last_dir = res?;
    if let Some(file) = cli.last_dir_path {
        std::fs::write(&file, last_dir.as_os_str().as_encoded_bytes())
            .map_err(|e| format!("cannot write {}: {e}", file.display()))?;
    }
    Ok(())
}

/// `kura msg ...`: send one request to a running kura. Returns the exit
/// status (0 when kura accepted it).
fn send_message(socket: Option<PathBuf>, message: Message, cwd: &Path) -> i32 {
    let request = match message {
        Message::Reveal { path } => {
            ipc::resolve_path(&path, cwd).map(|path| ipc::Request::Reveal { path })
        }
        Message::Cd { path } => ipc::resolve_path(&path, cwd).and_then(|path| {
            if path.is_dir() {
                Ok(ipc::Request::Cd { path })
            } else {
                Err(format!("{}: Not a directory", path.display()))
            }
        }),
    };
    let env = std::env::var_os(ipc::SOCKET_ENV);
    let socket = ipc::explicit_socket(socket.as_deref(), env.as_deref());
    match request.and_then(|r| ipc::send(socket.as_deref(), &r)) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("kura: {e}");
            1
        }
    }
}
