//! `e`: open a file in `$VISUAL` / `$EDITOR`, suspending the TUI meanwhile.

use std::io;
use std::path::Path;
use std::process::ExitStatus;

use ratatui::{backend::Backend, Terminal};

use crate::app::{fs_error_message, App};

/// Editor used when neither `$VISUAL` nor `$EDITOR` is set.
pub const FALLBACK_EDITOR: &str = "vi";

/// Choose the editor command: the first non-blank of `$VISUAL`, `$EDITOR`,
/// else `vi`. The value is split on whitespace into program and arguments
/// (e.g. `code -w` → `code` + `["-w"]`), and run as `<program> <args...> <file>`.
pub fn editor_command(visual: Option<&str>, editor: Option<&str>) -> (String, Vec<String>) {
    [visual, editor]
        .into_iter()
        .flatten()
        .find_map(|value| {
            let mut words = value.split_whitespace().map(str::to_string);
            let program = words.next()?;
            Some((program, words.collect()))
        })
        .unwrap_or_else(|| (FALLBACK_EDITOR.to_string(), Vec::new()))
}

/// Hand the terminal to the editor for `path`, then restore the TUI, redraw
/// from scratch and refresh both panes. Failures go to the status line.
pub fn edit<B: Backend<Error = io::Error>>(
    terminal: &mut Terminal<B>,
    app: &mut App,
    path: &Path,
) -> io::Result<()> {
    let visual = std::env::var("VISUAL").ok();
    let editor = std::env::var("EDITOR").ok();
    let (program, args) = editor_command(visual.as_deref(), editor.as_deref());

    crate::terminal::restore();
    let result = std::process::Command::new(&program)
        .args(&args)
        .arg(path)
        .status();
    crate::terminal::enter()?;
    // The screen was used by the editor: force a full redraw.
    terminal.clear()?;

    match result {
        Ok(status) => {
            if let Some(msg) = exit_message(&program, status) {
                app.notify_error(msg);
            }
        }
        Err(e) => app.notify_error(fs_error_message(&e, Path::new(&program))),
    }
    let _ = app.left.refresh();
    let _ = app.right.refresh();
    Ok(())
}

/// Status-line message for an editor that did not exit successfully.
fn exit_message(program: &str, status: ExitStatus) -> Option<String> {
    if status.success() {
        return None;
    }
    Some(match status.code() {
        Some(code) => format!("{program} exited with status {code}"),
        None => format!("{program} was terminated by a signal"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cmd(program: &str, args: &[&str]) -> (String, Vec<String>) {
        (
            program.to_string(),
            args.iter().map(|s| s.to_string()).collect(),
        )
    }

    #[test]
    fn visual_wins_over_editor() {
        assert_eq!(editor_command(Some("nvim"), Some("nano")), cmd("nvim", &[]));
    }

    #[test]
    fn editor_is_used_without_visual() {
        assert_eq!(editor_command(None, Some("nano")), cmd("nano", &[]));
    }

    #[test]
    fn blank_values_are_skipped() {
        assert_eq!(editor_command(Some("  "), Some("hx")), cmd("hx", &[]));
        assert_eq!(editor_command(Some(""), Some(" ")), cmd("vi", &[]));
    }

    #[test]
    fn falls_back_to_vi() {
        assert_eq!(editor_command(None, None), cmd("vi", &[]));
    }

    #[test]
    fn arguments_are_split_on_whitespace() {
        assert_eq!(
            editor_command(None, Some("code  -w --new-window")),
            cmd("code", &["-w", "--new-window"])
        );
    }
}
