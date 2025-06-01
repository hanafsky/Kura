use std::fs;
use std::path::PathBuf;

use crate::app::{App, Pane};
use crate::fs_utils::copy_dir_recursively;

/// Toggle mark on the selected entry in the given pane.
pub fn toggle_mark(pane: &mut Pane) {
    if pane.marked.contains(&pane.selected) {
        pane.marked.remove(&pane.selected);
    } else {
        pane.marked.insert(pane.selected);
    }
}

/// Copy marked entries or the current entry into the clipboard.
pub fn copy_selection(app: &mut App) {
    let items = {
        let pane = app.current_pane_mut();
        let mut items = Vec::new();
        if !pane.marked.is_empty() {
            for &i in &pane.marked {
                if let Some(entry) = pane.items.get(i) {
                    items.push(entry.path());
                }
            }
        } else if let Some(entry) = pane.items.get(pane.selected) {
            items.push(entry.path());
        }
        pane.marked.clear();
        items
    };
    app.clipboard = items;
}

/// Paste clipboard entries into the current directory.
pub fn paste(app: &mut App) {
    let items = app.clipboard.clone();
    let dst_dir = app.current_pane_mut().current_dir.clone();
    for src in &items {
        if let Some(file_name) = src.file_name() {
            let dst = dst_dir.join(file_name);
            if src.is_dir() {
                if let Err(e) = copy_dir_recursively(src, &dst) {
                    eprintln!("Failed to copy directory {:?}: {}", src, e);
                }
            } else if let Err(e) = fs::copy(src, &dst) {
                eprintln!("Failed to copy file {:?}: {}", src, e);
            }
        }
    }
    let _ = app.current_pane_mut().refresh();
}

/// Delete the given files or directories from disk and refresh the pane.
pub fn delete_items(app: &mut App, items: &[PathBuf]) {
    let pane = app.current_pane_mut();
    for path in items {
        if path.is_dir() {
            if let Err(e) = fs::remove_dir_all(path) {
                eprintln!("Failed to delete directory {:?}: {}", path, e);
            }
        } else {
            if let Err(e) = fs::remove_file(path) {
                eprintln!("Failed to delete file {:?}: {}", path, e);
            }
        }
    }
    let _ = pane.refresh();
}
