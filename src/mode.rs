use std::path::PathBuf;

#[derive(PartialEq)]
pub enum PaneType {
    Left,
    Right,
}

pub enum Mode {
    Filer,
    /// Visual multi-selection mode (anchor index for selection range)
    Visual {
        anchor: usize,
    },
    Viewer {
        content: String,
        title: String,
        offset: u16,
    },
    ConfirmDelete {
        items: Vec<PathBuf>,
    },
    /// Search mode: prompt for a query and jump to matching entries
    Search {
        query: String,
    },
    /// Rename mode: inline editing of the selected filename
    Rename {
        original: String,
        buffer: String,
    },
    /// Sort mode: choose a sort order for the file list
    Sort {
        selected: usize,
    },
}
