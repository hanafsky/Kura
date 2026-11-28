use std::path::PathBuf;

use crate::delete_confirm::DeleteKind;

#[derive(PartialEq)]
pub enum PaneType {
    Left,
    Right,
}

pub enum Mode {
    Filer,
    /// Visual multi-selection mode. The anchor is the path of the entry
    /// where the selection started, so it survives refreshes.
    Visual {
        anchor: PathBuf,
    },
    Viewer {
        /// Rendered text (title, lines); see [`crate::text_view`].
        doc: crate::text_view::TextDoc,
        offset: usize,
        /// Scrolled to the end (`G`, or `j` / a page down reaching it):
        /// the draw keeps the last row at the bottom when the size
        /// changes. Moving up clears it.
        at_end: bool,
        /// Showing `git diff` output (`D`): lines are coloured and
        /// `q` / `Esc` close the viewer instead of quitting.
        diff: bool,
    },
    ConfirmDelete {
        items: Vec<PathBuf>,
        kind: DeleteKind,
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
    /// Image/video preview shown in the pane opposite to the active one
    /// (content lives in `App::preview`).
    Preview,
    /// Paste collision dialog: an entry with the same name already exists
    /// in the destination (Overwrite / Skip / Keep both).
    PasteConflict {
        pending: crate::paste::PendingPaste,
    },
    /// Files the coding agent changed (`A`), listed in the active pane
    /// newest first; see [`crate::agent_changes`].
    Changes {
        items: Vec<PathBuf>,
        selected: usize,
    },
    /// Git worktrees of the active pane's repository (`W`), listed in the
    /// active pane; see [`crate::worktrees`].
    Worktrees {
        items: Vec<crate::worktrees::Worktree>,
        selected: usize,
    },
    /// Choose the agent pane to send `@path` references to (`Y` in herdr
    /// with several agents).
    SendTarget {
        targets: Vec<crate::ade::Agent>,
        selected: usize,
        /// The entries to send.
        paths: Vec<PathBuf>,
        /// Where the dialog was opened (the file list or the agent
        /// changes list), restored when it closes.
        back: Box<Mode>,
    },
}
