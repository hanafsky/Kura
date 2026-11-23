use std::{
    collections::HashSet,
    fs, io,
    path::{Path, PathBuf},
};

use ratatui_image::picker::Picker;

use crate::media::{self, Preview};
use crate::mode::{Mode, PaneType};

pub struct Pane {
    pub items: Vec<fs::DirEntry>,
    pub selected: usize,
    pub current_dir: PathBuf,
    pub marked: HashSet<usize>,
}

/// Read a directory's entries sorted by name. Entries that cannot be read
/// are skipped so that a single bad entry does not hide the whole listing.
fn read_entries(path: &Path) -> io::Result<Vec<fs::DirEntry>> {
    let mut entries: Vec<fs::DirEntry> = fs::read_dir(path)?.filter_map(Result::ok).collect();
    entries.sort_by_key(|e| e.file_name());
    Ok(entries)
}

impl Pane {
    pub fn new(path: PathBuf) -> io::Result<Self> {
        let entries = read_entries(&path)?;
        Ok(Self {
            items: entries,
            selected: 0,
            current_dir: path,
            marked: HashSet::new(),
        })
    }

    pub fn refresh(&mut self) -> io::Result<()> {
        self.items = read_entries(&self.current_dir)?;
        self.selected = 0;
        self.marked.clear();
        Ok(())
    }

    /// Move to `path`. The pane is only updated if the directory can be read;
    /// on error the current directory and listing are left untouched.
    pub fn change_dir(&mut self, path: PathBuf) -> io::Result<()> {
        let entries = read_entries(&path)?;
        self.items = entries;
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
    /// Detects the terminal graphics protocol (Kitty / Sixel / iTerm2 / halfblocks).
    pub picker: Picker,
    pub preview: Option<Preview>,
    /// Detected graphics protocol / font size (for the preview footer).
    pub graphics_info: String,
    /// Message shown in the status line (errors, result counts).
    pub message: Option<Message>,
}

impl App {
    pub fn new(picker: Picker) -> io::Result<Self> {
        Self::with_dir(picker, std::env::current_dir()?)
    }

    /// Start with both panes in `dir`.
    pub fn with_dir(picker: Picker, dir: PathBuf) -> io::Result<Self> {
        Ok(Self {
            left: Pane::new(dir.clone())?,
            right: Pane::new(dir)?,
            active: PaneType::Left,
            mode: Mode::Filer,
            clipboard: Vec::new(),
            picker,
            preview: None,
            graphics_info: String::new(),
            message: None,
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

    /// Path of the entry under the cursor in the active pane.
    pub fn selected_path(&self) -> Option<PathBuf> {
        let pane = self.current_pane();
        pane.items.get(pane.selected).map(|e| e.path())
    }

    /// Show an image/video preview in the opposite pane. Returns false if
    /// the path is not a previewable media file.
    pub fn open_preview(&mut self, path: &std::path::Path) -> bool {
        let preview = if media::is_image(path) {
            media::load_image_preview(&self.picker, path)
        } else if media::is_video(path) {
            media::load_video_preview(&self.picker, path)
        } else {
            return false;
        };
        self.preview = Some(preview);
        self.mode = Mode::Preview;
        true
    }

    pub fn close_preview(&mut self) {
        self.preview = None;
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
                    content,
                    title,
                    offset: 0,
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
        pane.marked.insert(0);
        assert!(pane.change_dir(root.join("missing")).is_err());

        assert_eq!(pane.current_dir, root);
        assert_eq!(pane.items.len(), 2);
        assert_eq!(pane.selected, 1);
        assert!(pane.marked.contains(&0));
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
}
