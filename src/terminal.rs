//! Entering and leaving the full-screen TUI.
//!
//! Used at startup and exit, by the panic hook, and to hand the terminal to
//! an external program (`$EDITOR`) and take it back afterwards.

use crossterm::{
    cursor::Show,
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use std::io;

/// Switch to raw mode and the alternate screen.
pub fn enter() -> io::Result<()> {
    enable_raw_mode()?;
    execute!(io::stdout(), EnterAlternateScreen)
}

/// Put the terminal back into its normal state. Safe to call more than once
/// (also from the panic hook).
pub fn restore() {
    let _ = disable_raw_mode();
    let _ = execute!(io::stdout(), LeaveAlternateScreen, Show);
}

/// Restore the terminal before the default panic message is printed, so the
/// message is readable and the shell is not left in raw mode.
pub fn install_panic_hook() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore();
        default_hook(info);
    }));
}
