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
mod media;
mod mode;
mod paste;
mod terminal;
mod ui;

use app::App;
use clap::Parser;
use cli::Cli;
use input::run_app;
use ratatui::{backend::CrosstermBackend, Terminal};
use std::io;
use std::path::PathBuf;

/// Run the TUI. Returns the active pane's directory at exit.
fn run(
    left: PathBuf,
    right: PathBuf,
    launch_dir: PathBuf,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend)?;
    // Query the terminal for its graphics protocol and font size.
    let (picker, graphics_info) = graphics::detect();
    let mut app = App::with_panes(picker, left, right, launch_dir)?;
    app.graphics_info = graphics_info;
    run_app(&mut terminal, &mut app)?;
    Ok(app.current_pane().current_dir.clone())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // `--help` / `--version` / usage errors exit here, before the TUI starts.
    let cli = Cli::parse();
    let cwd = std::env::current_dir()?;
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

    terminal::install_panic_hook();
    // Start in ASCII (英数) input so Vim-like keys work immediately.
    input_source::select(input_source::ASCII_INPUT_SOURCE);
    terminal::enter()?;
    let res = run(left, right, cwd);
    terminal::restore();
    // Returning the error prints it to stderr after the terminal is restored.
    let last_dir = res?;
    if let Some(file) = cli.last_dir_path {
        std::fs::write(&file, last_dir.as_os_str().as_encoded_bytes())
            .map_err(|e| format!("cannot write {}: {e}", file.display()))?;
    }
    Ok(())
}
