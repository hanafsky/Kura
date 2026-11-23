mod actions;
mod app;
mod clipboard;
mod command;
mod delete_confirm;
mod fs_utils;
mod graphics;
mod input;
mod input_source;
mod media;
mod mode;
mod paste;
mod ui;

use app::App;
use crossterm::{
    cursor::Show,
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use input::run_app;
use ratatui::{backend::CrosstermBackend, Terminal};
use std::io;

/// Put the terminal back into its normal state. Safe to call more than once.
fn restore_terminal() {
    let _ = disable_raw_mode();
    let _ = execute!(io::stdout(), LeaveAlternateScreen, Show);
}

/// Restore the terminal before the default panic message is printed, so the
/// message is readable and the shell is not left in raw mode.
fn install_panic_hook() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore_terminal();
        default_hook(info);
    }));
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    // Query the terminal for its graphics protocol and font size.
    let (picker, graphics_info) = graphics::detect();
    let mut app = App::new(picker)?;
    app.graphics_info = graphics_info;
    run_app(&mut terminal, &mut app)?;
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    install_panic_hook();
    // Start in ASCII (英数) input so Vim-like keys work immediately.
    input_source::select(input_source::ASCII_INPUT_SOURCE);
    enable_raw_mode()?;
    let res = run();
    restore_terminal();
    // Returning the error prints it to stderr after the terminal is restored.
    res
}
