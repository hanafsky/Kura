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
use crate::app::{fs_error_message, App};
use crate::delete_confirm::DeleteKind;
use crate::fs_utils::{apply_sort, find_match, SortBy, DIRS_FIRST_ROW, SORT_MENU_ROWS};
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
    /// Jump to the next match of the current query.
    SearchNext,
    /// Leave the search and open the entry under the cursor.
    SearchConfirm,
    /// Open the sort menu.
    SortMenu,
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
            Command::SearchNext => self.search_step(),
            Command::SearchConfirm => {
                if matches!(self.mode, Mode::Search { .. }) {
                    self.mode = Mode::Filer;
                    self.open_selected();
                }
            }
            Command::SortMenu if filer => self.mode = Mode::Sort { selected: 0 },
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
                    self.search_step();
                }
                Mode::Rename { buffer, .. } => buffer.push(c),
                _ => {}
            },
            Command::DeleteChar => match &mut self.mode {
                Mode::Search { query } => {
                    query.pop();
                    self.search_step();
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

    /// Move the cursor to the next entry matching the search query.
    fn search_step(&mut self) {
        let Mode::Search { query } = &self.mode else {
            return;
        };
        let q = query.clone();
        let pane = self.current_pane_mut();
        if let Some(idx) = find_match(&pane.items, &q, pane.selected) {
            pane.selected = idx;
        }
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
}
