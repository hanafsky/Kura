use std::{collections::HashSet, fs, io, path::PathBuf};

use crate::mode::{Mode, PaneType};

pub struct Pane {
    pub items: Vec<fs::DirEntry>,
    pub selected: usize,
    pub current_dir: PathBuf,
    pub marked: HashSet<usize>,
}

impl Pane {
    pub fn new(path: PathBuf) -> io::Result<Self> {
        let mut entries = fs::read_dir(&path)?.collect::<Result<Vec<_>, _>>()?;
        entries.sort_by_key(|e| e.file_name());
        Ok(Self {
            items: entries,
            selected: 0,
            current_dir: path,
            marked: HashSet::new(),
        })
    }

    pub fn refresh(&mut self) -> io::Result<()> {
        let mut entries = fs::read_dir(&self.current_dir)?.collect::<Result<Vec<_>, _>>()?;
        entries.sort_by_key(|e| e.file_name());
        self.items = entries;
        self.selected = 0;
        self.marked.clear();
        Ok(())
    }
}

pub struct App {
    pub left: Pane,
    pub right: Pane,
    pub active: PaneType,
    pub mode: Mode,
    pub clipboard: Vec<PathBuf>,
}

impl App {
    pub fn new() -> io::Result<Self> {
        let cwd = std::env::current_dir()?;
        Ok(Self {
            left: Pane::new(cwd.clone())?,
            right: Pane::new(cwd)?,
            active: PaneType::Left,
            mode: Mode::Filer,
            clipboard: Vec::new(),
        })
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
        if let Some(parent) = pane.current_dir.parent() {
            pane.current_dir = parent.to_path_buf();
            let _ = pane.refresh();
        }
    }

    pub fn on_enter(&mut self) {
        let pane = self.current_pane_mut();
        if let Some(entry) = pane.items.get(pane.selected) {
            let path = entry.path();
            if path.is_dir() {
                pane.current_dir = path;
                let _ = pane.refresh();
            } else if let Ok(content) = fs::read_to_string(&path) {
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
        }
    }
}
