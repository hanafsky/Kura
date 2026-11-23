use std::fs;
use std::path::{Path, PathBuf};

use crate::app::{fs_error_message, App, Pane};
use crate::fs_utils::ensure_rename_target_free;
use crate::mode::Mode;
use crate::paste::{execute, prepare, ConflictChoice, PendingPaste, Planner};

/// Toggle mark on the selected entry in the given pane.
pub fn toggle_mark(pane: &mut Pane) {
    if pane.marked.contains(&pane.selected) {
        pane.marked.remove(&pane.selected);
    } else {
        pane.marked.insert(pane.selected);
    }
}

/// Paths to act on: the marked entries (in list order), or the entry under
/// the cursor when nothing is marked.
pub fn target_paths(app: &App) -> Vec<PathBuf> {
    let pane = app.current_pane();
    if pane.marked.is_empty() {
        return pane
            .items
            .get(pane.selected)
            .map(|e| e.path())
            .into_iter()
            .collect();
    }
    let mut idx: Vec<usize> = pane.marked.iter().copied().collect();
    idx.sort_unstable();
    idx.iter()
        .filter_map(|&i| pane.items.get(i).map(|e| e.path()))
        .collect()
}

/// Rename the entry under the cursor to `new_name` (in the same directory),
/// refresh the pane and put the cursor on the renamed entry.
pub fn rename_selected(app: &mut App, new_name: &str) {
    let pane = app.current_pane_mut();
    let Some(old) = pane.items.get(pane.selected).map(|e| e.path()) else {
        return;
    };
    let newp = old.with_file_name(new_name);
    // Unix `rename` silently replaces an existing entry; refuse instead.
    let result = ensure_rename_target_free(&old, &newp)
        .map_err(|e| (e, newp.clone()))
        .and_then(|()| fs::rename(&old, &newp).map_err(|e| (e, old.clone())));
    if pane.refresh().is_ok() {
        if let Some(pos) = pane
            .items
            .iter()
            .position(|e| e.file_name().to_string_lossy() == new_name)
        {
            pane.selected = pos;
        }
    }
    match result {
        Ok(()) => app.notify_info(format!("Renamed to {new_name}")),
        Err((e, path)) => app.notify_error(fs_error_message(&e, &path)),
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
/// truncating the source. If an entry pasted from elsewhere collides with an
/// existing name, the collision dialog (`Mode::PasteConflict`) is opened and
/// nothing is copied until every collision is answered.
pub fn paste(app: &mut App) {
    let dst_dir = app.current_pane().current_dir.clone();
    let mut items = Vec::new();
    let mut failures = Vec::new();
    for src in &app.clipboard {
        match prepare(src, &dst_dir) {
            Ok(Some(item)) => items.push(item),
            Ok(None) => {}
            Err(e) => failures.push((src.clone(), e)),
        }
    }
    let pending = PendingPaste {
        planner: Planner::new(items, dst_dir),
        failures,
    };
    continue_paste(app, pending);
}

/// Answer the current collision of the paste waiting in the dialog.
pub fn resolve_paste_conflict(app: &mut App, choice: ConflictChoice) {
    let Mode::PasteConflict { mut pending } = std::mem::replace(&mut app.mode, Mode::Filer) else {
        return;
    };
    match choice {
        ConflictChoice::Cancel => app.notify_info("Paste cancelled"),
        ConflictChoice::Resolve(res, all) => {
            pending.planner.resolve(res, all, &exists_no_follow);
            continue_paste(app, pending);
        }
    }
}

fn exists_no_follow(p: &Path) -> bool {
    fs::symlink_metadata(p).is_ok()
}

/// Plan until the next collision (then show the dialog) or run the plan.
fn continue_paste(app: &mut App, mut pending: PendingPaste) {
    if pending.planner.next_conflict(&exists_no_follow).is_some() {
        app.mode = Mode::PasteConflict { pending };
        return;
    }
    let report = execute(pending.planner.into_steps(), pending.failures);
    let _ = app.current_pane_mut().refresh();
    match report.message() {
        (true, text) => app.notify_info(text),
        (false, text) => app.notify_error(text),
    }
}

/// Delete the given files or directories from disk and refresh the pane.
pub fn delete_items(app: &mut App, items: &[PathBuf]) {
    let mut deleted = 0;
    let mut failed = 0;
    let mut first_err: Option<String> = None;
    for path in items {
        let res = if path.is_dir() {
            fs::remove_dir_all(path)
        } else {
            fs::remove_file(path)
        };
        match res {
            Ok(()) => deleted += 1,
            Err(e) => {
                failed += 1;
                first_err.get_or_insert_with(|| fs_error_message(&e, path));
            }
        }
    }
    let _ = app.current_pane_mut().refresh();
    match first_err {
        None => app.notify_info(format!("Deleted {deleted} item(s)")),
        Some(first) => app.notify_error(format!("{first} (deleted {deleted}, failed {failed})")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Paste a single entry the way `paste` does when there is no collision.
    fn paste_one(src: &Path, dst_dir: &Path) -> std::io::Result<()> {
        let item = prepare(src, dst_dir)?.expect("has a file name");
        let mut planner = Planner::new(vec![item], dst_dir.to_path_buf());
        assert!(planner.next_conflict(&exists_no_follow).is_none());
        let mut report = execute(planner.into_steps(), vec![]);
        match report.failures.pop() {
            Some((_, e)) => Err(e),
            None => Ok(()),
        }
    }

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
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
        assert!(!d.join("sub/dir").exists());
    }
}
