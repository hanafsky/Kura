//! Delete confirmation dialog: key handling and dialog contents.
//!
//! Kept free of terminal I/O so the behaviour can be unit tested.

use crossterm::event::KeyCode;
use std::path::{Path, PathBuf};

/// How the confirmed items will be removed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeleteKind {
    /// `x`: regular delete (currently also permanent; becomes trash in #12).
    Normal,
    /// `X`: permanent delete that cannot be undone.
    Permanent,
}

/// Maximum number of file names listed in the dialog.
pub const MAX_LISTED_NAMES: usize = 10;

/// Outcome of a key press while the dialog is open.
#[derive(Debug, PartialEq, Eq)]
pub enum ConfirmAction {
    Delete,
    Cancel,
    /// Key has no meaning in the dialog; keep it open.
    Ignore,
}

/// Only `y` / `Y` confirms. Enter is treated as the default answer ("No").
pub fn confirm_key(code: KeyCode) -> ConfirmAction {
    match code {
        KeyCode::Char('y') | KeyCode::Char('Y') => ConfirmAction::Delete,
        KeyCode::Char('n')
        | KeyCode::Char('N')
        | KeyCode::Char('q')
        | KeyCode::Esc
        | KeyCode::Enter => ConfirmAction::Cancel,
        _ => ConfirmAction::Ignore,
    }
}

/// Dialog title for the given kind.
pub fn dialog_title(kind: DeleteKind) -> &'static str {
    match kind {
        DeleteKind::Normal => "Confirm Deletion",
        DeleteKind::Permanent => "Confirm Permanent Deletion",
    }
}

fn display_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// Text lines shown inside the dialog (without borders).
///
/// `max_lines` is the number of rows available inside the dialog; the name
/// list is shortened (at most [`MAX_LISTED_NAMES`] names) so that the
/// question and the "(y/N)" prompt always fit.
pub fn dialog_lines(items: &[PathBuf], kind: DeleteKind, max_lines: usize) -> Vec<String> {
    let n = items.len();
    let mut header = vec![match kind {
        DeleteKind::Normal => format!("Delete {n} item(s)?"),
        DeleteKind::Permanent => format!("Permanently delete {n} item(s)?"),
    }];
    // Both `x` and `X` remove files from disk today, so both warn.
    // TODO(#12): drop the warning for `x` once it moves items to the trash.
    header.push("This cannot be undone.".to_string());
    // header + blank + names + blank + prompt
    let fixed = header.len() + 3;
    let room = max_lines.saturating_sub(fixed).min(MAX_LISTED_NAMES);

    let mut names: Vec<String> = Vec::new();
    if room > 0 && n > 0 {
        if n <= room {
            names.extend(items.iter().map(|p| format!("  {}", display_name(p))));
        } else {
            // Reserve one row for the "and M more" line.
            let shown = room - 1;
            names.extend(
                items
                    .iter()
                    .take(shown)
                    .map(|p| format!("  {}", display_name(p))),
            );
            names.push(format!("  ... and {} more", n - shown));
        }
    }

    let mut lines = header;
    if !names.is_empty() {
        lines.push(String::new());
        lines.extend(names);
    }
    lines.push(String::new());
    lines.push("Press y to delete, n/Esc/Enter to cancel (y/N)".to_string());
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(n: usize) -> Vec<PathBuf> {
        (0..n).map(|i| PathBuf::from(format!("/d/f{i}"))).collect()
    }

    #[test]
    fn only_y_confirms() {
        assert_eq!(confirm_key(KeyCode::Char('y')), ConfirmAction::Delete);
        assert_eq!(confirm_key(KeyCode::Char('Y')), ConfirmAction::Delete);
        for k in [
            KeyCode::Enter,
            KeyCode::Esc,
            KeyCode::Char('n'),
            KeyCode::Char('N'),
            KeyCode::Char('q'),
        ] {
            assert_eq!(confirm_key(k), ConfirmAction::Cancel, "{k:?}");
        }
        assert_eq!(confirm_key(KeyCode::Char('j')), ConfirmAction::Ignore);
    }

    #[test]
    fn lists_all_names_when_few() {
        let lines = dialog_lines(&paths(3), DeleteKind::Normal, 100);
        assert_eq!(lines[0], "Delete 3 item(s)?");
        assert!(lines.contains(&"  f0".to_string()));
        assert!(lines.contains(&"  f2".to_string()));
        assert!(!lines.iter().any(|l| l.contains("more")));
        assert!(lines.last().unwrap().contains("(y/N)"));
    }

    #[test]
    fn truncates_long_lists() {
        let lines = dialog_lines(&paths(25), DeleteKind::Normal, 100);
        let names: Vec<_> = lines.iter().filter(|l| l.starts_with("  f")).collect();
        assert_eq!(names.len(), MAX_LISTED_NAMES - 1);
        assert!(lines.contains(&format!("  ... and {} more", 25 - (MAX_LISTED_NAMES - 1))));
    }

    #[test]
    fn exactly_max_names_needs_no_more_line() {
        let lines = dialog_lines(&paths(MAX_LISTED_NAMES), DeleteKind::Normal, 100);
        assert!(!lines.iter().any(|l| l.contains("more")));
    }

    #[test]
    fn fits_small_heights() {
        for h in 0..20 {
            let lines = dialog_lines(&paths(25), DeleteKind::Permanent, h);
            // Question and prompt are always present.
            assert!(lines[0].starts_with("Permanently delete 25"));
            assert!(lines.last().unwrap().contains("(y/N)"));
            if h >= 5 {
                assert!(lines.len() <= h, "h={h} len={}", lines.len());
            }
        }
    }

    #[test]
    fn both_kinds_warn_irreversible() {
        let lines = dialog_lines(&paths(1), DeleteKind::Permanent, 100);
        assert_eq!(lines[0], "Permanently delete 1 item(s)?");
        assert_eq!(lines[1], "This cannot be undone.");
        let normal = dialog_lines(&paths(1), DeleteKind::Normal, 100);
        assert_eq!(normal[0], "Delete 1 item(s)?");
        assert_eq!(normal[1], "This cannot be undone.");
        assert_ne!(
            dialog_title(DeleteKind::Normal),
            dialog_title(DeleteKind::Permanent)
        );
    }
}
