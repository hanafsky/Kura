//! "Agent changes": files a coding agent edited, listed in one pane (`A`).
//!
//! A Claude Code `PostToolUse` hook appends the absolute path of every file
//! the agent edits to a log, one path per line (see the README). kura reads
//! that log, keeps the paths under the directory it was started in and
//! lists them newest first. The log's directory is watched while the list
//! is open, so new edits show up by themselves. Parsing the log, choosing
//! its location and following a change are plain functions here.

use std::{
    ffi::OsStr,
    fs,
    io::{self, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    time::SystemTime,
};

use crate::app::{fs_error_message, App};
use crate::mode::Mode;
use crate::watch;

/// Environment variable that overrides the log's location.
pub const LOG_ENV: &str = "KURA_AGENT_LOG";

/// Only this much of the end of the log is read, so a log that has grown
/// for months does not slow down opening the list.
const MAX_READ: u64 = 1024 * 1024;

/// Where the hook writes the log: `KURA_AGENT_LOG` if set, else
/// `$XDG_STATE_HOME/kura/agent-changes.log`, else
/// `~/.local/state/kura/agent-changes.log`. Empty values count as unset.
pub fn log_path(
    override_path: Option<&OsStr>,
    xdg_state_home: Option<&OsStr>,
    home: Option<&OsStr>,
) -> Option<PathBuf> {
    let set = |v: Option<&OsStr>| v.filter(|v| !v.is_empty()).map(PathBuf::from);
    if let Some(p) = set(override_path) {
        return Some(p);
    }
    let state =
        set(xdg_state_home).or_else(|| set(home).map(|h| h.join(".local").join("state")))?;
    Some(state.join("kura").join("agent-changes.log"))
}

/// [`log_path`] from the environment (`HOME`, or `USERPROFILE` on Windows).
pub fn log_path_from_env() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"));
    log_path(
        std::env::var_os(LOG_ENV).as_deref(),
        std::env::var_os("XDG_STATE_HOME").as_deref(),
        home.as_deref(),
    )
}

/// The changed files in `log` (one path per line, oldest first) that lie
/// under one of `roots`: newest first, each once. Blank lines and relative
/// paths are skipped.
pub fn parse_log(log: &str, roots: &[PathBuf]) -> Vec<PathBuf> {
    let mut seen = std::collections::HashSet::new();
    log.lines()
        .rev()
        .map(|l| l.trim_end_matches('\r'))
        .filter(|l| !l.trim().is_empty())
        .map(PathBuf::from)
        .filter(|p| p.is_absolute() && roots.iter().any(|r| p.starts_with(r)))
        .filter(|p| seen.insert(p.clone()))
        .collect()
}

/// `path` shown relative to the first of `roots` it lies under.
pub fn display_path(path: &Path, roots: &[PathBuf]) -> String {
    roots
        .iter()
        .find_map(|r| path.strip_prefix(r).ok())
        .unwrap_or(path)
        .display()
        .to_string()
}

/// Whether one of `changed` is `file` (in any of the spellings of its
/// directory, see [`watch::dir_keys`]), or its directory itself.
pub fn touches_file(changed: &[PathBuf], file: &Path) -> bool {
    let (Some(dir), Some(name)) = (file.parent(), file.file_name()) else {
        return false;
    };
    watch::dir_keys(dir)
        .iter()
        .any(|d| changed.iter().any(|p| p == d || *p == d.join(name)))
}

/// The end of the log (at most [`MAX_READ`] bytes, starting at a whole
/// line). A missing log reads as empty: nothing was edited yet.
fn read_log(path: &Path) -> io::Result<String> {
    let mut file = match fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(String::new()),
        Err(e) => return Err(e),
    };
    let len = file.metadata()?.len();
    let cut = len > MAX_READ;
    if cut {
        file.seek(SeekFrom::Start(len - MAX_READ))?;
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    let mut text = String::from_utf8_lossy(&bytes).into_owned();
    if cut {
        // Drop the partial first line.
        let start = text.find('\n').map_or(text.len(), |i| i + 1);
        text.drain(..start);
    }
    Ok(text)
}

/// Where `Enter` on a listed change goes.
#[derive(Debug, PartialEq, Eq)]
pub enum Jump {
    /// Into `dir`, with the cursor on the entry named `name`.
    Entry {
        dir: PathBuf,
        name: std::ffi::OsString,
    },
    /// The file is gone; into the directory it was in.
    Gone { dir: PathBuf },
    /// The file and its directory are gone.
    Nowhere,
}

/// Decide where `Enter` on `path` goes, given whether it and its directory
/// still exist.
pub fn plan_jump(path: &Path, exists: bool, dir_exists: bool) -> Jump {
    match (path.parent(), path.file_name()) {
        (Some(dir), Some(name)) if exists => Jump::Entry {
            dir: dir.to_path_buf(),
            name: name.to_os_string(),
        },
        (Some(dir), _) if dir_exists => Jump::Gone {
            dir: dir.to_path_buf(),
        },
        _ => Jump::Nowhere,
    }
}

impl App {
    /// Spellings of the directory kura was started in; only changes under
    /// it are listed.
    pub fn change_roots(&self) -> Vec<PathBuf> {
        watch::dir_keys(&self.launch_dir)
    }

    /// Read the log again. `None` (after reporting why) if it cannot be read.
    fn read_changes(&mut self) -> Option<Vec<PathBuf>> {
        let Some(log) = self.agent_log.clone() else {
            self.notify_error(format!("Cannot find the agent log: set {LOG_ENV}"));
            return None;
        };
        match read_log(&log) {
            Ok(text) => Some(parse_log(&text, &self.change_roots())),
            Err(e) => {
                self.notify_error(fs_error_message(&e, &log));
                None
            }
        }
    }

    /// `A`: list the agent's changes in the active pane.
    pub fn open_agent_changes(&mut self) {
        let Some(items) = self.read_changes() else {
            return;
        };
        if items.is_empty() {
            self.notify_info("No agent changes logged yet");
        }
        self.mode = Mode::Changes { items, selected: 0 };
    }

    /// Re-read the list after the log changed, keeping the cursor on the
    /// same file (or row).
    pub fn reload_agent_changes(&mut self) {
        if !matches!(self.mode, Mode::Changes { .. }) {
            return;
        }
        let Some(new_items) = self.read_changes() else {
            return;
        };
        if let Mode::Changes { items, selected } = &mut self.mode {
            let cursor = items.get(*selected).cloned();
            let found = cursor.and_then(|c| new_items.iter().position(|p| *p == c));
            *selected = found
                .unwrap_or(*selected)
                .min(new_items.len().saturating_sub(1));
            *items = new_items;
        }
    }

    /// The listed change under the cursor.
    pub fn selected_change(&self) -> Option<&Path> {
        match &self.mode {
            Mode::Changes { items, selected } => items.get(*selected).map(PathBuf::as_path),
            _ => None,
        }
    }

    /// Move the cursor of the list to `index` (clamped).
    pub fn select_change(&mut self, index: usize) {
        if let Mode::Changes { items, selected } = &mut self.mode {
            *selected = index.min(items.len().saturating_sub(1));
        }
    }

    /// Move the cursor of the list `n` rows down / up.
    pub fn move_change_cursor(&mut self, n: usize, down: bool) {
        if let Mode::Changes { selected, .. } = &self.mode {
            let to = if down {
                selected.saturating_add(n)
            } else {
                selected.saturating_sub(n)
            };
            self.select_change(to);
        }
    }

    /// `Enter`: close the list and show the file in its directory (or the
    /// directory it was in, if it is gone).
    pub fn jump_to_change(&mut self) {
        let Some(path) = self.selected_change().map(Path::to_path_buf) else {
            return;
        };
        let dir_exists = path.parent().is_some_and(Path::is_dir);
        let (dir, name) = match plan_jump(&path, path.exists(), dir_exists) {
            Jump::Entry { dir, name } => (dir, Some(name)),
            Jump::Gone { dir } => (dir, None),
            Jump::Nowhere => {
                self.notify_error(format!("No longer exists: {}", path.display()));
                return;
            }
        };
        let pane = self.current_pane_mut();
        if let Err(e) = pane.change_dir(dir.clone()) {
            self.notify_error(fs_error_message(&e, &dir));
            return;
        }
        self.mode = Mode::Filer;
        let label = path
            .file_name()
            .unwrap_or(path.as_os_str())
            .to_string_lossy();
        let Some(name) = name else {
            self.notify_info(format!("No longer exists: {label}"));
            return;
        };
        let pane = self.current_pane_mut();
        match pane.items.iter().position(|e| e.file_name() == name) {
            Some(i) => pane.selected = i,
            // Hidden by a filter (`.`, `M`, `i`).
            None => self.notify_info(format!("Hidden by a filter: {label}")),
        }
    }

    /// `e` in the list: edit the file under the cursor.
    pub fn edit_change(&mut self) {
        let Some(path) = self.selected_change().map(Path::to_path_buf) else {
            return;
        };
        if path.exists() {
            self.pending_edit = Some(path);
        } else {
            let label = path
                .file_name()
                .unwrap_or(path.as_os_str())
                .to_string_lossy();
            self.notify_error(format!("No longer exists: {label}"));
        }
    }

    /// `Y` in the list: copy the file under the cursor as an `@path`, the
    /// same way as from the file list.
    pub fn copy_change_at_path(&mut self) {
        if let Some(path) = self.selected_change().map(Path::to_path_buf) {
            crate::clipboard::copy_paths_as_at(self, &[path]);
        }
    }

    /// The log's directory, watched while the list is open (if it exists).
    pub fn agent_log_watch_dir(&self) -> Option<PathBuf> {
        if !matches!(self.mode, Mode::Changes { .. }) {
            return None;
        }
        let dir = self.agent_log.as_deref()?.parent()?;
        dir.is_dir().then(|| dir.to_path_buf())
    }

    /// Whether a watcher batch touched the log.
    pub fn touches_agent_log(&self, changed: &[PathBuf]) -> bool {
        self.agent_log
            .as_deref()
            .is_some_and(|log| touches_file(changed, log))
    }

    /// Modification times of the listed files, for the "just changed"
    /// highlight to expire on time.
    pub fn change_mtimes(&self) -> Vec<SystemTime> {
        match &self.mode {
            Mode::Changes { items, .. } => items
                .iter()
                .filter_map(|p| fs::metadata(p).and_then(|m| m.modified()).ok())
                .collect(),
            _ => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn os(s: &str) -> Option<&OsStr> {
        Some(OsStr::new(s))
    }

    #[test]
    fn log_path_prefers_the_override_then_xdg_then_home() {
        assert_eq!(
            log_path(os("/x/log"), os("/state"), os("/home/u")),
            Some(PathBuf::from("/x/log"))
        );
        assert_eq!(
            log_path(None, os("/state"), os("/home/u")),
            Some(PathBuf::from("/state/kura/agent-changes.log"))
        );
        assert_eq!(
            log_path(os(""), os(""), os("/home/u")),
            Some(PathBuf::from("/home/u/.local/state/kura/agent-changes.log"))
        );
        assert_eq!(log_path(None, None, None), None);
    }

    #[cfg(unix)]
    #[test]
    fn parse_log_keeps_paths_under_the_roots_newest_first_once() {
        let roots = vec![PathBuf::from("/proj"), PathBuf::from("/private/proj")];
        let log = "/proj/a.rs\n\
                   /other/x.rs\n\
                   relative.rs\n\
                   \n\
                   /proj/b.rs\r\n\
                   /proj/a.rs\n\
                   /projector/c.rs\n\
                   /private/proj/d.rs\n";
        assert_eq!(
            parse_log(log, &roots),
            [
                PathBuf::from("/private/proj/d.rs"),
                PathBuf::from("/proj/a.rs"),
                PathBuf::from("/proj/b.rs"),
            ]
        );
        assert!(parse_log("", &roots).is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn display_path_is_relative_to_the_matching_root() {
        let roots = vec![PathBuf::from("/proj"), PathBuf::from("/private/proj")];
        assert_eq!(
            display_path(Path::new("/proj/src/a.rs"), &roots),
            "src/a.rs"
        );
        assert_eq!(display_path(Path::new("/private/proj/b"), &roots), "b");
        assert_eq!(display_path(Path::new("/else/c"), &roots), "/else/c");
    }

    #[test]
    fn touches_file_matches_the_file_or_its_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let log = tmp.path().join("agent-changes.log");
        let real = tmp.path().canonicalize().unwrap();
        assert!(touches_file(std::slice::from_ref(&log), &log));
        assert!(touches_file(&[real.join("agent-changes.log")], &log));
        assert!(touches_file(&[tmp.path().to_path_buf()], &log));
        assert!(!touches_file(&[tmp.path().join("other")], &log));
        assert!(!touches_file(&[], &log));
    }

    #[cfg(unix)]
    #[test]
    fn plan_jump_by_what_still_exists() {
        let p = Path::new("/proj/src/a.rs");
        assert_eq!(
            plan_jump(p, true, true),
            Jump::Entry {
                dir: PathBuf::from("/proj/src"),
                name: "a.rs".into()
            }
        );
        assert_eq!(
            plan_jump(p, false, true),
            Jump::Gone {
                dir: PathBuf::from("/proj/src")
            }
        );
        assert_eq!(plan_jump(p, false, false), Jump::Nowhere);
    }

    #[test]
    fn read_log_of_a_missing_file_is_empty() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(read_log(&tmp.path().join("none")).unwrap(), "");
    }

    #[test]
    fn read_log_keeps_whole_lines_of_the_end() {
        let tmp = tempfile::tempdir().unwrap();
        let log = tmp.path().join("log");
        let line = "/p/".to_string() + &"x".repeat(100) + "\n";
        let count = (MAX_READ as usize / line.len()) + 10;
        fs::write(&log, line.repeat(count) + "/p/last\n").unwrap();
        let text = read_log(&log).unwrap();
        assert!(text.len() as u64 <= MAX_READ);
        assert!(text.lines().all(|l| l == line.trim_end() || l == "/p/last"));
        assert!(text.ends_with("/p/last\n"));
    }

    // --- `A` list through `App::apply` ---

    use crate::app::MessageLevel;
    use crate::command::Command;
    use ratatui_image::picker::Picker;

    /// A project with `src/a.rs`, `b.md` and `.hidden`, an app started in
    /// it, and a log (in another directory) listing `lines` in order.
    fn setup(lines: &[&str]) -> (tempfile::TempDir, tempfile::TempDir, App) {
        let proj = tempfile::tempdir().unwrap();
        fs::create_dir(proj.path().join("src")).unwrap();
        fs::write(proj.path().join("src/a.rs"), "a").unwrap();
        fs::write(proj.path().join("b.md"), "b").unwrap();
        fs::write(proj.path().join(".hidden"), "h").unwrap();
        let state = tempfile::tempdir().unwrap();
        let log = state.path().join("agent-changes.log");
        let text: String = lines
            .iter()
            .map(|l| format!("{}\n", proj.path().join(l).display()))
            .collect();
        fs::write(&log, text).unwrap();
        let mut app = App::with_dir(Picker::halfblocks(), proj.path().to_path_buf()).unwrap();
        app.agent_log = Some(log);
        (proj, state, app)
    }

    fn listed(app: &App) -> Vec<PathBuf> {
        match &app.mode {
            Mode::Changes { items, .. } => items.clone(),
            _ => panic!("not listing agent changes"),
        }
    }

    fn message(app: &App) -> Option<(MessageLevel, String)> {
        app.message.as_ref().map(|m| (m.level, m.text.clone()))
    }

    #[test]
    fn a_lists_changes_under_the_launch_dir_newest_first() {
        let (proj, state, mut app) = setup(&["b.md", "src/a.rs", "b.md"]);
        let mut log = fs::read_to_string(state.path().join("agent-changes.log")).unwrap();
        log.push_str("/elsewhere/x.rs\n");
        fs::write(state.path().join("agent-changes.log"), log).unwrap();
        let _ = app.apply(Command::ToggleAgentChanges);
        let p = proj.path();
        assert_eq!(listed(&app), [p.join("b.md"), p.join("src/a.rs")]);
        assert_eq!(app.selected_change(), Some(p.join("b.md").as_path()));
        // `A` again (or Esc) goes back to the file list.
        let _ = app.apply(Command::ToggleAgentChanges);
        assert!(matches!(app.mode, Mode::Filer));
        let _ = app.apply(Command::ToggleAgentChanges);
        let _ = app.apply(Command::Back);
        assert!(matches!(app.mode, Mode::Filer));
    }

    #[test]
    fn a_without_a_log_opens_an_empty_list() {
        let (_proj, state, mut app) = setup(&[]);
        fs::remove_file(state.path().join("agent-changes.log")).unwrap();
        let _ = app.apply(Command::ToggleAgentChanges);
        assert!(listed(&app).is_empty());
        assert_eq!(
            message(&app),
            Some((MessageLevel::Info, "No agent changes logged yet".into()))
        );
        // Nothing to act on.
        let _ = app.apply(Command::Open);
        let _ = app.apply(Command::OpenEditor);
        assert!(matches!(app.mode, Mode::Changes { .. }));
        assert_eq!(app.pending_edit, None);
    }

    #[test]
    fn a_is_ignored_outside_the_file_list() {
        let (_proj, _state, mut app) = setup(&["b.md"]);
        let _ = app.apply(Command::SortMenu);
        let _ = app.apply(Command::ToggleAgentChanges);
        assert!(matches!(app.mode, Mode::Sort { .. }));
    }

    #[test]
    fn moving_in_the_list_stays_within_it() {
        let (proj, _state, mut app) = setup(&["b.md", "src/a.rs", ".hidden"]);
        let _ = app.apply(Command::ToggleAgentChanges);
        let _ = app.apply(Command::MoveDown(5));
        assert_eq!(
            app.selected_change(),
            Some(proj.path().join("b.md").as_path())
        );
        let _ = app.apply(Command::MoveUp(1));
        assert_eq!(
            app.selected_change(),
            Some(proj.path().join("src/a.rs").as_path())
        );
        let _ = app.apply(Command::GoTop);
        assert_eq!(
            app.selected_change(),
            Some(proj.path().join(".hidden").as_path())
        );
        let _ = app.apply(Command::GoBottom);
        assert_eq!(
            app.selected_change(),
            Some(proj.path().join("b.md").as_path())
        );
    }

    #[test]
    fn enter_goes_to_the_file_in_its_directory() {
        let (proj, _state, mut app) = setup(&["src/a.rs"]);
        let _ = app.apply(Command::ToggleAgentChanges);
        let _ = app.apply(Command::Open);
        assert!(matches!(app.mode, Mode::Filer));
        let pane = app.current_pane();
        assert_eq!(pane.current_dir, proj.path().join("src"));
        assert_eq!(app.selected_path(), Some(proj.path().join("src/a.rs")));
    }

    #[test]
    fn enter_on_a_filtered_file_says_so() {
        let (proj, _state, mut app) = setup(&[".hidden"]);
        let _ = app.apply(Command::ToggleHidden); // hide dot files
        let _ = app.apply(Command::ToggleAgentChanges);
        let _ = app.apply(Command::Open);
        assert!(matches!(app.mode, Mode::Filer));
        assert_eq!(app.current_pane().current_dir, proj.path());
        assert_eq!(
            message(&app),
            Some((MessageLevel::Info, "Hidden by a filter: .hidden".into()))
        );
    }

    #[test]
    fn deleted_files_stay_listed_and_enter_opens_their_directory() {
        let (proj, _state, mut app) = setup(&["src/gone.rs"]);
        let _ = app.apply(Command::ToggleAgentChanges);
        assert_eq!(listed(&app), [proj.path().join("src/gone.rs")]);
        let _ = app.apply(Command::OpenEditor);
        assert_eq!(app.pending_edit, None);
        assert_eq!(
            message(&app),
            Some((MessageLevel::Error, "No longer exists: gone.rs".into()))
        );
        let _ = app.apply(Command::Open);
        assert!(matches!(app.mode, Mode::Filer));
        assert_eq!(app.current_pane().current_dir, proj.path().join("src"));
        assert_eq!(
            message(&app),
            Some((MessageLevel::Info, "No longer exists: gone.rs".into()))
        );
    }

    #[test]
    fn enter_when_the_directory_is_gone_too_stays_in_the_list() {
        let (proj, _state, mut app) = setup(&["old/x.rs"]);
        let _ = app.apply(Command::ToggleAgentChanges);
        let _ = app.apply(Command::Open);
        assert!(matches!(app.mode, Mode::Changes { .. }));
        assert_eq!(app.current_pane().current_dir, proj.path());
        assert_eq!(message(&app).map(|m| m.0), Some(MessageLevel::Error));
    }

    #[test]
    fn e_edits_and_y_copies_the_file_under_the_cursor() {
        let (proj, _state, mut app) = setup(&["b.md", "src/a.rs"]);
        let _ = app.apply(Command::ToggleAgentChanges);
        let _ = app.apply(Command::OpenEditor);
        assert_eq!(app.pending_edit, Some(proj.path().join("src/a.rs")));
        let _ = app.apply(Command::CopyAtPaths);
        let copied = crate::clipboard::LAST_COPY.with(|c| c.borrow_mut().take());
        assert_eq!(copied.as_deref(), Some("@src/a.rs"));
        assert!(matches!(app.mode, Mode::Changes { .. }));
    }

    #[test]
    fn a_change_to_the_log_reloads_the_list_keeping_the_cursor() {
        let (proj, state, mut app) = setup(&["b.md", "src/a.rs"]);
        let log = state.path().join("agent-changes.log");
        let _ = app.apply(Command::ToggleAgentChanges);
        let _ = app.apply(Command::MoveDown(1)); // b.md
        assert_eq!(
            app.watch_targets().last(),
            Some(&state.path().to_path_buf())
        );
        let mut text = fs::read_to_string(&log).unwrap();
        text.push_str(&format!("{}\n", proj.path().join(".hidden").display()));
        fs::write(&log, text).unwrap();
        // Unrelated changes do not re-read the log.
        app.handle_fs_event(&watch::FsEvent::Changed(vec![state.path().join("x")]));
        assert_eq!(listed(&app).len(), 2);
        app.handle_fs_event(&watch::FsEvent::Changed(vec![log]));
        let p = proj.path();
        assert_eq!(
            listed(&app),
            [p.join(".hidden"), p.join("src/a.rs"), p.join("b.md")]
        );
        assert_eq!(app.selected_change(), Some(p.join("b.md").as_path()));
        // Closed, the log's directory is no longer watched.
        let _ = app.apply(Command::Back);
        assert!(!app.watch_targets().contains(&state.path().to_path_buf()));
    }
}
