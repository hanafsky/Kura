use std::{
    collections::HashSet,
    fs, io,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

use ratatui_image::picker::Picker;

use crate::fs_utils::{sort_entries, SortBy};
use crate::git::{self, FileStatus, GitStatus};
use crate::media;
use crate::mode::{Mode, PaneType};
use crate::preview::{self, Previewer};
use crate::text_view::{self, PanePreview, PreviewContent, TextDoc};
use crate::watch::{self, FsEvent};

pub struct Pane {
    pub items: Vec<fs::DirEntry>,
    pub selected: usize,
    pub current_dir: PathBuf,
    /// Marked entries, by path, so marks survive refreshes and re-sorting.
    pub marked: HashSet<PathBuf>,
    /// Chosen sort order (`None` = by name, byte order). Kept across refreshes.
    pub sort: Option<SortBy>,
    /// List directories before files (on by default).
    pub dirs_first: bool,
    /// Cached git status of `current_dir` (`None` outside a repository).
    /// Updated only when the directory is (re)loaded.
    pub git: Option<GitStatus>,
    /// Show only entries with git changes (toggled with `M`).
    pub git_filter: bool,
    /// List hidden entries (names starting with `.`; toggled with `.`, on by default).
    pub show_hidden: bool,
    /// Hide entries ignored by git (toggled with `i`; only inside a repository).
    pub hide_ignored: bool,
}

/// Read a directory's entries in the given order. Entries that cannot be
/// read are skipped so that a single bad entry does not hide the whole listing.
fn read_entries(
    path: &Path,
    sort: Option<SortBy>,
    dirs_first: bool,
) -> io::Result<Vec<fs::DirEntry>> {
    let mut entries: Vec<fs::DirEntry> = fs::read_dir(path)?.filter_map(Result::ok).collect();
    sort_entries(&mut entries, sort, dirs_first);
    Ok(entries)
}

fn is_dir_entry(e: &fs::DirEntry) -> bool {
    e.file_type().is_ok_and(|t| t.is_dir())
}

/// Git status of a listed entry.
fn entry_status(git: Option<&GitStatus>, e: &fs::DirEntry) -> Option<FileStatus> {
    git?.lookup(&e.file_name().to_string_lossy(), is_dir_entry(e))
}

/// Which entries a pane lists. Every field is the filter actually in
/// effect (git filters are off outside a repository).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ListFilter {
    /// Only entries with git changes (`M`).
    pub changed_only: bool,
    /// Hide names starting with `.` (`.`).
    pub hide_hidden: bool,
    /// Hide entries ignored by git (`i`).
    pub hide_ignored: bool,
}

/// Whether an entry named `name` with git status `status` is listed.
pub fn is_visible(name: &str, status: Option<FileStatus>, filter: ListFilter) -> bool {
    if filter.hide_hidden && name.starts_with('.') {
        return false;
    }
    if filter.hide_ignored && status == Some(FileStatus::Ignored) {
        return false;
    }
    if filter.changed_only && !status.is_some_and(FileStatus::is_change) {
        return false;
    }
    true
}

/// Keep only the entries `filter` lets through.
fn filter_entries(
    entries: Vec<fs::DirEntry>,
    git: Option<&GitStatus>,
    filter: ListFilter,
) -> Vec<fs::DirEntry> {
    if filter == ListFilter::default() {
        return entries;
    }
    entries
        .into_iter()
        .filter(|e| {
            is_visible(
                &e.file_name().to_string_lossy(),
                entry_status(git, e),
                filter,
            )
        })
        .collect()
}

impl Pane {
    pub fn new(path: PathBuf) -> io::Result<Self> {
        let entries = read_entries(&path, None, true)?;
        let git = git::read_status(&path);
        Ok(Self {
            items: entries,
            selected: 0,
            current_dir: path,
            marked: HashSet::new(),
            sort: None,
            dirs_first: true,
            git,
            git_filter: false,
            show_hidden: true,
            hide_ignored: false,
        })
    }

    /// Path of the entry under the cursor.
    pub fn selected_path(&self) -> Option<PathBuf> {
        self.items.get(self.selected).map(|e| e.path())
    }

    /// Re-read the directory. The cursor stays on the same file (or the
    /// same row if that file is gone) and marks on files that still exist.
    pub fn refresh(&mut self) -> io::Result<()> {
        self.git = git::read_status(&self.current_dir);
        self.reload()
    }

    /// Re-read the listing using the cached git status. The cursor stays on
    /// the same file (or row) and only marks on listed entries are kept, so
    /// entries hidden by the git filter are never acted on.
    fn reload(&mut self) -> io::Result<()> {
        let cursor = self.selected_path();
        let entries = read_entries(&self.current_dir, self.sort, self.dirs_first)?;
        self.items = filter_entries(entries, self.git.as_ref(), self.list_filter());
        self.restore_cursor(cursor);
        let present: HashSet<PathBuf> = self.items.iter().map(|e| e.path()).collect();
        self.marked.retain(|p| present.contains(p));
        Ok(())
    }

    /// Git status of a listed entry (from the cached status).
    pub fn git_status_of(&self, e: &fs::DirEntry) -> Option<FileStatus> {
        entry_status(self.git.as_ref(), e)
    }

    /// Whether the changed-only filter is currently in effect.
    pub fn git_filter_active(&self) -> bool {
        self.git_filter && self.git.is_some()
    }

    /// Whether ignored entries are currently hidden. Inside an ignored
    /// directory (e.g. after entering `target/`) nothing is hidden, since
    /// every entry there is ignored.
    pub fn ignore_filter_active(&self) -> bool {
        self.hide_ignored && self.git.as_ref().is_some_and(|g| !g.in_ignored_dir())
    }

    /// The filter in effect for the current directory.
    pub fn list_filter(&self) -> ListFilter {
        ListFilter {
            changed_only: self.git_filter_active(),
            hide_hidden: !self.show_hidden,
            hide_ignored: self.ignore_filter_active(),
        }
    }

    /// Flip one filter flag and re-list using the cached git status; the
    /// flag is restored if the directory cannot be read.
    fn toggle_flag(&mut self, flag: fn(&mut Pane) -> &mut bool) -> io::Result<()> {
        *flag(self) ^= true;
        let result = self.reload();
        if result.is_err() {
            *flag(self) ^= true;
        }
        result
    }

    /// Toggle the changed-only filter using the cached git status.
    pub fn toggle_git_filter(&mut self) -> io::Result<()> {
        self.toggle_flag(|p| &mut p.git_filter)
    }

    /// Toggle listing hidden (dot) entries.
    pub fn toggle_hidden(&mut self) -> io::Result<()> {
        self.toggle_flag(|p| &mut p.show_hidden)
    }

    /// Toggle hiding git-ignored entries using the cached git status.
    pub fn toggle_ignored(&mut self) -> io::Result<()> {
        self.toggle_flag(|p| &mut p.hide_ignored)
    }

    /// Re-order the listing with the current sort settings, keeping the
    /// cursor on the same file.
    pub fn resort(&mut self) {
        let cursor = self.selected_path();
        sort_entries(&mut self.items, self.sort, self.dirs_first);
        self.restore_cursor(cursor);
    }

    /// Put the cursor on `path`, or keep the row (clamped to the list) if
    /// it is not listed.
    fn restore_cursor(&mut self, path: Option<PathBuf>) {
        let found = path.and_then(|p| self.items.iter().position(|e| e.path() == p));
        self.selected = found
            .unwrap_or(self.selected)
            .min(self.items.len().saturating_sub(1));
    }

    /// Mark the entry at `index` (no-op if out of range).
    pub fn mark_index(&mut self, index: usize) {
        if let Some(e) = self.items.get(index) {
            self.marked.insert(e.path());
        }
    }

    /// Move to `path`. The pane is only updated if the directory can be read;
    /// on error the current directory and listing are left untouched.
    pub fn change_dir(&mut self, path: PathBuf) -> io::Result<()> {
        let entries = read_entries(&path, self.sort, self.dirs_first)?;
        self.git = git::read_status(&path);
        self.items = filter_entries(entries, self.git.as_ref(), self.list_filter());
        self.current_dir = path;
        self.selected = 0;
        self.marked.clear();
        Ok(())
    }

    /// Move to the parent directory and put the cursor on the directory we
    /// came from. Does nothing at the filesystem root.
    pub fn go_parent(&mut self) -> io::Result<()> {
        let Some(parent) = self.current_dir.parent().map(Path::to_path_buf) else {
            return Ok(());
        };
        let came_from = self.current_dir.file_name().map(|n| n.to_os_string());
        self.change_dir(parent)?;
        if let Some(name) = came_from {
            if let Some(pos) = self.items.iter().position(|e| e.file_name() == name) {
                self.selected = pos;
            }
        }
        Ok(())
    }

    /// Number of marked entries.
    pub fn marked_count(&self) -> usize {
        self.marked.len()
    }
}

/// Format a filesystem error for the status line: the reason comes first and
/// only the last path component is shown, so the reason is not cut off at the
/// right edge by a long path. e.g. `Permission denied: a_locked`.
pub fn fs_error_message(err: &io::Error, path: &Path) -> String {
    let mut reason = err.to_string();
    // Drop the trailing " (os error N)" that std appends to OS errors.
    if let Some(idx) = reason.rfind(" (os error ") {
        if reason.ends_with(')') {
            reason.truncate(idx);
        }
    }
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string());
    format!("{reason}: {name}")
}

/// For each listed entry, whether it is an image or a video (folders are not).
pub fn media_flags(pane: &Pane) -> Vec<bool> {
    pane.items
        .iter()
        .map(|e| {
            let path = e.path();
            let is_dir = match e.file_type() {
                Ok(t) if t.is_symlink() => path.is_dir(),
                Ok(t) => t.is_dir(),
                Err(_) => false,
            };
            !is_dir && media::is_media(&path)
        })
        .collect()
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MessageLevel {
    Info,
    Error,
}

/// A message shown in the status line until the next key press.
pub struct Message {
    pub level: MessageLevel,
    pub text: String,
}

pub struct App {
    pub left: Pane,
    pub right: Pane,
    pub active: PaneType,
    pub mode: Mode,
    pub clipboard: Vec<PathBuf>,
    /// Image / video preview (Mode::Preview) and its background loader.
    pub preview: Previewer,
    /// Detected graphics protocol / font size (for the preview footer).
    pub graphics_info: String,
    /// Message shown in the status line (errors, result counts).
    pub message: Option<Message>,
    /// Working directory kura was started in (the agent's project root);
    /// `@path` copies are made relative to it.
    pub launch_dir: PathBuf,
    /// Query of the last search, repeated by `n` / `N`.
    pub last_search: Option<String>,
    /// File to open in `$EDITOR` once the event loop gets control back
    /// (set by `e`; the loop owns the terminal, so it runs the editor).
    pub pending_edit: Option<PathBuf>,
    /// When kura started; files changed after it are highlighted.
    pub session_start: SystemTime,
    /// Text preview of the entry under the cursor in the opposite pane
    /// (`P`); `None` while it is off.
    pub text_preview: Option<PanePreview>,
    /// Width × height of the text viewer's text area at the last draw, so
    /// that scrolling can stop at the last wrapped row (`G`, `j`). `None`
    /// before the viewer was first drawn.
    pub viewer_size: Option<(u16, u16)>,
    /// The agent terminal kura runs in (`Y` sends `@path` to its agents).
    pub ade: crate::ade::Ade,
    /// Log of the files the coding agent changed (`A`); `None` if no
    /// location could be worked out.
    pub agent_log: Option<PathBuf>,
}

impl App {
    /// Start with both panes in `dir`.
    #[cfg(test)]
    pub fn with_dir(picker: Picker, dir: PathBuf) -> io::Result<Self> {
        Self::with_panes(picker, dir.clone(), dir.clone(), dir)
    }

    /// Start with the left / right panes in the given directories.
    /// `launch_dir` is the base for `@path` copies.
    pub fn with_panes(
        picker: Picker,
        left: PathBuf,
        right: PathBuf,
        launch_dir: PathBuf,
    ) -> io::Result<Self> {
        Ok(Self {
            left: Pane::new(left)?,
            right: Pane::new(right)?,
            active: PaneType::Left,
            mode: Mode::Filer,
            clipboard: Vec::new(),
            preview: Previewer::new(picker),
            graphics_info: String::new(),
            message: None,
            launch_dir,
            last_search: None,
            pending_edit: None,
            session_start: SystemTime::now(),
            text_preview: None,
            viewer_size: None,
            ade: crate::ade::Ade::None,
            agent_log: crate::agent_changes::log_path_from_env(),
        })
    }

    /// Show an informational message in the status line.
    pub fn notify_info(&mut self, text: impl Into<String>) {
        self.message = Some(Message {
            level: MessageLevel::Info,
            text: text.into(),
        });
    }

    /// Show an error message in the status line.
    pub fn notify_error(&mut self, text: impl Into<String>) {
        self.message = Some(Message {
            level: MessageLevel::Error,
            text: text.into(),
        });
    }

    pub fn current_pane(&self) -> &Pane {
        match self.active {
            PaneType::Left => &self.left,
            PaneType::Right => &self.right,
        }
    }

    pub fn current_pane_mut(&mut self) -> &mut Pane {
        match self.active {
            PaneType::Left => &mut self.left,
            PaneType::Right => &mut self.right,
        }
    }

    pub fn switch_pane(&mut self) {
        self.active = match self.active {
            PaneType::Left => PaneType::Right,
            PaneType::Right => PaneType::Left,
        };
    }

    pub fn on_up(&mut self) {
        let pane = self.current_pane_mut();
        if pane.selected > 0 {
            pane.selected -= 1;
        }
    }

    pub fn on_down(&mut self) {
        let pane = self.current_pane_mut();
        if pane.selected + 1 < pane.items.len() {
            pane.selected += 1;
        }
    }

    pub fn on_left(&mut self) {
        let pane = self.current_pane_mut();
        if let Err(e) = pane.go_parent() {
            let target = pane
                .current_dir
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_default();
            self.notify_error(fs_error_message(&e, &target));
        }
    }

    /// Toggle the changed-only filter of the active pane (`M`).
    pub fn toggle_git_filter(&mut self) {
        let pane = self.current_pane_mut();
        if pane.git.is_none() {
            self.notify_info("Not a git repository");
            return;
        }
        if let Err(e) = pane.toggle_git_filter() {
            let dir = pane.current_dir.clone();
            self.notify_error(fs_error_message(&e, &dir));
            return;
        }
        let text = if pane.git_filter {
            "Showing changed files only"
        } else {
            "Showing all files"
        };
        self.notify_info(text);
    }

    /// Toggle listing hidden (dot) entries in the active pane (`.`).
    pub fn toggle_hidden(&mut self) {
        let pane = self.current_pane_mut();
        if let Err(e) = pane.toggle_hidden() {
            let dir = pane.current_dir.clone();
            self.notify_error(fs_error_message(&e, &dir));
            return;
        }
        let text = if pane.show_hidden {
            "Showing hidden files"
        } else {
            "Hiding hidden files"
        };
        self.notify_info(text);
    }

    /// Toggle hiding git-ignored entries in the active pane (`i`).
    pub fn toggle_ignored(&mut self) {
        let pane = self.current_pane_mut();
        if pane.git.is_none() {
            self.notify_info("Not a git repository");
            return;
        }
        if let Err(e) = pane.toggle_ignored() {
            let dir = pane.current_dir.clone();
            self.notify_error(fs_error_message(&e, &dir));
            return;
        }
        let text = if pane.hide_ignored && !pane.ignore_filter_active() {
            // Everything here is ignored, so the filter hides nothing.
            "Already inside an ignored directory"
        } else if pane.hide_ignored {
            "Hiding git-ignored files"
        } else {
            "Showing git-ignored files"
        };
        self.notify_info(text);
    }

    /// Directories the watcher should follow (both panes, once if equal).
    /// A shown directory that was removed is dropped, so the watcher does
    /// not keep reporting it (it is watched again once it exists). While
    /// the agent changes are listed (`A`), the log's directory is watched too.
    pub fn watch_targets(&self) -> Vec<PathBuf> {
        let mut targets = watch::watch_targets(&self.left.current_dir, &self.right.current_dir);
        targets.retain(|d| d.is_dir());
        if let Some(dir) = self.agent_log_watch_dir() {
            if !targets.contains(&dir) {
                targets.push(dir);
            }
        }
        targets
    }

    /// React to a change reported by the directory watcher: re-read the
    /// panes showing an affected directory (cursor, marks and the Visual
    /// range stay on the same files; the git status is re-read too).
    pub fn handle_fs_event(&mut self, event: &FsEvent) {
        let (left, right) = match event {
            FsEvent::Changed(paths) => (
                watch::affects_dir(paths, &watch::dir_keys(&self.left.current_dir)),
                watch::affects_dir(paths, &watch::dir_keys(&self.right.current_dir)),
            ),
            FsEvent::Rescan => (true, true),
            FsEvent::Error(e) => {
                self.notify_error(format!("Watch error: {e}"));
                return;
            }
        };
        let log = match event {
            FsEvent::Changed(paths) => self.touches_agent_log(paths),
            _ => true,
        };
        if log {
            self.reload_agent_changes();
        }
        self.refresh_panes(left, right);
    }

    /// Re-read both panes by hand (`R`), e.g. with `KURA_WATCH=off`.
    pub fn refresh_all(&mut self) {
        if self.refresh_panes(true, true) {
            self.notify_info("Refreshed");
        }
    }

    /// Re-read the chosen panes, keeping cursor, marks and the Visual
    /// range on the same files. Reports the first error; returns whether
    /// all succeeded.
    fn refresh_panes(&mut self, left: bool, right: bool) -> bool {
        let mut first_err = None;
        for (hit, pane) in [(left, &mut self.left), (right, &mut self.right)] {
            if hit {
                if let Err(e) = pane.refresh() {
                    first_err.get_or_insert_with(|| fs_error_message(&e, &pane.current_dir));
                }
            }
        }
        let ok = first_err.is_none();
        if let Some(msg) = first_err {
            self.notify_error(msg);
        }
        self.update_visual_marks();
        self.cancel_rename_if_target_gone();
        ok
    }

    /// Renaming acts on the entry under the cursor; if a refresh moved the
    /// cursor off the file being renamed (it was removed or renamed from
    /// outside), stop instead of renaming another file.
    fn cancel_rename_if_target_gone(&mut self) {
        let Mode::Rename { original, .. } = &self.mode else {
            return;
        };
        let pane = self.current_pane();
        let still_there = pane
            .items
            .get(pane.selected)
            .is_some_and(|e| e.file_name().to_string_lossy() == original.as_str());
        if !still_there {
            let text = format!("Rename cancelled: {original} is gone");
            self.mode = Mode::Filer;
            self.notify_error(text);
        }
    }

    /// In Visual mode, mark the range between the anchor and the cursor.
    /// If the anchor file is gone, the selection restarts at the cursor.
    pub fn update_visual_marks(&mut self) {
        let Mode::Visual { anchor } = &self.mode else {
            return;
        };
        let anchor = anchor.clone();
        let pane = self.current_pane_mut();
        let end = pane.selected;
        let start = match pane.items.iter().position(|e| e.path() == anchor) {
            Some(i) => i,
            None => {
                if let Some(p) = pane.selected_path() {
                    self.mode = Mode::Visual { anchor: p };
                }
                end
            }
        };
        let pane = self.current_pane_mut();
        pane.marked.clear();
        for i in start.min(end)..=start.max(end) {
            pane.mark_index(i);
        }
    }

    /// Time until a "just changed" highlight in either pane expires.
    pub fn next_highlight_expiry(&self, now: SystemTime) -> Option<Duration> {
        let mtimes = [&self.left, &self.right]
            .into_iter()
            .flat_map(|p| p.items.iter())
            .filter_map(|e| e.metadata().and_then(|m| m.modified()).ok())
            .chain(self.change_mtimes());
        watch::next_expiry(mtimes, now, self.session_start)
    }

    /// Path of the entry under the cursor in the active pane.
    pub fn selected_path(&self) -> Option<PathBuf> {
        self.current_pane().selected_path()
    }

    /// Show an image/video preview in the opposite pane. Returns false if
    /// the path is not a previewable media file. The preview loads in the
    /// background (see [`crate::preview`]).
    pub fn open_preview(&mut self, path: &std::path::Path) -> bool {
        if !self.show_media(path) {
            return false;
        }
        self.mode = Mode::Preview;
        true
    }

    /// Show `path` in [`App::preview`] and prefetch the media entries
    /// around the cursor. False if `path` is not an image / video. Cheap to
    /// repeat: cached entries are reused unless their file changed.
    fn show_media(&mut self, path: &std::path::Path) -> bool {
        if !media::is_media(path) || path.is_dir() {
            return false;
        }
        let pane = self.current_pane();
        let flags = media_flags(pane);
        let prefetch = preview::prefetch_around(&flags, pane.selected, true)
            .into_iter()
            .map(|i| pane.items[i].path())
            .collect();
        self.preview.show(path.to_path_buf(), prefetch);
        true
    }

    /// `P`: turn the pane preview in the opposite pane on or off.
    pub fn toggle_text_preview(&mut self) {
        self.text_preview = match self.text_preview {
            Some(_) => {
                // Turned off from the file list, so the gallery is not open.
                self.preview.close();
                None
            }
            None => Some(PanePreview::new()),
        };
        self.sync_text_preview();
    }

    /// Whether the `P` pane preview shows an image / video (drawn by
    /// [`App::preview`]) rather than text.
    pub fn pane_preview_shows_media(&self) -> bool {
        self.text_preview.is_some() && self.preview.current().is_some()
    }

    /// Load the entry under the cursor into the pane preview, if it is on
    /// and the entry changed. Called before every draw. Images / videos
    /// load in the background in [`App::preview`] (shared with the `Enter`
    /// gallery); other entries are read as text.
    pub fn sync_text_preview(&mut self) {
        if self.text_preview.is_none() || matches!(self.mode, Mode::Preview) {
            return;
        }
        let path = self.selected_path();
        let is_media = path.as_deref().is_some_and(|p| self.show_media(p));
        if !is_media {
            self.preview.hide();
        }
        let mut error = None;
        if let Some(preview) = self.text_preview.as_mut() {
            preview.sync(path.as_deref(), |p| {
                if is_media {
                    // Shown by `App::preview`; nothing to read here.
                    return PreviewContent::Message(String::new());
                }
                text_view::load_preview(p).unwrap_or_else(|e| {
                    error = Some(e.clone());
                    PreviewContent::Message(e)
                })
            });
        }
        if let Some(e) = error {
            self.notify_error(e);
        }
    }

    /// Leave the `Enter` gallery. With `P` on, the pane preview keeps the
    /// loader and cache and goes on showing the entry under the cursor.
    pub fn close_preview(&mut self) {
        if self.text_preview.is_none() {
            self.preview.close();
        }
        self.mode = Mode::Filer;
    }

    pub fn on_enter(&mut self) {
        let pane = self.current_pane_mut();
        let Some(path) = pane.items.get(pane.selected).map(|e| e.path()) else {
            return;
        };
        if path.is_dir() {
            if let Err(e) = pane.change_dir(path.clone()) {
                self.notify_error(fs_error_message(&e, &path));
            }
            return;
        }
        match fs::read_to_string(&path) {
            Ok(content) => {
                let title = path
                    .file_name()
                    .map(|os_str| os_str.to_string_lossy().into_owned())
                    .unwrap_or_default();
                self.mode = Mode::Viewer {
                    doc: TextDoc::for_file(title, &path, &content),
                    offset: 0,
                    at_end: false,
                    diff: false,
                };
            }
            Err(e) => self.notify_error(fs_error_message(&e, &path)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Create a fresh, empty directory under the system temp dir.
    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("kura-test-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn is_visible_combines_the_filters() {
        let all = ListFilter::default();
        let no_hidden = ListFilter {
            hide_hidden: true,
            ..all
        };
        let no_ignored = ListFilter {
            hide_ignored: true,
            ..all
        };
        let changed = ListFilter {
            changed_only: true,
            ..all
        };
        let ignored = Some(FileStatus::Ignored);
        let modified = Some(FileStatus::Modified);

        assert!(is_visible(".env", ignored, all));
        assert!(!is_visible(".env", None, no_hidden));
        assert!(is_visible("a.env", None, no_hidden));
        assert!(!is_visible("target", ignored, no_ignored));
        assert!(is_visible("src", Some(FileStatus::DirtyDir), no_ignored));
        assert!(is_visible("new", Some(FileStatus::Untracked), no_ignored));
        assert!(is_visible("a", None, no_ignored));
        assert!(!is_visible("a", None, changed));
        assert!(!is_visible("target", ignored, changed));
        // Combined with `M`: a changed dot file is hidden by `.`.
        let both = ListFilter {
            hide_hidden: true,
            ..changed
        };
        assert!(!is_visible(".github", modified, both));
        assert!(is_visible("src", modified, both));
    }

    #[test]
    fn change_dir_moves_into_readable_directory() {
        let root = temp_dir("change-ok");
        fs::create_dir(root.join("sub")).unwrap();
        fs::write(root.join("sub").join("a.txt"), "a").unwrap();

        let mut pane = Pane::new(root.clone()).unwrap();
        pane.selected = 0;
        pane.change_dir(root.join("sub")).unwrap();

        assert_eq!(pane.current_dir, root.join("sub"));
        assert_eq!(pane.items.len(), 1);
        assert_eq!(pane.selected, 0);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn change_dir_failure_keeps_current_state() {
        let root = temp_dir("change-fail");
        fs::write(root.join("a.txt"), "a").unwrap();
        fs::write(root.join("b.txt"), "b").unwrap();

        let mut pane = Pane::new(root.clone()).unwrap();
        pane.selected = 1;
        pane.mark_index(0);
        assert!(pane.change_dir(root.join("missing")).is_err());

        assert_eq!(pane.current_dir, root);
        assert_eq!(pane.items.len(), 2);
        assert_eq!(pane.selected, 1);
        assert!(pane.marked.contains(&root.join("a.txt")));
        fs::remove_dir_all(&root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn change_dir_into_unreadable_directory_fails() {
        use std::os::unix::fs::PermissionsExt;

        let root = temp_dir("change-perm");
        let locked = root.join("locked");
        fs::create_dir(&locked).unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();

        // Running as root bypasses permissions; nothing to check then.
        if fs::read_dir(&locked).is_err() {
            let mut pane = Pane::new(root.clone()).unwrap();
            assert!(pane.change_dir(locked.clone()).is_err());
            assert_eq!(pane.current_dir, root);
        }

        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
        fs::remove_dir_all(&root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn fs_error_message_puts_reason_first_with_file_name_only() {
        let err = io::Error::from_raw_os_error(libc::EACCES);
        let msg = fs_error_message(&err, Path::new("/very/long/path/to/a_locked"));
        assert_eq!(msg, "Permission denied: a_locked");
    }

    #[test]
    fn fs_error_message_keeps_custom_reason() {
        let custom = io::Error::new(io::ErrorKind::InvalidInput, "bad input");
        assert_eq!(fs_error_message(&custom, Path::new("/")), "bad input: /");
    }

    #[test]
    fn go_parent_selects_previous_directory() {
        let root = temp_dir("go-parent");
        for name in ["a", "b", "c"] {
            fs::create_dir(root.join(name)).unwrap();
        }

        let mut pane = Pane::new(root.join("b")).unwrap();
        pane.go_parent().unwrap();

        assert_eq!(pane.current_dir, root);
        assert_eq!(pane.items[pane.selected].file_name(), "b");
        fs::remove_dir_all(&root).unwrap();
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
    fn git_filter_shows_only_changed_entries() {
        let root = temp_dir("git-filter");
        if !git(&root, &["init", "-q", "-b", "main"]) {
            fs::remove_dir_all(&root).unwrap();
            return;
        }
        fs::create_dir(root.join("clean")).unwrap();
        fs::write(root.join("clean").join("c"), "c").unwrap();
        fs::create_dir(root.join("dirty")).unwrap();
        fs::write(root.join("dirty").join("d"), "d").unwrap();
        fs::write(root.join("same"), "s").unwrap();
        fs::write(root.join("changed"), "1").unwrap();
        assert!(git(&root, &["add", "."]));
        assert!(git(&root, &["commit", "-qm", "init"]));
        fs::write(root.join("changed"), "2").unwrap();
        fs::write(root.join("dirty").join("d"), "2").unwrap();
        fs::write(root.join("new"), "n").unwrap();

        let mut pane = Pane::new(root.clone()).unwrap();
        let git_status = pane.git.as_ref().expect("inside a repository");
        assert_eq!(git_status.branch.as_deref(), Some("main"));
        let status = |pane: &Pane, name: &str| {
            let e = pane.items.iter().find(|e| e.file_name() == name).unwrap();
            pane.git_status_of(e)
        };
        assert_eq!(status(&pane, "changed"), Some(FileStatus::Modified));
        assert_eq!(status(&pane, "new"), Some(FileStatus::Untracked));
        assert_eq!(status(&pane, "dirty"), Some(FileStatus::DirtyDir));
        assert_eq!(status(&pane, "same"), None);

        pane.selected = pane
            .items
            .iter()
            .position(|e| e.file_name() == "new")
            .unwrap();
        pane.marked.insert(root.join("same"));
        pane.marked.insert(root.join("new"));
        pane.toggle_git_filter().unwrap();
        let names: Vec<_> = pane.items.iter().map(|e| e.file_name()).collect();
        assert_eq!(names, ["dirty", "changed", "new"]); // directories first
        assert_eq!(pane.items[pane.selected].file_name(), "new");
        // Marks on hidden entries are dropped so they are never acted on.
        assert_eq!(pane.marked, HashSet::from([root.join("new")]));

        // Paths resolve against the shown subdirectory, not the repo root.
        pane.change_dir(root.join("dirty")).unwrap();
        assert_eq!(status(&pane, "d"), Some(FileStatus::Modified));

        pane.go_parent().unwrap();
        pane.toggle_git_filter().unwrap();
        assert_eq!(pane.items.len(), 6); // .git, changed, clean, dirty, new, same
        fs::remove_dir_all(&root).unwrap();
    }
}
