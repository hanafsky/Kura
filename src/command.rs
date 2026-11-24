//! Commands: every user-visible operation as a value.
//!
//! `input.rs` turns key presses into [`Command`]s; [`App::apply`] carries
//! them out. Keeping the two apart lets key bindings, IPC and tests drive
//! the same operations without a terminal.

use std::ops::ControlFlow;

use crate::actions::{
    copy_selection, delete_items, paste, rename_selected, resolve_paste_conflict, target_paths,
    toggle_mark,
};
use crate::app::{fs_error_message, App, Pane};
use crate::delete_confirm::DeleteKind;
use crate::diff;
use crate::fs_utils::{apply_sort, find_match, SearchFrom, SortBy, DIRS_FIRST_ROW, SORT_MENU_ROWS};
use crate::media::open_external;
use crate::mode::{Mode, PaneType};
use crate::paste::ConflictChoice;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    /// Exit kura.
    Quit,
    /// Move down `n` rows: the cursor (file list / Visual), the scroll
    /// offset (text viewer) or the highlighted option (sort menu).
    MoveDown(usize),
    /// Move up `n` rows; see [`Command::MoveDown`].
    MoveUp(usize),
    /// Jump to the first entry (file list) or the top (text viewer).
    GoTop,
    /// Jump to the last entry (file list) or the bottom (text viewer).
    GoBottom,
    /// `h`: parent directory in the left pane, focus the left pane from the right one.
    MoveLeft,
    /// `l`: focus the right pane from the left one, parent directory in the right pane.
    MoveRight,
    /// Open the entry under the cursor: media preview, directory or text viewer.
    Open,
    /// Open the entry under the cursor with the system's default application.
    OpenExternal,
    /// Mark / unmark the entry under the cursor.
    ToggleMark,
    /// Start a Visual (range) selection at the cursor.
    VisualStart,
    /// Copy the marked entries (or the one under the cursor) to the clipboard.
    Yank,
    /// Paste the clipboard into the current directory.
    Paste,
    /// Copy the marked entries (or the one under the cursor) to the system
    /// clipboard as `@path` references for coding agents.
    CopyAtPaths,
    /// Ask for confirmation to delete the marked entries (or the one under the cursor).
    Delete(DeleteKind),
    /// Confirm the pending deletion.
    ConfirmDelete,
    /// Answer the current paste collision.
    ResolvePaste(ConflictChoice),
    /// Start renaming the entry under the cursor.
    RenameStart,
    /// Rename to the edited name.
    RenameCommit,
    /// Start an incremental search.
    SearchStart,
    /// Jump to the next match of the current query (while searching) or of
    /// the last search (`n` in the file list).
    SearchNext,
    /// Jump to the previous match of the last search (`N`).
    SearchPrev,
    /// Leave the search and open the entry under the cursor.
    SearchConfirm,
    /// Open the sort menu.
    SortMenu,
    /// Show only entries with git changes in the current pane, or all again.
    ToggleGitFilter,
    /// Show or hide hidden (dot) entries in the current pane.
    ToggleHidden,
    /// Hide or show entries ignored by git in the current pane.
    ToggleIgnored,
    /// Show the diff between the files under the cursors of both panes
    /// (or of the active file against HEAD).
    Diff,
    /// Sort by the highlighted option.
    SortConfirm,
    /// Type a character into the search query or rename buffer.
    InsertChar(char),
    /// Delete the last character of the search query or rename buffer.
    DeleteChar,
    /// Leave the current mode or dialog without committing anything.
    Back,
}

impl Command {
    /// Apply a numeric prefix (`5j`) to commands that take a count.
    pub fn with_count(self, count: usize) -> Self {
        match self {
            Command::MoveDown(n) => Command::MoveDown(n.saturating_mul(count)),
            Command::MoveUp(n) => Command::MoveUp(n.saturating_mul(count)),
            other => other,
        }
    }
}

impl App {
    /// Carry out a command. Returns `Break` when kura should exit.
    ///
    /// Commands that make no sense in the current mode are ignored.
    pub fn apply(&mut self, cmd: Command) -> ControlFlow<()> {
        let filer = matches!(self.mode, Mode::Filer);
        match cmd {
            Command::Quit => return ControlFlow::Break(()),
            Command::MoveDown(n) => self.move_by(n, true),
            Command::MoveUp(n) => self.move_by(n, false),
            Command::GoTop => match &mut self.mode {
                Mode::Filer => self.current_pane_mut().selected = 0,
                Mode::Viewer { offset, .. } => *offset = 0,
                _ => {}
            },
            Command::GoBottom => match &mut self.mode {
                Mode::Filer => {
                    let pane = self.current_pane_mut();
                    pane.selected = pane.items.len().saturating_sub(1);
                }
                Mode::Viewer {
                    offset, content, ..
                } => {
                    let total = content.lines().count();
                    *offset = total.saturating_sub(1) as u16;
                }
                _ => {}
            },
            Command::MoveLeft if filer => match self.active {
                PaneType::Left => self.on_left(),
                PaneType::Right => self.switch_pane(),
            },
            Command::MoveRight if filer => match self.active {
                PaneType::Left => self.switch_pane(),
                PaneType::Right => self.on_left(),
            },
            Command::Open if filer => self.open_selected(),
            Command::OpenExternal if filer || matches!(self.mode, Mode::Preview) => {
                if let Some(p) = self.selected_path() {
                    if let Err(e) = open_external(&p) {
                        self.notify_error(fs_error_message(&e, &p));
                    }
                }
            }
            Command::ToggleMark if filer => toggle_mark(self.current_pane_mut()),
            Command::VisualStart if filer => {
                let pane = self.current_pane_mut();
                let anchor = pane.selected;
                pane.marked.clear();
                pane.mark_index(anchor);
                self.mode = Mode::Visual { anchor };
            }
            Command::Yank if filer => copy_selection(self),
            Command::Paste if filer => paste(self),
            Command::CopyAtPaths if filer => crate::clipboard::copy_at_paths(self),
            Command::Delete(kind) if filer => {
                let items = target_paths(self);
                if !items.is_empty() {
                    self.mode = Mode::ConfirmDelete { items, kind };
                }
            }
            Command::ConfirmDelete => {
                if let Mode::ConfirmDelete { items, kind } = &mut self.mode {
                    let to_delete = std::mem::take(items);
                    let kind = *kind;
                    self.mode = Mode::Filer;
                    delete_items(self, &to_delete, kind);
                }
            }
            Command::ResolvePaste(choice) => resolve_paste_conflict(self, choice),
            Command::RenameStart if filer => {
                let pane = self.current_pane();
                if let Some(entry) = pane.items.get(pane.selected) {
                    let name = entry.file_name().to_string_lossy().into_owned();
                    self.mode = Mode::Rename {
                        original: name.clone(),
                        buffer: name,
                    };
                }
            }
            Command::RenameCommit => {
                if let Mode::Rename { buffer, .. } = &mut self.mode {
                    let new_name = std::mem::take(buffer);
                    self.mode = Mode::Filer;
                    rename_selected(self, &new_name);
                }
            }
            Command::SearchStart if filer => {
                self.mode = Mode::Search {
                    query: String::new(),
                };
            }
            Command::SearchNext if filer => self.repeat_search(SearchFrom::Next),
            Command::SearchPrev if filer => self.repeat_search(SearchFrom::Prev),
            Command::SearchNext => self.search_step(SearchFrom::Next),
            Command::SearchConfirm => {
                if matches!(self.mode, Mode::Search { .. }) {
                    self.end_search();
                    self.open_selected();
                }
            }
            Command::SortMenu if filer => self.mode = Mode::Sort { selected: 0 },
            Command::ToggleGitFilter if filer => self.toggle_git_filter(),
            Command::ToggleHidden if filer => self.toggle_hidden(),
            Command::ToggleIgnored if filer => self.toggle_ignored(),
            Command::Diff if filer => self.show_diff(),
            Command::SortConfirm => {
                if let Mode::Sort { selected } = self.mode {
                    self.mode = Mode::Filer;
                    let pane = self.current_pane_mut();
                    if selected == DIRS_FIRST_ROW {
                        pane.dirs_first = !pane.dirs_first;
                        pane.resort();
                    } else {
                        let by = match selected {
                            0 => SortBy::Modified,
                            1 => SortBy::Created,
                            2 => SortBy::Size,
                            _ => SortBy::Name,
                        };
                        apply_sort(pane, by);
                    }
                }
            }
            Command::InsertChar(c) => match &mut self.mode {
                Mode::Search { query } => {
                    query.push(c);
                    self.search_step(SearchFrom::Here);
                }
                Mode::Rename { buffer, .. } => buffer.push(c),
                _ => {}
            },
            Command::DeleteChar => match &mut self.mode {
                Mode::Search { query } => {
                    query.pop();
                    self.search_step(SearchFrom::Here);
                }
                Mode::Rename { buffer, .. } => {
                    buffer.pop();
                }
                _ => {}
            },
            Command::Back => match self.mode {
                Mode::Filer => {}
                Mode::Preview => self.close_preview(),
                Mode::ConfirmDelete { .. } => {
                    self.mode = Mode::Filer;
                    self.notify_info("Delete cancelled");
                }
                Mode::PasteConflict { .. } => resolve_paste_conflict(self, ConflictChoice::Cancel),
                Mode::Search { .. } => self.end_search(),
                _ => self.mode = Mode::Filer,
            },
            // Filer-only commands issued in another mode.
            _ => {}
        }
        ControlFlow::Continue(())
    }

    /// `MoveDown` / `MoveUp` in the current mode.
    fn move_by(&mut self, n: usize, down: bool) {
        match &mut self.mode {
            Mode::Filer | Mode::Visual { .. } => {
                for _ in 0..n {
                    if down {
                        self.on_down();
                    } else {
                        self.on_up();
                    }
                }
                if let Mode::Visual { anchor } = self.mode {
                    let pane = self.current_pane_mut();
                    pane.marked.clear();
                    let end = pane.selected;
                    let (lo, hi) = if anchor <= end {
                        (anchor, end)
                    } else {
                        (end, anchor)
                    };
                    for i in lo..=hi {
                        pane.mark_index(i);
                    }
                }
            }
            Mode::Viewer { offset, .. } => {
                *offset = if down {
                    offset.saturating_add(n as u16)
                } else {
                    offset.saturating_sub(n as u16)
                };
            }
            Mode::Sort { selected } => {
                let len = SORT_MENU_ROWS;
                for _ in 0..n {
                    *selected = if down {
                        (*selected + 1) % len
                    } else {
                        (*selected + len - 1) % len
                    };
                }
            }
            _ => {}
        }
    }

    /// Enter on the file list: media preview, else directory / text viewer.
    fn open_selected(&mut self) {
        let previewed = self.selected_path().is_some_and(|p| self.open_preview(&p));
        if !previewed {
            self.on_enter();
        }
    }

    /// `D`: diff the entries under the cursors of both panes and show it in
    /// the viewer, or report why there is nothing to diff.
    fn show_diff(&mut self) {
        let active_is_left = self.active == PaneType::Left;
        let (active, other) = if active_is_left {
            (&self.left, &self.right)
        } else {
            (&self.right, &self.left)
        };
        let plan = diff::plan(&diff_target(active), &diff_target(other), active_is_left);
        if let diff::Plan::Reason(reason) = plan {
            self.notify_info(reason);
            return;
        }
        match diff::run(&plan) {
            diff::Outcome::Diff(content) => {
                self.mode = Mode::Viewer {
                    content,
                    title: diff::title(&plan),
                    offset: 0,
                    diff: true,
                };
            }
            diff::Outcome::Same => self.notify_info(match plan {
                diff::Plan::Head(_) => "No changes against HEAD",
                _ => "Files are identical",
            }),
            diff::Outcome::Error(e) => self.notify_error(e),
        }
    }

    /// Move the cursor to an entry matching the search query being typed.
    fn search_step(&mut self, from: SearchFrom) {
        let Mode::Search { query } = &self.mode else {
            return;
        };
        let q = query.clone();
        let pane = self.current_pane_mut();
        if let Some(idx) = find_match(&entry_names(pane), &q, pane.selected, from) {
            pane.selected = idx;
        }
    }

    /// Leave the search, remembering a non-empty query for `n` / `N`.
    fn end_search(&mut self) {
        if let Mode::Search { query } = std::mem::replace(&mut self.mode, Mode::Filer) {
            if !query.is_empty() {
                self.last_search = Some(query);
            }
        }
    }

    /// `n` / `N`: jump to the next / previous match of the last search.
    fn repeat_search(&mut self, from: SearchFrom) {
        let Some(q) = self.last_search.clone() else {
            self.notify_info("No previous search");
            return;
        };
        let pane = self.current_pane_mut();
        match find_match(&entry_names(pane), &q, pane.selected, from) {
            Some(idx) => pane.selected = idx,
            None => self.notify_info(format!("Pattern not found: {q}")),
        }
    }
}

/// Names of the listed entries, in list order.
fn entry_names(pane: &crate::app::Pane) -> Vec<String> {
    pane.items
        .iter()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect()
}

/// What the cursor of `pane` is on, for [`diff::plan`].
fn diff_target(pane: &Pane) -> diff::Target {
    let Some(path) = pane.selected_path() else {
        return diff::Target::None;
    };
    if path.is_dir() {
        return diff::Target::Dir;
    }
    diff::Target::File {
        path: std::fs::canonicalize(&path).unwrap_or(path),
        in_repo: pane.git.is_some(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::MessageLevel;
    use ratatui_image::picker::Picker;
    use std::fs;
    use std::path::Path;

    fn app_in(dir: &Path) -> App {
        App::with_dir(Picker::halfblocks(), dir.to_path_buf()).unwrap()
    }

    /// A temp dir with files `a`, `b`, `c`, `d` (sorted by name).
    fn abcd() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        for name in ["a", "b", "c", "d"] {
            fs::write(tmp.path().join(name), name).unwrap();
        }
        tmp
    }

    fn selected_name(app: &App) -> String {
        let pane = app.current_pane();
        pane.items[pane.selected]
            .file_name()
            .to_string_lossy()
            .into_owned()
    }

    /// Indices of the marked entries, in list order.
    fn marked(app: &App) -> Vec<usize> {
        let pane = app.current_pane();
        (0..pane.items.len())
            .filter(|&i| pane.marked.contains(&pane.items[i].path()))
            .collect()
    }

    #[test]
    fn move_down_and_up_clamp_to_the_list() {
        let tmp = abcd();
        let mut app = app_in(tmp.path());
        let _ = app.apply(Command::MoveDown(2));
        assert_eq!(selected_name(&app), "c");
        let _ = app.apply(Command::MoveDown(10));
        assert_eq!(selected_name(&app), "d");
        let _ = app.apply(Command::MoveUp(1));
        assert_eq!(selected_name(&app), "c");
        let _ = app.apply(Command::GoTop);
        assert_eq!(selected_name(&app), "a");
        let _ = app.apply(Command::GoBottom);
        assert_eq!(selected_name(&app), "d");
    }

    #[test]
    fn quit_breaks() {
        let tmp = abcd();
        let mut app = app_in(tmp.path());
        assert!(app.apply(Command::Quit).is_break());
        assert!(app.apply(Command::MoveDown(1)).is_continue());
    }

    #[test]
    fn visual_selection_marks_the_range_and_survives_back() {
        let tmp = abcd();
        let mut app = app_in(tmp.path());
        let _ = app.apply(Command::MoveDown(1));
        let _ = app.apply(Command::VisualStart);
        assert!(matches!(app.mode, Mode::Visual { anchor: 1 }));
        let _ = app.apply(Command::MoveDown(2));
        assert_eq!(marked(&app), vec![1, 2, 3]);
        let _ = app.apply(Command::MoveUp(3));
        assert_eq!(marked(&app), vec![0, 1]);
        let _ = app.apply(Command::Back);
        assert!(matches!(app.mode, Mode::Filer));
        assert_eq!(marked(&app), vec![0, 1]);
    }

    #[test]
    fn delete_asks_for_the_marked_entries_then_deletes_on_confirm() {
        let tmp = abcd();
        let mut app = app_in(tmp.path());
        let _ = app.apply(Command::ToggleMark);
        let _ = app.apply(Command::MoveDown(2));
        let _ = app.apply(Command::ToggleMark);
        let _ = app.apply(Command::Delete(DeleteKind::Permanent));
        match &app.mode {
            Mode::ConfirmDelete { items, kind } => {
                assert_eq!(items, &vec![tmp.path().join("a"), tmp.path().join("c")]);
                assert_eq!(*kind, DeleteKind::Permanent);
            }
            _ => panic!("expected the delete dialog"),
        }
        let _ = app.apply(Command::ConfirmDelete);
        assert!(matches!(app.mode, Mode::Filer));
        assert!(!tmp.path().join("a").exists());
        assert!(!tmp.path().join("c").exists());
        assert!(tmp.path().join("b").exists());
        assert_eq!(app.current_pane().items.len(), 2);
        assert_eq!(app.message.as_ref().unwrap().text, "Deleted 2 item(s)");
    }

    #[test]
    fn x_asks_to_move_to_trash() {
        let tmp = abcd();
        let mut app = app_in(tmp.path());
        let _ = app.apply(Command::Delete(DeleteKind::Normal));
        match &app.mode {
            Mode::ConfirmDelete { items, kind } => {
                assert_eq!(items, &vec![tmp.path().join("a")]);
                assert_eq!(*kind, DeleteKind::Normal);
            }
            _ => panic!("expected the delete dialog"),
        }
    }

    /// Confirming `x` goes through the trash path, which reports a failure
    /// and never falls back to deleting from disk. A vanished entry is used
    /// so the test does not touch the real OS trash.
    #[test]
    fn confirm_trash_reports_failure_without_deleting() {
        let tmp = abcd();
        let mut app = app_in(tmp.path());
        let gone = tmp.path().join("gone");
        app.mode = Mode::ConfirmDelete {
            items: vec![gone],
            kind: DeleteKind::Normal,
        };
        let _ = app.apply(Command::ConfirmDelete);
        assert!(matches!(app.mode, Mode::Filer));
        let msg = app.message.as_ref().unwrap();
        assert_eq!(msg.level, MessageLevel::Error);
        assert!(msg.text.ends_with("(moved 0, failed 1)"), "{}", msg.text);
        assert!(tmp.path().join("a").exists());
    }

    #[test]
    fn delete_cancel_keeps_files() {
        let tmp = abcd();
        let mut app = app_in(tmp.path());
        let _ = app.apply(Command::Delete(DeleteKind::Normal));
        let _ = app.apply(Command::Back);
        assert!(matches!(app.mode, Mode::Filer));
        assert!(tmp.path().join("a").exists());
        assert_eq!(app.message.as_ref().unwrap().text, "Delete cancelled");
    }

    #[test]
    fn yank_switch_pane_and_paste() {
        let tmp = abcd();
        fs::create_dir(tmp.path().join("sub")).unwrap();
        let mut app = app_in(tmp.path());
        // Directories come first: sub, a, b, c, d.
        let _ = app.apply(Command::MoveDown(2));
        let _ = app.apply(Command::Yank);
        assert_eq!(app.clipboard, vec![tmp.path().join("b")]);

        let _ = app.apply(Command::MoveRight);
        assert!(app.active == PaneType::Right);
        assert_eq!(selected_name(&app), "sub");
        let _ = app.apply(Command::Open);
        assert_eq!(app.current_pane().current_dir, tmp.path().join("sub"));
        let _ = app.apply(Command::Paste);
        assert_eq!(fs::read_to_string(tmp.path().join("sub/b")).unwrap(), "b");

        // `l` in the right pane goes to the parent directory.
        let _ = app.apply(Command::MoveRight);
        assert_eq!(app.current_pane().current_dir, tmp.path());
        let _ = app.apply(Command::MoveLeft);
        assert!(app.active == PaneType::Left);
    }

    #[test]
    fn rename_edits_the_buffer_and_selects_the_new_name() {
        let tmp = abcd();
        let mut app = app_in(tmp.path());
        let _ = app.apply(Command::MoveDown(1));
        let _ = app.apply(Command::RenameStart);
        let _ = app.apply(Command::DeleteChar);
        for c in "zz".chars() {
            let _ = app.apply(Command::InsertChar(c));
        }
        let _ = app.apply(Command::RenameCommit);
        assert!(matches!(app.mode, Mode::Filer));
        assert!(!tmp.path().join("b").exists());
        assert_eq!(fs::read_to_string(tmp.path().join("zz")).unwrap(), "b");
        assert_eq!(selected_name(&app), "zz");
    }

    #[test]
    fn rename_back_changes_nothing() {
        let tmp = abcd();
        let mut app = app_in(tmp.path());
        let _ = app.apply(Command::RenameStart);
        let _ = app.apply(Command::InsertChar('x'));
        let _ = app.apply(Command::Back);
        assert!(matches!(app.mode, Mode::Filer));
        assert!(tmp.path().join("a").exists());
    }

    #[test]
    fn search_jumps_to_matches() {
        let tmp = tempfile::tempdir().unwrap();
        for name in ["apple", "banana", "cherry", "grape"] {
            fs::write(tmp.path().join(name), "").unwrap();
        }
        let mut app = app_in(tmp.path());
        let _ = app.apply(Command::SearchStart);
        let _ = app.apply(Command::InsertChar('r'));
        assert_eq!(selected_name(&app), "cherry");
        let _ = app.apply(Command::SearchNext);
        assert_eq!(selected_name(&app), "grape");
        let _ = app.apply(Command::Back);
        assert!(matches!(app.mode, Mode::Filer));
        assert_eq!(selected_name(&app), "grape");
    }

    #[test]
    fn search_confirm_opens_the_match() {
        let tmp = abcd();
        fs::create_dir(tmp.path().join("sub")).unwrap();
        let mut app = app_in(tmp.path());
        let _ = app.apply(Command::SearchStart);
        for c in "su".chars() {
            let _ = app.apply(Command::InsertChar(c));
        }
        let _ = app.apply(Command::SearchConfirm);
        assert!(matches!(app.mode, Mode::Filer));
        assert_eq!(app.current_pane().current_dir, tmp.path().join("sub"));
    }

    #[test]
    fn sort_menu_wraps_and_sorts_by_size() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("small"), "x").unwrap();
        fs::write(tmp.path().join("big"), "xxxxxxxx").unwrap();
        let mut app = app_in(tmp.path());
        // The cursor starts on "big" (name order) and stays on it.
        let _ = app.apply(Command::SortMenu);
        let _ = app.apply(Command::MoveUp(1));
        assert!(matches!(
            app.mode,
            Mode::Sort {
                selected: DIRS_FIRST_ROW
            }
        ));
        let _ = app.apply(Command::MoveUp(2));
        let _ = app.apply(Command::SortConfirm); // 2 = File size
        assert!(matches!(app.mode, Mode::Filer));
        assert_eq!(app.current_pane().sort, Some(SortBy::Size));
        assert_eq!(selected_name(&app), "big");
        let _ = app.apply(Command::MoveDown(1));
        assert_eq!(selected_name(&app), "small");
    }

    /// Names in list order.
    fn names(app: &App) -> Vec<String> {
        app.current_pane()
            .items
            .iter()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn sort_keeps_cursor_and_marks_on_the_same_files() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("a"), "xx").unwrap();
        fs::write(tmp.path().join("b"), "x").unwrap();
        fs::write(tmp.path().join("c"), "xxx").unwrap();
        let mut app = app_in(tmp.path());
        let _ = app.apply(Command::ToggleMark); // a
        let _ = app.apply(Command::MoveDown(1)); // b
        let _ = app.apply(Command::SortMenu);
        let _ = app.apply(Command::MoveDown(2));
        let _ = app.apply(Command::SortConfirm); // File size
        assert_eq!(names(&app), ["c", "a", "b"]);
        assert_eq!(selected_name(&app), "b");
        assert_eq!(marked(&app), vec![1]); // still "a"
    }

    #[test]
    fn refresh_keeps_sort_cursor_and_marks_after_external_changes() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("b"), "x").unwrap();
        fs::write(tmp.path().join("d"), "xxx").unwrap();
        let mut app = app_in(tmp.path());
        let _ = app.apply(Command::SortMenu);
        let _ = app.apply(Command::MoveDown(2));
        let _ = app.apply(Command::SortConfirm); // File size: d, b
        let _ = app.apply(Command::GoTop);
        let _ = app.apply(Command::ToggleMark); // d
        let _ = app.apply(Command::MoveDown(1)); // b

        // Files appear from outside kura.
        fs::write(tmp.path().join("a"), "xxxxx").unwrap();
        fs::write(tmp.path().join("c"), "").unwrap();
        app.current_pane_mut().refresh().unwrap();

        assert_eq!(names(&app), ["a", "d", "b", "c"]);
        assert_eq!(selected_name(&app), "b");
        assert_eq!(marked(&app), vec![1]);
        assert_eq!(app.current_pane().sort, Some(SortBy::Size));
    }

    #[test]
    fn refresh_drops_marks_of_removed_files_and_clamps_cursor() {
        let tmp = abcd();
        let mut app = app_in(tmp.path());
        let _ = app.apply(Command::ToggleMark); // a
        let _ = app.apply(Command::GoBottom);
        let _ = app.apply(Command::ToggleMark); // d
        fs::remove_file(tmp.path().join("d")).unwrap();
        app.current_pane_mut().refresh().unwrap();
        assert_eq!(selected_name(&app), "c");
        assert_eq!(marked(&app), vec![0]);
        assert_eq!(app.current_pane().marked.len(), 1);
    }

    #[test]
    fn paste_keeps_cursor_and_marks() {
        let tmp = abcd();
        let src = tempfile::tempdir().unwrap();
        fs::write(src.path().join("0new"), "n").unwrap();
        let mut app = app_in(tmp.path());
        app.clipboard = vec![src.path().join("0new")];
        let _ = app.apply(Command::MoveDown(1)); // b
        let _ = app.apply(Command::ToggleMark);
        let _ = app.apply(Command::MoveDown(1)); // c
        let _ = app.apply(Command::Paste);
        assert_eq!(names(&app), ["0new", "a", "b", "c", "d"]);
        assert_eq!(selected_name(&app), "c");
        assert_eq!(marked(&app), vec![2]);
    }

    #[test]
    fn delete_moves_cursor_to_the_next_row() {
        let tmp = abcd();
        let mut app = app_in(tmp.path());
        let _ = app.apply(Command::MoveDown(1)); // b
        let _ = app.apply(Command::Delete(DeleteKind::Normal));
        let _ = app.apply(Command::ConfirmDelete);
        assert_eq!(names(&app), ["a", "c", "d"]);
        assert_eq!(selected_name(&app), "c");
    }

    #[test]
    fn dirs_first_is_on_by_default_and_toggles_from_the_sort_menu() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("a"), "").unwrap();
        fs::create_dir(tmp.path().join("b")).unwrap();
        fs::write(tmp.path().join("c"), "").unwrap();
        fs::create_dir(tmp.path().join("d")).unwrap();
        let mut app = app_in(tmp.path());
        assert!(app.current_pane().dirs_first);
        assert!(app.left.dirs_first && app.right.dirs_first);
        assert_eq!(names(&app), ["b", "d", "a", "c"]);

        // Toggle off; the cursor stays on "c".
        let _ = app.apply(Command::GoBottom); // c
        let _ = app.apply(Command::SortMenu);
        let _ = app.apply(Command::MoveUp(1));
        let _ = app.apply(Command::SortConfirm);
        assert!(!app.current_pane().dirs_first);
        assert_eq!(names(&app), ["a", "b", "c", "d"]);
        assert_eq!(selected_name(&app), "c");

        // Kept by refresh and when entering / leaving directories.
        app.current_pane_mut().refresh().unwrap();
        assert_eq!(names(&app), ["a", "b", "c", "d"]);
        let _ = app.apply(Command::MoveUp(1)); // b
        let _ = app.apply(Command::Open);
        assert_eq!(app.current_pane().current_dir, tmp.path().join("b"));
        let _ = app.apply(Command::MoveLeft);
        assert_eq!(names(&app), ["a", "b", "c", "d"]);

        // Toggle back on.
        let _ = app.apply(Command::SortMenu);
        let _ = app.apply(Command::MoveUp(1));
        let _ = app.apply(Command::SortConfirm);
        assert_eq!(names(&app), ["b", "d", "a", "c"]);
    }

    #[test]
    fn sort_setting_is_kept_when_changing_directory() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir(tmp.path().join("sub")).unwrap();
        fs::write(tmp.path().join("sub/small"), "x").unwrap();
        fs::write(tmp.path().join("sub/big"), "xxxxxxxx").unwrap();
        let mut app = app_in(tmp.path());
        let _ = app.apply(Command::SortMenu);
        let _ = app.apply(Command::MoveDown(2));
        let _ = app.apply(Command::SortConfirm); // File size
        let _ = app.apply(Command::Open); // sub
        assert_eq!(app.current_pane().sort, Some(SortBy::Size));
        assert_eq!(names(&app), ["big", "small"]);
    }

    #[test]
    fn rename_keeps_other_marks() {
        let tmp = abcd();
        let mut app = app_in(tmp.path());
        let _ = app.apply(Command::GoBottom);
        let _ = app.apply(Command::ToggleMark); // d
        let _ = app.apply(Command::GoTop);
        let _ = app.apply(Command::RenameStart);
        let _ = app.apply(Command::InsertChar('z'));
        let _ = app.apply(Command::RenameCommit);
        assert_eq!(names(&app), ["az", "b", "c", "d"]);
        assert_eq!(selected_name(&app), "az");
        assert_eq!(marked(&app), vec![3]);
    }

    #[test]
    fn viewer_scrolls_and_closes() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("t.txt"), "1\n2\n3\n4\n5\n").unwrap();
        let mut app = app_in(tmp.path());
        let _ = app.apply(Command::Open);
        let _ = app.apply(Command::MoveDown(3));
        assert!(matches!(app.mode, Mode::Viewer { offset: 3, .. }));
        let _ = app.apply(Command::GoBottom);
        assert!(matches!(app.mode, Mode::Viewer { offset: 4, .. }));
        let _ = app.apply(Command::GoTop);
        assert!(matches!(app.mode, Mode::Viewer { offset: 0, .. }));
        let _ = app.apply(Command::Back);
        assert!(matches!(app.mode, Mode::Filer));
    }

    #[test]
    fn filer_commands_are_ignored_in_other_modes() {
        let tmp = abcd();
        let mut app = app_in(tmp.path());
        let _ = app.apply(Command::SortMenu);
        let _ = app.apply(Command::Delete(DeleteKind::Normal));
        let _ = app.apply(Command::Yank);
        assert!(matches!(app.mode, Mode::Sort { .. }));
        assert!(app.clipboard.is_empty());
    }

    fn last_copy() -> Option<String> {
        crate::clipboard::LAST_COPY.with(|c| c.borrow_mut().take())
    }

    #[test]
    fn copy_at_paths_copies_marked_entries_relative_to_launch_dir() {
        let tmp = abcd();
        let mut app = app_in(tmp.path());
        let _ = app.apply(Command::MoveDown(3));
        let _ = app.apply(Command::ToggleMark);
        let _ = app.apply(Command::MoveUp(2));
        let _ = app.apply(Command::ToggleMark);
        let _ = app.apply(Command::CopyAtPaths);
        assert_eq!(last_copy().as_deref(), Some("@b @d"));
        assert!(marked(&app).is_empty());
        assert_eq!(app.message.as_ref().unwrap().text, "Copied 2 path(s)");
    }

    #[test]
    fn copy_at_paths_without_marks_uses_the_cursor_entry() {
        let tmp = abcd();
        let mut app = app_in(tmp.path());
        let _ = app.apply(Command::MoveDown(2));
        let _ = app.apply(Command::CopyAtPaths);
        assert_eq!(last_copy().as_deref(), Some("@c"));
        assert_eq!(app.message.as_ref().unwrap().text, "Copied 1 path(s)");
    }

    #[test]
    fn copy_at_paths_is_ignored_outside_the_file_list() {
        let tmp = abcd();
        let mut app = app_in(tmp.path());
        let _ = app.apply(Command::SortMenu);
        let _ = app.apply(Command::CopyAtPaths);
        assert_eq!(last_copy(), None);
    }

    /// Run git in `dir` for test setup; returns false if git is unavailable.
    fn git(dir: &Path, args: &[&str]) -> bool {
        std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["-c", "user.name=t", "-c", "user.email=t@t"])
            .args(args)
            .env_remove("GIT_DIR")
            .env_remove("GIT_INDEX_FILE")
            .env_remove("GIT_WORK_TREE")
            .output()
            .is_ok_and(|o| o.status.success())
    }

    #[test]
    fn toggle_git_filter_shows_changed_entries_only() {
        let tmp = abcd();
        if !git(tmp.path(), &["init", "-q", "-b", "main"]) {
            return;
        }
        assert!(git(tmp.path(), &["add", "."]));
        assert!(git(tmp.path(), &["commit", "-qm", "init"]));
        fs::write(tmp.path().join("c"), "changed").unwrap();
        let mut app = app_in(tmp.path());
        let _ = app.apply(Command::ToggleGitFilter);
        assert!(app.current_pane().git_filter_active());
        assert_eq!(selected_name(&app), "c");
        assert_eq!(app.current_pane().items.len(), 1);
        // Ignored outside the file list.
        let _ = app.apply(Command::SortMenu);
        let _ = app.apply(Command::ToggleGitFilter);
        let _ = app.apply(Command::Back);
        assert!(app.current_pane().git_filter_active());
        let _ = app.apply(Command::ToggleGitFilter);
        assert!(!app.current_pane().git_filter_active());
        assert_eq!(app.current_pane().items.len(), 5); // .git, a, b, c, d
    }

    #[test]
    fn toggle_git_filter_outside_repo_reports_it() {
        let tmp = abcd();
        let mut app = app_in(tmp.path());
        if app.current_pane().git.is_some() {
            return; // the temp dir sits inside a repository on this machine
        }
        let _ = app.apply(Command::ToggleGitFilter);
        assert!(!app.current_pane().git_filter);
        assert_eq!(app.current_pane().items.len(), 4);
        let msg = app.message.as_ref().unwrap();
        assert_eq!(msg.text, "Not a git repository");
    }

    #[test]
    fn toggle_hidden_hides_dot_entries_and_drops_their_marks() {
        let tmp = abcd();
        fs::write(tmp.path().join(".env"), "").unwrap();
        fs::create_dir(tmp.path().join(".cache")).unwrap();
        let mut app = app_in(tmp.path());
        assert!(app.current_pane().show_hidden); // shown by default
        assert_eq!(names(&app), [".cache", ".env", "a", "b", "c", "d"]);
        let _ = app.apply(Command::ToggleMark); // .cache
        let _ = app.apply(Command::MoveDown(3)); // b
        let _ = app.apply(Command::ToggleMark);
        let _ = app.apply(Command::ToggleHidden);
        assert_eq!(names(&app), ["a", "b", "c", "d"]);
        assert_eq!(selected_name(&app), "b");
        assert_eq!(app.current_pane().marked, [tmp.path().join("b")].into());
        assert_eq!(app.message.as_ref().unwrap().text, "Hiding hidden files");

        // Kept when changing directory; per pane.
        fs::create_dir(tmp.path().join("sub")).unwrap();
        fs::write(tmp.path().join("sub/.x"), "").unwrap();
        fs::write(tmp.path().join("sub/y"), "").unwrap();
        app.current_pane_mut()
            .change_dir(tmp.path().join("sub"))
            .unwrap();
        assert_eq!(names(&app), ["y"]);
        assert!(app.right.show_hidden);

        let _ = app.apply(Command::ToggleHidden);
        assert_eq!(names(&app), [".x", "y"]);
        assert_eq!(app.message.as_ref().unwrap().text, "Showing hidden files");
        // Ignored outside the file list.
        let _ = app.apply(Command::SortMenu);
        let _ = app.apply(Command::ToggleHidden);
        assert!(app.current_pane().show_hidden);
    }

    #[test]
    fn n_and_shift_n_repeat_the_last_search_and_wrap() {
        let tmp = tempfile::tempdir().unwrap();
        for name in ["apple", "banana", "cherry", "grape"] {
            fs::write(tmp.path().join(name), "").unwrap();
        }
        let mut app = app_in(tmp.path());
        let _ = app.apply(Command::SearchNext);
        assert_eq!(app.message.as_ref().unwrap().text, "No previous search");
        assert_eq!(selected_name(&app), "apple");

        let _ = app.apply(Command::GoBottom);
        let _ = app.apply(Command::SearchStart);
        // The match is searched from the cursor over the whole list.
        let _ = app.apply(Command::InsertChar('a'));
        assert_eq!(selected_name(&app), "grape");
        let _ = app.apply(Command::InsertChar('n'));
        assert_eq!(selected_name(&app), "banana"); // wrapped to the top
        let _ = app.apply(Command::Back);
        assert_eq!(app.last_search.as_deref(), Some("an"));

        let _ = app.apply(Command::GoTop);
        let _ = app.apply(Command::SearchNext);
        assert_eq!(selected_name(&app), "banana");
        let _ = app.apply(Command::SearchNext);
        assert_eq!(selected_name(&app), "banana"); // the only match

        let _ = app.apply(Command::SearchStart);
        let _ = app.apply(Command::InsertChar('r'));
        assert_eq!(selected_name(&app), "cherry");
        let _ = app.apply(Command::SearchConfirm); // opens the viewer
        assert_eq!(app.last_search.as_deref(), Some("r"));
        let _ = app.apply(Command::Back);
        let _ = app.apply(Command::SearchNext);
        assert_eq!(selected_name(&app), "grape");
        let _ = app.apply(Command::SearchNext);
        assert_eq!(selected_name(&app), "cherry"); // wrapped
        let _ = app.apply(Command::SearchPrev);
        assert_eq!(selected_name(&app), "grape"); // wrapped back
        let _ = app.apply(Command::SearchPrev);
        assert_eq!(selected_name(&app), "cherry");

        // An empty search keeps the previous query.
        let _ = app.apply(Command::SearchStart);
        let _ = app.apply(Command::Back);
        assert_eq!(app.last_search.as_deref(), Some("r"));

        app.last_search = Some("zzz".into());
        let _ = app.apply(Command::SearchNext);
        assert_eq!(selected_name(&app), "cherry");
        assert_eq!(app.message.as_ref().unwrap().text, "Pattern not found: zzz");
    }

    #[test]
    fn typing_a_query_keeps_the_cursor_on_a_matching_entry() {
        let tmp = tempfile::tempdir().unwrap();
        for name in ["read", "ream", "red"] {
            fs::write(tmp.path().join(name), "").unwrap();
        }
        let mut app = app_in(tmp.path());
        let _ = app.apply(Command::MoveDown(1)); // ream
        let _ = app.apply(Command::SearchStart);
        let _ = app.apply(Command::InsertChar('r'));
        let _ = app.apply(Command::InsertChar('e'));
        assert_eq!(selected_name(&app), "ream");
        let _ = app.apply(Command::InsertChar('d'));
        assert_eq!(selected_name(&app), "red");
    }

    #[test]
    fn toggle_ignored_hides_gitignored_entries_and_combines_with_m() {
        let tmp = abcd();
        if !git(tmp.path(), &["init", "-q", "-b", "main"]) {
            return;
        }
        fs::write(tmp.path().join(".gitignore"), "target/\n*.log\n").unwrap();
        assert!(git(tmp.path(), &["add", "."]));
        assert!(git(tmp.path(), &["commit", "-qm", "init"]));
        fs::create_dir(tmp.path().join("target")).unwrap();
        fs::write(tmp.path().join("target/out"), "").unwrap();
        fs::write(tmp.path().join("x.log"), "").unwrap();
        fs::write(tmp.path().join("new"), "").unwrap();
        fs::write(tmp.path().join("c"), "changed").unwrap();
        let mut app = app_in(tmp.path());
        assert_eq!(
            names(&app),
            [
                ".git",
                "target",
                ".gitignore",
                "a",
                "b",
                "c",
                "d",
                "new",
                "x.log"
            ]
        );
        let _ = app.apply(Command::GoBottom); // x.log
        let _ = app.apply(Command::ToggleMark);
        let _ = app.apply(Command::MoveUp(4)); // b
        let _ = app.apply(Command::ToggleMark);
        let _ = app.apply(Command::ToggleIgnored);
        assert!(app.current_pane().ignore_filter_active());
        assert_eq!(
            names(&app),
            [".git", ".gitignore", "a", "b", "c", "d", "new"]
        );
        assert_eq!(selected_name(&app), "b");
        assert_eq!(app.current_pane().marked, [tmp.path().join("b")].into());
        assert_eq!(
            app.message.as_ref().unwrap().text,
            "Hiding git-ignored files"
        );

        // Combined with `M` and `.`.
        let _ = app.apply(Command::ToggleGitFilter);
        assert_eq!(names(&app), ["c", "new"]);
        let _ = app.apply(Command::ToggleGitFilter);
        let _ = app.apply(Command::ToggleHidden);
        assert_eq!(names(&app), ["a", "b", "c", "d", "new"]);
        let _ = app.apply(Command::ToggleHidden);

        // Inside an ignored directory nothing is hidden.
        let _ = app.apply(Command::ToggleIgnored);
        let _ = app.apply(Command::GoTop);
        let _ = app.apply(Command::MoveDown(1)); // target
        let _ = app.apply(Command::Open);
        let _ = app.apply(Command::ToggleIgnored);
        assert_eq!(app.current_pane().current_dir, tmp.path().join("target"));
        assert!(app.current_pane().hide_ignored);
        assert!(!app.current_pane().ignore_filter_active());
        assert_eq!(names(&app), ["out"]);
        assert_eq!(
            app.message.as_ref().unwrap().text,
            "Already inside an ignored directory"
        );
        let _ = app.apply(Command::MoveLeft);
        assert!(!names(&app).contains(&"target".to_string()));

        let _ = app.apply(Command::ToggleIgnored);
        assert_eq!(
            app.message.as_ref().unwrap().text,
            "Showing git-ignored files"
        );
        assert_eq!(names(&app).len(), 9);
    }

    #[test]
    fn toggle_ignored_outside_repo_reports_it() {
        let tmp = abcd();
        let mut app = app_in(tmp.path());
        if app.current_pane().git.is_some() {
            return;
        }
        let _ = app.apply(Command::ToggleIgnored);
        assert!(!app.current_pane().hide_ignored);
        assert_eq!(app.message.as_ref().unwrap().text, "Not a git repository");
    }

    fn diff_content(app: &App) -> &str {
        match &app.mode {
            Mode::Viewer {
                content,
                diff: true,
                ..
            } => content,
            _ => panic!("not in the diff viewer"),
        }
    }

    fn message(app: &App) -> &str {
        app.message.as_ref().map_or("", |m| m.text.as_str())
    }

    #[test]
    fn diff_shows_files_under_both_cursors_left_to_right() {
        let left = tempfile::tempdir().unwrap();
        let right = tempfile::tempdir().unwrap();
        fs::write(left.path().join("a"), "same\nold\n").unwrap();
        fs::write(right.path().join("a"), "same\nnew\n").unwrap();
        let mut app = app_in(left.path());
        app.right.change_dir(right.path().to_path_buf()).unwrap();
        let _ = app.apply(Command::MoveRight); // focus the right pane
        let _ = app.apply(Command::Diff);
        let content = diff_content(&app);
        assert!(content.contains("\n-old\n"), "{content}");
        assert!(content.contains("\n+new\n"), "{content}");
        // Scrolls like the text viewer and closes back to the file list.
        let _ = app.apply(Command::MoveDown(2));
        assert!(matches!(app.mode, Mode::Viewer { offset: 2, .. }));
        let _ = app.apply(Command::Back);
        assert!(matches!(app.mode, Mode::Filer));
    }

    #[test]
    fn diff_reports_same_file_identical_files_and_directories() {
        let tmp = abcd();
        let mut app = app_in(tmp.path());
        if app.current_pane().git.is_some() {
            return; // the temp dir sits inside a repository on this machine
        }
        let _ = app.apply(Command::Diff);
        assert!(matches!(app.mode, Mode::Filer));
        assert_eq!(message(&app), diff::SAME_FILE);

        let other = tempfile::tempdir().unwrap();
        fs::write(other.path().join("a"), "a").unwrap();
        app.right.change_dir(other.path().to_path_buf()).unwrap();
        let _ = app.apply(Command::Diff);
        assert!(matches!(app.mode, Mode::Filer));
        assert_eq!(message(&app), "Files are identical");

        fs::create_dir(other.path().join("0dir")).unwrap();
        app.right.refresh().unwrap();
        app.right.selected = 0;
        let _ = app.apply(Command::Diff);
        assert_eq!(message(&app), diff::DIRECTORY);
    }

    #[test]
    fn diff_is_ignored_outside_the_file_list() {
        let left = tempfile::tempdir().unwrap();
        let right = tempfile::tempdir().unwrap();
        fs::write(left.path().join("a"), "1").unwrap();
        fs::write(right.path().join("a"), "2").unwrap();
        let mut app = app_in(left.path());
        app.right.change_dir(right.path().to_path_buf()).unwrap();
        let _ = app.apply(Command::SortMenu);
        let _ = app.apply(Command::Diff);
        assert!(matches!(app.mode, Mode::Sort { .. }));
    }

    #[test]
    fn diff_of_repo_file_against_directory_or_itself_is_against_head() {
        let tmp = abcd();
        if !git(tmp.path(), &["init", "-q", "-b", "main"]) {
            return;
        }
        assert!(git(tmp.path(), &["add", "."]));
        assert!(git(tmp.path(), &["commit", "-qm", "init"]));
        fs::write(tmp.path().join("a"), "changed\n").unwrap();
        let mut app = app_in(tmp.path());
        // The right pane's cursor is on `.git`, a directory.
        assert_eq!(app.right.selected_path().unwrap(), tmp.path().join(".git"));
        let _ = app.apply(Command::MoveDown(1)); // a
        let _ = app.apply(Command::Diff);
        let content = diff_content(&app);
        assert!(content.contains("\n-a"), "{content}");
        assert!(content.contains("\n+changed\n"), "{content}");

        // Both panes on the same file: also against HEAD.
        let _ = app.apply(Command::Back);
        app.right.selected = 1; // a
        let _ = app.apply(Command::Diff);
        assert!(diff_content(&app).contains("\n+changed\n"));

        // An unchanged file has nothing to show.
        let _ = app.apply(Command::Back);
        app.right.selected = 0; // .git
        let _ = app.apply(Command::MoveDown(1)); // b
        let _ = app.apply(Command::Diff);
        assert!(matches!(app.mode, Mode::Filer));
        assert_eq!(message(&app), "No changes against HEAD");
    }
}
