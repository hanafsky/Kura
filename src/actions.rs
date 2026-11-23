use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::app::{App, Pane};
use crate::fs_utils::{copy_entry, is_same_location, paste_destination};

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
    let n = items.len();
    app.clipboard = items;
    app.notify_info(format!("Copied {n} item(s) to clipboard"));
}

/// Paste clipboard entries into the current directory.
///
/// Pasting onto the entry's own location creates `name (copy).ext` instead of
/// truncating the source. Every item is attempted; the first error is returned.
pub fn paste(app: &mut App) -> io::Result<()> {
    let items = app.clipboard.clone();
    let dst_dir = app.current_pane_mut().current_dir.clone();
    let mut first_err = None;
    for src in &items {
        if let Err(e) = paste_one(src, &dst_dir) {
            first_err.get_or_insert(e);
        }
    }
    let _ = app.current_pane_mut().refresh();
    first_err.map_or(Ok(()), Err)
}

fn paste_one(src: &Path, dst_dir: &Path) -> io::Result<()> {
    let Some(file_name) = src.file_name() else {
        return Ok(());
    };
    let is_dir = fs::symlink_metadata(src)?.is_dir();
    let same = is_same_location(src, dst_dir)?;
    let dst = paste_destination(file_name, dst_dir, is_dir, same, |p| {
        fs::symlink_metadata(p).is_ok()
    });
    copy_entry(src, &dst)
}

/// Delete the given files or directories from disk and refresh the pane.
pub fn delete_items(app: &mut App, items: &[PathBuf]) {
    let mut deleted = 0;
    let mut errors = Vec::new();
    for path in items {
        let res = if path.is_dir() {
            fs::remove_dir_all(path)
        } else {
            fs::remove_file(path)
        };
        match res {
            Ok(()) => deleted += 1,
            Err(e) => errors.push(format!("{}: {e}", path.display())),
        }
    }
    let _ = app.current_pane_mut().refresh();
    match errors.first() {
        None => app.notify_info(format!("Deleted {deleted} item(s)")),
        Some(first) => app.notify_error(format!(
            "Deleted {deleted} item(s), failed {}: {first}",
            errors.len()
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paste_into_same_dir_creates_numbered_copies() {
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("a.txt");
        fs::write(&f, "hello").unwrap();
        paste_one(&f, tmp.path()).unwrap();
        paste_one(&f, tmp.path()).unwrap();
        assert_eq!(fs::read_to_string(&f).unwrap(), "hello");
        let c1 = tmp.path().join("a (copy).txt");
        let c2 = tmp.path().join("a (copy 2).txt");
        assert_eq!(fs::read_to_string(c1).unwrap(), "hello");
        assert_eq!(fs::read_to_string(c2).unwrap(), "hello");
    }

    #[test]
    fn paste_same_dir_via_different_spelling_is_detected() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir(tmp.path().join("sub")).unwrap();
        let f = tmp.path().join("a.txt");
        fs::write(&f, "hello").unwrap();
        let dst_dir = tmp.path().join("sub").join("..");
        paste_one(&f, &dst_dir).unwrap();
        assert_eq!(fs::read_to_string(&f).unwrap(), "hello");
        assert!(tmp.path().join("a (copy).txt").exists());
    }

    #[test]
    fn paste_dir_into_same_dir_creates_copy() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path().join("dir");
        fs::create_dir(&d).unwrap();
        fs::write(d.join("f"), "x").unwrap();
        paste_one(&d, tmp.path()).unwrap();
        assert_eq!(
            fs::read_to_string(tmp.path().join("dir (copy)/f")).unwrap(),
            "x"
        );
        assert_eq!(fs::read_dir(&d).unwrap().count(), 1);
    }

    #[test]
    fn paste_dir_into_its_own_child_fails() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path().join("dir");
        fs::create_dir_all(d.join("sub")).unwrap();
        let err = paste_one(&d, &d.join("sub")).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        assert!(!d.join("sub/dir").exists());
    }
}
