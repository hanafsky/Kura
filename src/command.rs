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
use crate::app::{fs_error_message, media_flags, App, Pane};
use crate::delete_confirm::DeleteKind;
use crate::diff;
use crate::fs_utils::{apply_sort, find_match, SearchFrom, SortBy, DIRS_FIRST_ROW, SORT_MENU_ROWS};
use crate::media::open_external;
use crate::mode::{Mode, PaneType};
use crate::paste::ConflictChoice;
use crate::preview::{prefetch_around, step_media};
use crate::text_view::{self, TextDoc};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    /// Exit kura.
    Quit,
    /// Move down `n` rows: the cursor (file list / Visual), the scroll
    /// offset (text viewer), the highlighted option (sort menu) or to the
    /// `n`-th next image / video (preview).
    MoveDown(usize),
    /// Move up `n` rows; see [`Command::MoveDown`].
    MoveUp(usize),
    /// Scroll the text viewer down `n` screens (`PageDown`, `Ctrl-f`).
    PageDown(usize),
    /// Scroll the text viewer up `n` screens (`PageUp`, `Ctrl-b`).
    PageUp(usize),
    /// Scroll the text viewer down `n` half screens (`Ctrl-d`).
    HalfPageDown(usize),
    /// Scroll the text viewer up `n` half screens (`Ctrl-u`).
    HalfPageUp(usize),
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
    /// Open the entry (file or directory) under the cursor in `$VISUAL` / `$EDITOR` / `vi`.
    OpenEditor,
    /// Mark / unmark the entry under the cursor.
    ToggleMark,
    /// Start a Visual (range) selection at the cursor.
    VisualStart,
    /// Copy the marked entries (or the one under the cursor) to the clipboard.
    Yank,
    /// Paste the clipboard into the current directory.
    Paste,
    /// Hand the marked entries (or the one under the cursor) to a coding
    /// agent as `@path` references: typed into an agent pane in herdr,
    /// copied to the system clipboard elsewhere.
    CopyAtPaths,
    /// Send to the agent highlighted in the target dialog.
    SendConfirm,
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
    /// Show or hide the text preview of the entry under the cursor in the
    /// opposite pane.
    ToggleTextPreview,
    /// Switch a Markdown file between the rendered and the source view, in
    /// the text viewer or in the text preview.
    ToggleMarkdown,
    /// Re-read both panes from disk (also works with `KURA_WATCH=off`).
    Refresh,
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
            Command::PageDown(n) => Command::PageDown(n.saturating_mul(count)),
            Command::PageUp(n) => Command::PageUp(n.saturating_mul(count)),
            Command::HalfPageDown(n) => Command::HalfPageDown(n.saturating_mul(count)),
            Command::HalfPageUp(n) => Command::HalfPageUp(n.saturating_mul(count)),
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
            Command::PageDown(n) => self.scroll_pages(n, false, true),
            Command::PageUp(n) => self.scroll_pages(n, false, false),
            Command::HalfPageDown(n) => self.scroll_pages(n, true, true),
            Command::HalfPageUp(n) => self.scroll_pages(n, true, false),
            Command::GoTop => match &mut self.mode {
                Mode::Filer => self.current_pane_mut().selected = 0,
                Mode::Viewer { offset, at_end, .. } => {
                    *offset = 0;
                    *at_end = false;
                }
                _ => {}
            },
            Command::GoBottom => match &mut self.mode {
                Mode::Filer => {
                    let pane = self.current_pane_mut();
                    pane.selected = pane.items.len().saturating_sub(1);
                }
                Mode::Viewer {
                    offset,
                    doc,
                    at_end,
                    ..
                } => {
                    *at_end = true;
                    // The last wrapped row at the bottom of the screen; the
                    // draw limits the offset when the size is not known yet.
                    *offset = match self.viewer_size {
                        Some((w, h)) => text_view::max_offset(doc, w, h),
                        None => usize::MAX,
                    };
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
            Command::OpenEditor if filer => self.pending_edit = self.selected_path(),
            Command::ToggleMark if filer => toggle_mark(self.current_pane_mut()),
            Command::VisualStart if filer => {
                if let Some(anchor) = self.selected_path() {
                    self.mode = Mode::Visual { anchor };
                    self.update_visual_marks();
                }
            }
            Command::Yank if filer => copy_selection(self),
            Command::Paste if filer => paste(self),
            Command::CopyAtPaths if filer => crate::ade::send_at_paths(self),
            Command::SendConfirm => crate::ade::confirm_send_target(self),
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
            // The preview hides the other pane's cursor, so diffing against
            // it would be a surprise.
            Command::Diff if filer && self.text_preview.is_some() => {
                self.notify_info("Close the preview first")
            }
            Command::Diff if filer => self.show_diff(),
            Command::Refresh if filer => self.refresh_all(),
            Command::ToggleTextPreview if filer => self.toggle_text_preview(),
            Command::ToggleMarkdown => self.toggle_markdown(),
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
                self.update_visual_marks();
            }
            Mode::Viewer {
                offset,
                doc,
                at_end,
                ..
            } => {
                *offset = if down {
                    offset.saturating_add(n)
                } else {
                    offset.saturating_sub(n)
                };
                if let Some((w, h)) = self.viewer_size {
                    *offset = text_view::clamp_offset(doc, w, h, *offset);
                }
                // At the end when one more row down would not move; only
                // counts the rows up to the screen (see `clamp_offset`).
                *at_end = down
                    && self.viewer_size.is_some_and(|(w, h)| {
                        text_view::clamp_offset(doc, w, h, offset.saturating_add(1)) == *offset
                    });
            }
            Mode::Preview => self.preview_step(n, down),
            Mode::SendTarget {
                selected, targets, ..
            } => {
                let len = targets.len().max(1);
                for _ in 0..n {
                    *selected = if down {
                        (*selected + 1) % len
                    } else {
                        (*selected + len - 1) % len
                    };
                }
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

    /// Page keys in the text viewer: scroll by `n` screens (or half
    /// screens) of the text area at the last draw, stopping at the end.
    fn scroll_pages(&mut self, n: usize, half: bool, down: bool) {
        let (Mode::Viewer { .. }, Some((_, h))) = (&self.mode, self.viewer_size) else {
            return;
        };
        let page = if half { (h / 2).max(1) } else { h.max(1) };
        self.move_by(n.saturating_mul(page as usize), down);
    }

    /// `j` / `k` in the preview: show the `n`-th next / previous image or
    /// video, moving the cursor along. Stops at the end of the list.
    fn preview_step(&mut self, n: usize, down: bool) {
        let pane = self.current_pane_mut();
        let flags = media_flags(pane);
        let (idx, steps) = step_media(&flags, pane.selected, n, down);
        if steps > 0 {
            pane.selected = idx;
            let path = pane.items[idx].path();
            let prefetch = prefetch_around(&flags, idx, down)
                .into_iter()
                .map(|i| pane.items[i].path())
                .collect();
            self.preview.show(path, prefetch);
        }
        if steps < n {
            self.notify_info(if down {
                "No next image or video"
            } else {
                "No previous image or video"
            });
        }
    }

    /// `m`: rendered / source Markdown in the text viewer, or in the text
    /// preview from the file list.
    fn toggle_markdown(&mut self) {
        let source = match &mut self.mode {
            Mode::Viewer {
                doc,
                offset,
                at_end,
                diff: false,
            } => {
                let switched = doc.toggle_markdown();
                if switched {
                    // Row offsets of the two views do not correspond.
                    *offset = 0;
                    *at_end = false;
                }
                switched.then(|| doc.shows_markdown_source())
            }
            Mode::Filer => self.text_preview.as_mut().and_then(|p| p.toggle_markdown()),
            _ => return,
        };
        match source {
            Some(true) => self.notify_info("Markdown: source"),
            Some(false) => self.notify_info("Markdown: rendered"),
            None => self.notify_info("No Markdown shown"),
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
                    doc: TextDoc::diff(diff::title(&plan), &content),
                    offset: 0,
                    at_end: false,
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
    use crate::text_view::{PreviewContent, Renderer};
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
    fn open_editor_requests_the_entry_under_the_cursor() {
        let tmp = abcd();
        fs::create_dir(tmp.path().join("dir")).unwrap();
        let mut app = app_in(tmp.path());
        // Directories are listed first: the cursor starts on "dir", which
        // is opened in the editor too (e.g. Vim's netrw).
        assert!(app.apply(Command::OpenEditor).is_continue());
        assert_eq!(app.pending_edit, Some(tmp.path().join("dir")));
        app.pending_edit = None;
        let _ = app.apply(Command::MoveDown(1));
        let _ = app.apply(Command::OpenEditor);
        assert_eq!(app.pending_edit, Some(tmp.path().join("a")));
    }

    #[test]
    fn open_editor_is_ignored_outside_the_file_list() {
        let tmp = abcd();
        let mut app = app_in(tmp.path());
        let _ = app.apply(Command::SearchStart);
        let _ = app.apply(Command::OpenEditor);
        assert_eq!(app.pending_edit, None);
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
        assert!(matches!(&app.mode, Mode::Visual { anchor } if *anchor == tmp.path().join("b")));
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
        // Before the first draw the size is unknown: the draw clamps.
        let _ = app.apply(Command::GoBottom);
        assert!(matches!(
            app.mode,
            Mode::Viewer {
                offset: usize::MAX,
                ..
            }
        ));
        // 5 rows on a screen of 3: the last page starts at row 2.
        app.viewer_size = Some((20, 3));
        let _ = app.apply(Command::GoBottom);
        assert!(matches!(app.mode, Mode::Viewer { offset: 2, .. }));
        let _ = app.apply(Command::GoTop);
        assert!(matches!(app.mode, Mode::Viewer { offset: 0, .. }));
        let _ = app.apply(Command::Back);
        assert!(matches!(app.mode, Mode::Filer));
    }

    #[test]
    fn viewer_scrolls_past_65535_lines() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("big.txt"), "x\n".repeat(70_000)).unwrap();
        let mut app = app_in(tmp.path());
        let _ = app.apply(Command::Open);
        app.viewer_size = Some((20, 1));
        let _ = app.apply(Command::GoBottom);
        assert!(matches!(app.mode, Mode::Viewer { offset: 69_999, .. }));
        let _ = app.apply(Command::MoveUp(10));
        assert!(matches!(app.mode, Mode::Viewer { offset: 69_989, .. }));
        let _ = app.apply(Command::MoveDown(10));
        assert!(matches!(app.mode, Mode::Viewer { offset: 69_999, .. }));
    }

    fn viewer_offset(app: &App) -> usize {
        match &app.mode {
            Mode::Viewer { offset, .. } => *offset,
            _ => panic!("not in the viewer"),
        }
    }

    #[test]
    fn viewer_go_bottom_and_scrolling_stop_at_the_last_wrapped_row() {
        let tmp = tempfile::tempdir().unwrap();
        // One Markdown paragraph (one document line) of many rows.
        let text = format!("# T\n\n{}\n", "日本語の段落を折り返す。".repeat(10));
        fs::write(tmp.path().join("p.md"), &text).unwrap();
        let mut app = app_in(tmp.path());
        let _ = app.apply(Command::Open);
        app.viewer_size = Some((22, 4));
        let max = text_view::max_offset(viewer_doc(&app), 22, 4);
        assert!(max >= viewer_doc(&app).line_count(), "{max}");
        let _ = app.apply(Command::GoBottom);
        assert_eq!(viewer_offset(&app), max);
        // `j` and a count (`5j`) do not go past the end.
        let _ = app.apply(Command::MoveDown(1));
        assert_eq!(viewer_offset(&app), max);
        let _ = app.apply(Command::GoTop);
        let _ = app.apply(Command::MoveDown(1).with_count(1000));
        assert_eq!(viewer_offset(&app), max);
        // So `k` moves up right away.
        let _ = app.apply(Command::MoveUp(1));
        assert_eq!(viewer_offset(&app), max - 1);
    }

    fn viewer_at_end(app: &App) -> bool {
        match &app.mode {
            Mode::Viewer { at_end, .. } => *at_end,
            _ => panic!("not in the viewer"),
        }
    }

    #[test]
    fn viewer_pages_stop_at_the_end() {
        let tmp = tempfile::tempdir().unwrap();
        let text: String = (1..=20).map(|i| format!("{i}\n")).collect();
        fs::write(tmp.path().join("t.txt"), text).unwrap();
        let mut app = app_in(tmp.path());
        let _ = app.apply(Command::Open);
        // Before the first draw the page size is unknown.
        let _ = app.apply(Command::PageDown(1));
        assert_eq!(viewer_offset(&app), 0);
        // 20 rows on a screen of 6: the last page starts at row 14.
        app.viewer_size = Some((20, 6));
        let _ = app.apply(Command::PageDown(1));
        assert_eq!(viewer_offset(&app), 6);
        let _ = app.apply(Command::HalfPageDown(1));
        assert_eq!(viewer_offset(&app), 9);
        let _ = app.apply(Command::HalfPageUp(1));
        assert_eq!(viewer_offset(&app), 6);
        let _ = app.apply(Command::PageUp(1));
        assert_eq!(viewer_offset(&app), 0);
        let _ = app.apply(Command::PageDown(2));
        assert_eq!(viewer_offset(&app), 12);
        assert!(!viewer_at_end(&app));
        let _ = app.apply(Command::PageDown(1));
        assert_eq!(viewer_offset(&app), 14);
        assert!(viewer_at_end(&app));
        let _ = app.apply(Command::PageUp(5));
        assert_eq!(viewer_offset(&app), 0);
        // Half a screen of 1 row is still one row.
        app.viewer_size = Some((20, 1));
        let _ = app.apply(Command::HalfPageDown(1));
        assert_eq!(viewer_offset(&app), 1);
    }

    #[test]
    fn viewer_remembers_being_at_the_end_until_moving_up() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("t.txt"), "1\n2\n3\n4\n5\n").unwrap();
        let mut app = app_in(tmp.path());
        let _ = app.apply(Command::Open);
        app.viewer_size = Some((20, 3));
        assert!(!viewer_at_end(&app));
        let _ = app.apply(Command::GoBottom);
        assert!(viewer_at_end(&app));
        // `j` at the end stays at the end.
        let _ = app.apply(Command::MoveDown(1));
        assert!(viewer_at_end(&app));
        for up in [Command::MoveUp(1), Command::PageUp(1), Command::GoTop] {
            let _ = app.apply(Command::GoBottom);
            let _ = app.apply(up);
            assert!(!viewer_at_end(&app), "{up:?}");
        }
        // `j` reaching the end sets it; one row short does not.
        let _ = app.apply(Command::MoveDown(1));
        assert!(!viewer_at_end(&app));
        let _ = app.apply(Command::MoveDown(1));
        assert!(viewer_at_end(&app));
        // Switching to the Markdown source restarts at the top.
        let _ = app.apply(Command::Back);
        fs::write(tmp.path().join("t.md"), "a\n\nb\n\nc\n\nd\n").unwrap();
        let _ = app.apply(Command::Refresh);
        app.current_pane_mut().selected = 0;
        let _ = app.apply(Command::Open);
        let _ = app.apply(Command::GoBottom);
        let _ = app.apply(Command::ToggleMarkdown);
        assert!(!viewer_at_end(&app));
    }

    #[test]
    fn viewer_highlights_known_languages() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("m.rs"), "fn main() {}\n").unwrap();
        let mut app = app_in(tmp.path());
        let _ = app.apply(Command::Open);
        match &app.mode {
            Mode::Viewer { doc, diff, .. } => {
                assert!(!diff);
                assert_eq!(doc.title, "m.rs");
                assert_eq!(doc.renderer, Renderer::Syntax("Rust".into()));
            }
            _ => panic!("not in the viewer"),
        }
    }

    fn viewer_doc(app: &App) -> &crate::text_view::TextDoc {
        match &app.mode {
            Mode::Viewer { doc, .. } => doc,
            _ => panic!("not in the viewer"),
        }
    }

    #[test]
    fn viewer_switches_markdown_between_rendered_and_source() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("r.md"), "# Title\n\n- a\n- b\n").unwrap();
        let mut app = app_in(tmp.path());
        let _ = app.apply(Command::Open);
        assert_eq!(viewer_doc(&app).renderer, Renderer::Markdown);
        assert_eq!(viewer_doc(&app).lines[2].to_string(), "• a");
        let _ = app.apply(Command::MoveDown(2));
        let _ = app.apply(Command::ToggleMarkdown);
        assert_eq!(message(&app), "Markdown: source");
        assert!(matches!(app.mode, Mode::Viewer { offset: 0, .. }));
        let doc = viewer_doc(&app);
        assert_eq!(doc.renderer, Renderer::Syntax("Markdown".into()));
        assert_eq!(doc.lines[2].to_string(), "- a");
        let _ = app.apply(Command::ToggleMarkdown);
        assert_eq!(message(&app), "Markdown: rendered");
        assert_eq!(viewer_doc(&app).renderer, Renderer::Markdown);
    }

    #[test]
    fn toggle_markdown_reports_when_there_is_no_markdown() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("m.rs"), "fn main() {}\n").unwrap();
        let mut app = app_in(tmp.path());
        // File list without the preview.
        let _ = app.apply(Command::ToggleMarkdown);
        assert_eq!(message(&app), "No Markdown shown");
        app.message = None;
        let _ = app.apply(Command::Open);
        let _ = app.apply(Command::ToggleMarkdown);
        assert_eq!(message(&app), "No Markdown shown");
        assert_eq!(viewer_doc(&app).renderer, Renderer::Syntax("Rust".into()));
    }

    #[test]
    fn text_preview_keeps_the_markdown_view_across_files() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("a.md"), "- a\n").unwrap();
        fs::write(tmp.path().join("b.txt"), "b\n").unwrap();
        fs::write(tmp.path().join("c.markdown"), "- c\n").unwrap();
        let mut app = app_in(tmp.path());
        let first_line = |app: &App| match app.text_preview.as_ref().map(|p| &p.content) {
            Some(PreviewContent::Text(doc)) => doc.lines[0].to_string(),
            _ => panic!("no text preview"),
        };
        let _ = app.apply(Command::ToggleTextPreview);
        assert_eq!(first_line(&app), "• a");
        let _ = app.apply(Command::ToggleMarkdown);
        assert_eq!(message(&app), "Markdown: source");
        assert_eq!(first_line(&app), "- a");
        // A plain text file has nothing to switch.
        let _ = app.apply(Command::MoveDown(1));
        app.sync_text_preview();
        let _ = app.apply(Command::ToggleMarkdown);
        assert_eq!(message(&app), "No Markdown shown");
        assert_eq!(first_line(&app), "b");
        // The next Markdown file opens as source too.
        let _ = app.apply(Command::MoveDown(1));
        app.sync_text_preview();
        assert_eq!(first_line(&app), "- c");
        let _ = app.apply(Command::ToggleMarkdown);
        assert_eq!(first_line(&app), "• c");
    }

    #[test]
    fn text_preview_toggles_and_follows_the_cursor() {
        let tmp = abcd();
        let mut app = app_in(tmp.path());
        let preview_title = |app: &App| match app.text_preview.as_ref().map(|p| &p.content) {
            Some(PreviewContent::Text(doc)) => doc.title.clone(),
            _ => panic!("no text preview"),
        };
        let _ = app.apply(Command::ToggleTextPreview);
        assert!(matches!(app.mode, Mode::Filer));
        assert_eq!(preview_title(&app), "a");
        let _ = app.apply(Command::MoveDown(2));
        app.sync_text_preview(); // done before every draw
        assert_eq!(preview_title(&app), "c");
        let _ = app.apply(Command::ToggleTextPreview);
        assert!(app.text_preview.is_none());
        // D needs the other pane's cursor, which the preview hides.
        let _ = app.apply(Command::ToggleTextPreview);
        let _ = app.apply(Command::Diff);
        assert!(matches!(app.mode, Mode::Filer));
        assert_eq!(message(&app), "Close the preview first");
        let _ = app.apply(Command::ToggleTextPreview);
        // Only from the file list.
        let _ = app.apply(Command::Open);
        let _ = app.apply(Command::ToggleTextPreview);
        assert!(app.text_preview.is_none());
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

    #[test]
    fn copy_at_paths_in_herdr_core_uses_the_clipboard() {
        let tmp = abcd();
        let mut app = app_in(tmp.path());
        app.ade = crate::ade::Ade::HerdrCore {
            terminal_id: "t7".into(),
        };
        let _ = app.apply(Command::CopyAtPaths);
        assert_eq!(last_copy().as_deref(), Some("@a"));
    }

    #[cfg(unix)]
    mod herdr {
        use super::*;
        use crate::ade::tests::FakeHerdr;
        use crate::ade::Ade;
        use serde_json::{json, Value};

        fn sent_texts(fake: &FakeHerdr) -> Vec<Value> {
            fake.requests
                .lock()
                .unwrap()
                .iter()
                .filter(|r| r["method"] == "pane.send_text")
                .map(|r| r["params"].clone())
                .collect()
        }

        fn agent(pane: &str, cwd: &Path) -> Value {
            json!({"pane_id": pane, "workspace_id": "w1", "tab_id": "w1:t1",
                   "agent": "claude", "cwd": cwd})
        }

        #[test]
        fn sends_marked_entries_to_the_only_agent() {
            let tmp = abcd();
            let fake = FakeHerdr::with_agents(json!([
                // kura's own pane is never a target.
                agent("w1:p1", tmp.path()),
                agent("w1:p2", tmp.path()),
            ]));
            let mut app = app_in(tmp.path());
            app.ade = Ade::Herdr(fake.env());
            let _ = app.apply(Command::MoveDown(1));
            let _ = app.apply(Command::ToggleMark);
            let _ = app.apply(Command::MoveDown(1));
            let _ = app.apply(Command::ToggleMark);
            let _ = app.apply(Command::CopyAtPaths);
            assert_eq!(
                sent_texts(&fake),
                [json!({"pane_id": "w1:p2", "text": "@b @c "})]
            );
            assert!(marked(&app).is_empty());
            assert_eq!(
                app.message.as_ref().unwrap().text,
                "Sent 2 path(s) to claude"
            );
            assert_eq!(last_copy(), None);
        }

        #[test]
        fn paths_are_relative_to_the_agent_cwd() {
            let tmp = abcd();
            fs::create_dir(tmp.path().join("sub")).unwrap();
            let fake = FakeHerdr::with_agents(json!([agent("w1:p2", &tmp.path().join("sub"))]));
            let mut app = app_in(tmp.path());
            app.ade = Ade::Herdr(fake.env());
            while selected_name(&app) != "a" {
                let _ = app.apply(Command::MoveDown(1));
            }
            let _ = app.apply(Command::CopyAtPaths);
            // `a` is outside the agent's directory, so it stays absolute.
            let text = sent_texts(&fake)[0]["text"].as_str().unwrap().to_string();
            assert_eq!(text, format!("@{} ", tmp.path().join("a").display()));
        }

        #[test]
        fn several_agents_open_the_target_dialog() {
            let tmp = abcd();
            let fake = FakeHerdr::with_agents(json!([
                agent("w1:p2", tmp.path()),
                agent("w1:p3", tmp.path()),
            ]));
            let mut app = app_in(tmp.path());
            app.ade = Ade::Herdr(fake.env());
            let _ = app.apply(Command::CopyAtPaths);
            assert!(matches!(app.mode, Mode::SendTarget { selected: 0, .. }));
            assert!(sent_texts(&fake).is_empty());
            // Wraps around like the sort menu.
            let _ = app.apply(Command::MoveDown(3));
            assert!(matches!(app.mode, Mode::SendTarget { selected: 1, .. }));
            let _ = app.apply(Command::SendConfirm);
            assert!(matches!(app.mode, Mode::Filer));
            assert_eq!(
                sent_texts(&fake),
                [json!({"pane_id": "w1:p3", "text": "@a "})]
            );
        }

        #[test]
        fn target_dialog_cancel_sends_nothing() {
            let tmp = abcd();
            let fake = FakeHerdr::with_agents(json!([
                agent("w1:p2", tmp.path()),
                agent("w1:p3", tmp.path()),
            ]));
            let mut app = app_in(tmp.path());
            app.ade = Ade::Herdr(fake.env());
            let _ = app.apply(Command::CopyAtPaths);
            let _ = app.apply(Command::Back);
            assert!(matches!(app.mode, Mode::Filer));
            let _ = app.apply(Command::SendConfirm);
            assert!(sent_texts(&fake).is_empty());
        }

        #[test]
        fn q_in_the_target_dialog_cancels_without_quitting() {
            use crossterm::event::{KeyCode, KeyEvent};
            let tmp = abcd();
            let fake = FakeHerdr::with_agents(json!([
                agent("w1:p2", tmp.path()),
                agent("w1:p3", tmp.path()),
            ]));
            let mut app = app_in(tmp.path());
            app.ade = Ade::Herdr(fake.env());
            let _ = app.apply(Command::ToggleMark);
            let _ = app.apply(Command::CopyAtPaths);
            let cmd = crate::input::KeyState::default()
                .translate(&app.mode, KeyEvent::from(KeyCode::Char('q')))
                .unwrap();
            assert!(app.apply(cmd).is_continue());
            assert!(matches!(app.mode, Mode::Filer));
            assert!(sent_texts(&fake).is_empty());
            assert_eq!(marked(&app), [0]);
        }

        #[test]
        fn no_agent_falls_back_to_the_clipboard() {
            let tmp = abcd();
            let fake = FakeHerdr::with_agents(json!([agent("w1:p1", tmp.path())]));
            let mut app = app_in(tmp.path());
            app.ade = Ade::Herdr(fake.env());
            let _ = app.apply(Command::CopyAtPaths);
            assert_eq!(last_copy().as_deref(), Some("@a"));
            assert_eq!(
                app.message.as_ref().unwrap().text,
                "No agent pane found; copied 1 path(s)"
            );
        }

        #[test]
        fn unreachable_herdr_reports_an_error() {
            let tmp = abcd();
            let mut app = app_in(tmp.path());
            let _ = app.apply(Command::ToggleMark);
            app.ade = Ade::Herdr(crate::ade::HerdrEnv {
                socket: tmp.path().join("missing.sock"),
                pane_id: None,
                workspace_id: None,
                tab_id: None,
            });
            let _ = app.apply(Command::CopyAtPaths);
            let msg = app.message.as_ref().unwrap();
            assert_eq!(msg.level, MessageLevel::Error);
            assert!(msg.text.starts_with("Send failed: herdr: "), "{}", msg.text);
            assert_eq!(last_copy(), None);
            // The marks stay for another try.
            assert_eq!(marked(&app), [0]);
        }

        #[test]
        fn failed_send_keeps_the_marks() {
            let tmp = abcd();
            let fake = FakeHerdr::start(|req| {
                let id = req["id"].clone();
                Some(match req["method"].as_str() {
                    Some("agent.list") => json!({"id": id, "result": {"type": "agent_list",
                        "agents": [{"pane_id": "w1:p2", "workspace_id": "w1", "tab_id": "w1:t1"}]}}),
                    _ => json!({"id": id, "error": {"code": "not_found", "message": "pane gone"}}),
                })
            });
            let mut app = app_in(tmp.path());
            app.ade = Ade::Herdr(fake.env());
            let _ = app.apply(Command::ToggleMark);
            let _ = app.apply(Command::CopyAtPaths);
            assert_eq!(
                app.message.as_ref().unwrap().text,
                "Send failed: herdr: pane gone"
            );
            assert_eq!(marked(&app), [0]);
        }
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

    fn changed(paths: &[std::path::PathBuf]) -> crate::watch::FsEvent {
        crate::watch::FsEvent::Changed(paths.to_vec())
    }

    #[test]
    fn fs_event_refreshes_the_pane_keeping_cursor_and_marks() {
        let tmp = abcd();
        let mut app = app_in(tmp.path());
        let _ = app.apply(Command::ToggleMark); // a
        let _ = app.apply(Command::MoveDown(2)); // c
        fs::write(tmp.path().join("0new"), "").unwrap();

        // A change elsewhere (or deeper down) does not re-read the listing.
        app.handle_fs_event(&changed(&[tmp.path().join("sub/x")]));
        assert_eq!(names(&app), ["a", "b", "c", "d"]);

        app.handle_fs_event(&changed(&[tmp.path().join("0new")]));
        assert_eq!(names(&app), ["0new", "a", "b", "c", "d"]);
        assert_eq!(names_of(&app.right), names(&app)); // same directory
        assert_eq!(selected_name(&app), "c");
        assert_eq!(marked(&app), vec![1]);
        assert!(app.message.is_none());
    }

    fn names_of(pane: &crate::app::Pane) -> Vec<String> {
        pane.items
            .iter()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn fs_event_refreshes_only_the_pane_showing_the_directory() {
        let tmp = abcd();
        fs::create_dir(tmp.path().join("sub")).unwrap();
        let mut app = app_in(tmp.path());
        app.right.change_dir(tmp.path().join("sub")).unwrap();
        fs::write(tmp.path().join("sub/s"), "").unwrap();
        fs::write(tmp.path().join("e"), "").unwrap();

        app.handle_fs_event(&changed(&[tmp.path().join("sub/s")]));
        assert_eq!(names_of(&app.right), ["s"]);
        assert_eq!(names_of(&app.left), ["sub", "a", "b", "c", "d"]);

        app.handle_fs_event(&crate::watch::FsEvent::Rescan);
        assert_eq!(names_of(&app.left), ["sub", "a", "b", "c", "d", "e"]);
    }

    #[test]
    fn fs_event_for_a_removed_directory_reports_an_error() {
        let tmp = abcd();
        fs::create_dir(tmp.path().join("sub")).unwrap();
        let mut app = app_in(tmp.path());
        app.right.change_dir(tmp.path().join("sub")).unwrap();
        assert_eq!(app.watch_targets().len(), 2);
        fs::remove_dir(tmp.path().join("sub")).unwrap();
        assert_eq!(app.watch_targets(), vec![tmp.path().to_path_buf()]);
        app.handle_fs_event(&changed(&[tmp.path().join("sub")]));
        let msg = app.message.as_ref().unwrap();
        assert_eq!(msg.level, MessageLevel::Error);
        assert!(msg.text.ends_with(": sub"), "{}", msg.text);

        app.handle_fs_event(&crate::watch::FsEvent::Error("boom".into()));
        assert_eq!(app.message.as_ref().unwrap().text, "Watch error: boom");
    }

    #[test]
    fn visual_range_follows_paths_across_refresh() {
        let tmp = abcd();
        let mut app = app_in(tmp.path());
        let _ = app.apply(Command::MoveDown(1)); // b
        let _ = app.apply(Command::VisualStart);
        let _ = app.apply(Command::MoveDown(1)); // c
        assert_eq!(marked(&app), vec![1, 2]);

        // Entries appear before the anchor and inside the range.
        fs::write(tmp.path().join("0"), "").unwrap();
        fs::write(tmp.path().join("bb"), "").unwrap();
        app.handle_fs_event(&changed(&[tmp.path().join("0"), tmp.path().join("bb")]));
        assert_eq!(names(&app), ["0", "a", "b", "bb", "c", "d"]);
        assert_eq!(selected_name(&app), "c");
        assert_eq!(marked(&app), vec![2, 3, 4]); // b, bb, c
        let _ = app.apply(Command::MoveDown(1)); // d
        assert_eq!(marked(&app), vec![2, 3, 4, 5]);
    }

    #[test]
    fn visual_restarts_at_the_cursor_when_the_anchor_is_removed() {
        let tmp = abcd();
        let mut app = app_in(tmp.path());
        let _ = app.apply(Command::VisualStart); // a
        let _ = app.apply(Command::MoveDown(2)); // c
        fs::remove_file(tmp.path().join("a")).unwrap();
        app.handle_fs_event(&changed(&[tmp.path().join("a")]));
        assert_eq!(selected_name(&app), "c");
        assert_eq!(marked(&app), vec![1]);
        assert!(matches!(&app.mode, Mode::Visual { anchor } if *anchor == tmp.path().join("c")));
        let _ = app.apply(Command::MoveUp(1)); // b
        assert_eq!(marked(&app), vec![0, 1]);
    }

    #[test]
    fn rename_is_cancelled_when_the_file_is_removed_from_outside() {
        let tmp = abcd();
        let mut app = app_in(tmp.path());
        let _ = app.apply(Command::MoveDown(1)); // b
        let _ = app.apply(Command::RenameStart);
        let _ = app.apply(Command::InsertChar('z'));
        fs::remove_file(tmp.path().join("b")).unwrap();
        app.handle_fs_event(&changed(&[tmp.path().join("b")]));
        assert!(matches!(app.mode, Mode::Filer));
        assert_eq!(
            app.message.as_ref().unwrap().text,
            "Rename cancelled: b is gone"
        );
        // A late Enter must not rename the entry now under the cursor.
        let _ = app.apply(Command::RenameCommit);
        assert_eq!(names(&app), ["a", "c", "d"]);
    }

    #[test]
    fn rename_continues_when_other_files_change() {
        let tmp = abcd();
        let mut app = app_in(tmp.path());
        let _ = app.apply(Command::MoveDown(1)); // b
        let _ = app.apply(Command::RenameStart);
        fs::write(tmp.path().join("0"), "").unwrap();
        app.handle_fs_event(&changed(&[tmp.path().join("0")]));
        let _ = app.apply(Command::InsertChar('z'));
        let _ = app.apply(Command::RenameCommit);
        assert_eq!(names(&app), ["0", "a", "bz", "c", "d"]);
    }

    #[test]
    fn refresh_rereads_both_panes_keeping_cursor_and_marks() {
        let tmp = abcd();
        fs::create_dir(tmp.path().join("sub")).unwrap();
        let mut app = app_in(tmp.path());
        app.right.change_dir(tmp.path().join("sub")).unwrap();
        let _ = app.apply(Command::MoveDown(1)); // a
        let _ = app.apply(Command::ToggleMark);
        let _ = app.apply(Command::MoveDown(2)); // c
        fs::write(tmp.path().join("0"), "").unwrap();
        fs::write(tmp.path().join("sub/s"), "").unwrap();

        let _ = app.apply(Command::Refresh);
        assert_eq!(names(&app), ["sub", "0", "a", "b", "c", "d"]);
        assert_eq!(names_of(&app.right), ["s"]);
        assert_eq!(selected_name(&app), "c");
        assert_eq!(marked(&app), vec![2]);
        assert_eq!(app.message.as_ref().unwrap().text, "Refreshed");
    }

    #[test]
    fn refresh_reports_a_removed_directory_and_is_filer_only() {
        let tmp = abcd();
        fs::create_dir(tmp.path().join("sub")).unwrap();
        let mut app = app_in(tmp.path());
        app.right.change_dir(tmp.path().join("sub")).unwrap();
        fs::remove_dir(tmp.path().join("sub")).unwrap();
        let _ = app.apply(Command::Refresh);
        let msg = app.message.as_ref().unwrap();
        assert_eq!(msg.level, MessageLevel::Error);
        assert!(msg.text.ends_with(": sub"), "{}", msg.text);

        app.message = None;
        fs::write(tmp.path().join("e"), "").unwrap();
        let _ = app.apply(Command::SortMenu);
        let _ = app.apply(Command::Refresh);
        assert!(app.message.is_none());
        assert_eq!(names_of(&app.left), ["a", "b", "c", "d"]); // no "e"
    }

    #[test]
    fn fs_event_keeps_the_hidden_filter() {
        let tmp = abcd();
        let mut app = app_in(tmp.path());
        let _ = app.apply(Command::ToggleHidden);
        fs::write(tmp.path().join(".dot"), "").unwrap();
        fs::write(tmp.path().join("e"), "").unwrap();
        app.handle_fs_event(&changed(&[tmp.path().join(".dot"), tmp.path().join("e")]));
        assert_eq!(names(&app), ["a", "b", "c", "d", "e"]);
    }

    #[test]
    fn fs_event_rereads_git_status() {
        let tmp = abcd();
        if !git(tmp.path(), &["init", "-q", "-b", "main"]) {
            return;
        }
        assert!(git(tmp.path(), &["add", "."]));
        assert!(git(tmp.path(), &["commit", "-qm", "init"]));
        let mut app = app_in(tmp.path());
        let status = |app: &App| {
            let pane = app.current_pane();
            let e = pane.items.iter().find(|e| e.file_name() == "c").unwrap();
            pane.git_status_of(e)
        };
        assert_eq!(status(&app), None);
        fs::write(tmp.path().join("c"), "changed").unwrap();
        app.handle_fs_event(&changed(&[tmp.path().join("c")]));
        assert_eq!(status(&app), Some(crate::git::FileStatus::Modified));
    }

    #[test]
    fn new_files_schedule_a_highlight_expiry() {
        let tmp = abcd();
        let mut app = app_in(tmp.path());
        let now = std::time::SystemTime::now();
        // Files from before the session are never highlighted.
        app.session_start = now + std::time::Duration::from_secs(3600);
        assert_eq!(app.next_highlight_expiry(now), None);
        app.session_start = now - std::time::Duration::from_secs(3600);
        let expiry = app.next_highlight_expiry(now).expect("just written");
        assert!(expiry <= crate::watch::RECENT_WINDOW);
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

    /// Text of the diff viewer, one `\n`-terminated line per row.
    fn diff_content(app: &App) -> String {
        match &app.mode {
            Mode::Viewer {
                doc, diff: true, ..
            } => doc.lines.iter().map(|l| format!("{l}\n")).collect(),
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

    /// A dir with images, videos, text files and a folder named like an image.
    fn media_dir() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir(tmp.path().join("dir.png")).unwrap();
        for name in ["a.png", "b.txt", "c.mp4", "d.md", "e.jpg", "f.txt"] {
            fs::write(tmp.path().join(name), name).unwrap();
        }
        tmp
    }

    /// Open the preview on `name` (the list is sorted, dirs first).
    fn preview_on(app: &mut App, name: &str) {
        while selected_name(app) != name {
            let _ = app.apply(Command::MoveDown(1));
        }
        let _ = app.apply(Command::Open);
        assert!(matches!(app.mode, Mode::Preview));
    }

    fn shown(app: &App) -> String {
        let p = app.preview.current().unwrap();
        p.file_name().unwrap().to_string_lossy().into_owned()
    }

    fn wanted(app: &App) -> Vec<String> {
        app.preview
            .wanted()
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn open_preview_prefetches_the_neighbours() {
        let tmp = media_dir();
        let mut app = app_in(tmp.path());
        preview_on(&mut app, "c.mp4");
        assert_eq!(wanted(&app), ["c.mp4", "e.jpg", "a.png"]);
    }

    #[test]
    fn preview_j_k_step_over_non_media_and_move_the_cursor() {
        let tmp = media_dir();
        let mut app = app_in(tmp.path());
        preview_on(&mut app, "a.png");
        let _ = app.apply(Command::MoveDown(1));
        assert!(matches!(app.mode, Mode::Preview));
        assert_eq!(selected_name(&app), "c.mp4");
        assert_eq!(shown(&app), "c.mp4");
        let _ = app.apply(Command::MoveDown(1));
        assert_eq!(selected_name(&app), "e.jpg");
        assert_eq!(shown(&app), "e.jpg");
        // Going back prefetches in the direction of travel first.
        let _ = app.apply(Command::MoveUp(1));
        assert_eq!(wanted(&app), ["c.mp4", "a.png", "e.jpg"]);
        assert!(app.message.is_none());
    }

    #[test]
    fn preview_count_stops_at_the_end_and_reports_it() {
        let tmp = media_dir();
        let mut app = app_in(tmp.path());
        preview_on(&mut app, "a.png");
        let _ = app.apply(Command::MoveDown(5));
        assert_eq!(selected_name(&app), "e.jpg");
        assert_eq!(shown(&app), "e.jpg");
        assert_eq!(message(&app), "No next image or video");

        app.message = None;
        let _ = app.apply(Command::MoveDown(1));
        assert_eq!(selected_name(&app), "e.jpg");
        assert_eq!(message(&app), "No next image or video");
        assert!(matches!(app.mode, Mode::Preview));

        let _ = app.apply(Command::MoveUp(2));
        assert_eq!(selected_name(&app), "a.png");
        app.message = None;
        let _ = app.apply(Command::MoveUp(1));
        assert_eq!(selected_name(&app), "a.png");
        assert_eq!(message(&app), "No previous image or video");
    }

    #[test]
    fn preview_closes_with_back() {
        let tmp = media_dir();
        let mut app = app_in(tmp.path());
        preview_on(&mut app, "e.jpg");
        let _ = app.apply(Command::Back);
        assert!(matches!(app.mode, Mode::Filer));
        assert_eq!(app.preview.current(), None);
        assert_eq!(selected_name(&app), "e.jpg");
    }

    /// Move the cursor onto `name` with `P` on (syncing as each draw does).
    fn cursor_to(app: &mut App, name: &str) {
        while selected_name(app) != name {
            let _ = app.apply(Command::MoveDown(1));
            app.sync_text_preview();
        }
    }

    fn pane_text_title(app: &App) -> Option<String> {
        match app.text_preview.as_ref().map(|p| &p.content) {
            Some(PreviewContent::Text(doc)) => Some(doc.title.clone()),
            _ => None,
        }
    }

    #[test]
    fn pane_preview_shows_images_and_videos_and_prefetches_around_the_cursor() {
        let tmp = media_dir();
        let mut app = app_in(tmp.path());
        let _ = app.apply(Command::ToggleTextPreview);
        cursor_to(&mut app, "c.mp4");
        assert!(app.pane_preview_shows_media());
        assert!(matches!(app.mode, Mode::Filer));
        assert_eq!(wanted(&app), ["c.mp4", "e.jpg", "a.png"]);
        cursor_to(&mut app, "e.jpg");
        assert_eq!(shown(&app), "e.jpg");
    }

    #[test]
    fn pane_preview_drops_the_image_when_the_cursor_leaves_it() {
        let tmp = media_dir();
        let mut app = app_in(tmp.path());
        let _ = app.apply(Command::ToggleTextPreview);
        // dir.png is a folder: not an image.
        assert!(!app.pane_preview_shows_media());
        cursor_to(&mut app, "a.png");
        assert!(app.pane_preview_shows_media());
        assert_eq!(shown(&app), "a.png");
        cursor_to(&mut app, "b.txt");
        assert!(!app.pane_preview_shows_media());
        assert_eq!(app.preview.current(), None);
        assert!(!app.preview.busy());
        assert_eq!(pane_text_title(&app).as_deref(), Some("b.txt"));
        cursor_to(&mut app, "c.mp4");
        assert_eq!(shown(&app), "c.mp4");
        cursor_to(&mut app, "d.md");
        assert_eq!(app.preview.current(), None);
        assert_eq!(pane_text_title(&app).as_deref(), Some("d.md"));
    }

    #[test]
    fn gallery_from_the_pane_preview_returns_to_it() {
        let tmp = media_dir();
        let mut app = app_in(tmp.path());
        let _ = app.apply(Command::ToggleTextPreview);
        cursor_to(&mut app, "a.png");
        let _ = app.apply(Command::Open);
        assert!(matches!(app.mode, Mode::Preview));
        let _ = app.apply(Command::MoveDown(1));
        app.sync_text_preview();
        assert_eq!(shown(&app), "c.mp4");
        let _ = app.apply(Command::Back);
        assert!(matches!(app.mode, Mode::Filer));
        app.sync_text_preview();
        // The pane preview goes on showing the entry under the cursor.
        assert!(app.pane_preview_shows_media());
        assert_eq!(shown(&app), "c.mp4");
        let _ = app.apply(Command::ToggleTextPreview);
        assert!(app.text_preview.is_none());
        assert_eq!(app.preview.current(), None);
    }

    #[test]
    fn folder_named_like_an_image_is_not_previewed() {
        let tmp = media_dir();
        let mut app = app_in(tmp.path());
        assert_eq!(selected_name(&app), "dir.png");
        let _ = app.apply(Command::Open);
        assert!(matches!(app.mode, Mode::Filer));
        assert_eq!(app.current_pane().current_dir, tmp.path().join("dir.png"));
    }
}
