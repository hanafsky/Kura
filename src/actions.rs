use std::fs;
use std::path::{Path, PathBuf};

use crate::app::{fs_error_message, App, Pane};
use crate::delete_confirm::DeleteKind;
use crate::fs_utils::ensure_rename_target_free;
use crate::mode::Mode;
use crate::paste::{execute, prepare, ConflictChoice, PendingPaste, Planner};

/// Toggle mark on the selected entry in the given pane.
pub fn toggle_mark(pane: &mut Pane) {
    let Some(path) = pane.selected_path() else {
        return;
    };
    if !pane.marked.remove(&path) {
        pane.marked.insert(path);
    }
}

/// Paths to act on: the marked entries (in list order), or the entry under
/// the cursor when nothing is marked.
pub fn target_paths(app: &App) -> Vec<PathBuf> {
    let pane = app.current_pane();
    if pane.marked.is_empty() {
        return pane.selected_path().into_iter().collect();
    }
    pane.items
        .iter()
        .map(|e| e.path())
        .filter(|p| pane.marked.contains(p))
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
    let items = target_paths(app);
    app.current_pane_mut().marked.clear();
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

/// Remove the given files or directories and refresh the pane.
///
/// [`DeleteKind::Normal`] moves the items to the OS trash. If that fails
/// (e.g. no usable trash on WSL or some Linux setups) the error is reported
/// and the item is left in place; it never falls back to permanent deletion.
/// [`DeleteKind::Permanent`] removes the items from disk.
pub fn delete_items(app: &mut App, items: &[PathBuf], kind: DeleteKind) {
    let mut done = 0;
    let mut failed = 0;
    let mut first_err: Option<String> = None;
    let trash_ctx = trash_context();
    for path in items {
        let res = match kind {
            DeleteKind::Normal => move_to_trash(&trash_ctx, path),
            DeleteKind::Permanent => remove_from_disk(path).map_err(|e| fs_error_message(&e, path)),
        };
        match res {
            Ok(()) => done += 1,
            Err(msg) => {
                failed += 1;
                first_err.get_or_insert(msg);
            }
        }
    }
    let _ = app.current_pane_mut().refresh();
    match removal_message(kind, done, failed, first_err) {
        (true, text) => app.notify_info(text),
        (false, text) => app.notify_error(text),
    }
}

fn remove_from_disk(path: &Path) -> std::io::Result<()> {
    if path.is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    }
}

/// Trash context used by `x`.
///
/// On macOS the `NSFileManager` method is used instead of the default Finder
/// (AppleScript) method: it needs no Automation permission, is silent and does
/// not start one `osascript` per item.
fn trash_context() -> trash::TrashContext {
    #[allow(unused_mut)]
    let mut ctx = trash::TrashContext::default();
    #[cfg(target_os = "macos")]
    {
        use trash::macos::{DeleteMethod, TrashContextExtMacos};
        ctx.set_delete_method(DeleteMethod::NsFileManager);
    }
    ctx
}

/// Move one entry to the trash, returning a status line message on failure.
fn move_to_trash(ctx: &trash::TrashContext, path: &Path) -> Result<(), String> {
    // Report a missing / inaccessible entry the same way `X` does.
    fs::symlink_metadata(path).map_err(|e| fs_error_message(&e, path))?;
    ctx.delete(path).map_err(|e| trash_error_message(&e, path))
}

/// Short, single-line message for a failed move to the trash.
fn trash_error_message(err: &trash::Error, path: &Path) -> String {
    let name = display_name(path);
    let reason = match err {
        trash::Error::Os { description, .. } | trash::Error::Unknown { description } => {
            short_reason(description)
        }
        #[cfg(all(
            unix,
            not(target_os = "macos"),
            not(target_os = "ios"),
            not(target_os = "android")
        ))]
        trash::Error::FileSystem { path: p, source } => {
            return format!("Cannot move to Trash: {}", fs_error_message(source, p));
        }
        trash::Error::TargetedRoot => "cannot trash the root directory".to_string(),
        _ => String::new(),
    };
    if reason.is_empty() {
        format!("Cannot move to Trash: {name}")
    } else {
        format!("Cannot move to Trash ({reason}): {name}")
    }
}

/// Strip the noisy prefix of a platform error description
/// (AppleScript `execution error: ...`, NSFileManager `... failed: ...`).
fn short_reason(description: &str) -> String {
    let tail = ["execution error: ", "failed: "]
        .iter()
        .filter_map(|m| description.rfind(m).map(|i| &description[i + m.len()..]))
        .min_by_key(|t| t.len())
        .unwrap_or(description);
    tail.lines().next().unwrap_or("").trim().to_string()
}

fn display_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// Status line text for a finished removal: `(success, text)`.
fn removal_message(
    kind: DeleteKind,
    done: usize,
    failed: usize,
    first_err: Option<String>,
) -> (bool, String) {
    match (kind, first_err) {
        (DeleteKind::Normal, None) => (true, format!("Moved {done} item(s) to Trash")),
        (DeleteKind::Permanent, None) => (true, format!("Deleted {done} item(s)")),
        (DeleteKind::Normal, Some(first)) => {
            (false, format!("{first} (moved {done}, failed {failed})"))
        }
        (DeleteKind::Permanent, Some(first)) => {
            (false, format!("{first} (deleted {done}, failed {failed})"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trash_errors_are_shortened() {
        let p = Path::new("/tmp/dir/a.txt");
        let finder = trash::Error::Os {
            code: 1,
            description: "The AppleScript exited with error. stderr: 29:142: execution error: Finderでエラーが起きました: 必要なアクセス権がないため、操作は完了できません。 (-5000)\n".into(),
        };
        assert_eq!(
            trash_error_message(&finder, p),
            "Cannot move to Trash (Finderでエラーが起きました: 必要なアクセス権がないため、操作は完了できません。 (-5000)): a.txt"
        );
        let ns = trash::Error::Unknown {
            description: "While deleting '\"/tmp/dir/a.txt\"', `trashItemAtURL` failed: The file “a.txt” doesn’t exist.".into(),
        };
        assert_eq!(
            trash_error_message(&ns, p),
            "Cannot move to Trash (The file “a.txt” doesn’t exist.): a.txt"
        );
        assert_eq!(
            trash_error_message(&trash::Error::CouldNotAccess { target: "x".into() }, p),
            "Cannot move to Trash: a.txt"
        );
    }

    #[test]
    fn missing_entry_is_reported_without_touching_trash() {
        let tmp = tempfile::tempdir().unwrap();
        let missing = tmp.path().join("gone.txt");
        let msg = move_to_trash(&trash_context(), &missing).unwrap_err();
        assert!(msg.ends_with(": gone.txt"), "{msg}");
        assert!(!msg.contains("Trash"), "{msg}");
    }

    #[test]
    fn removal_messages() {
        assert_eq!(
            removal_message(DeleteKind::Normal, 3, 0, None),
            (true, "Moved 3 item(s) to Trash".to_string())
        );
        assert_eq!(
            removal_message(DeleteKind::Permanent, 2, 0, None),
            (true, "Deleted 2 item(s)".to_string())
        );
        assert_eq!(
            removal_message(DeleteKind::Normal, 1, 2, Some("boom: a".into())),
            (false, "boom: a (moved 1, failed 2)".to_string())
        );
        assert_eq!(
            removal_message(DeleteKind::Permanent, 0, 1, Some("boom: b".into())),
            (false, "boom: b (deleted 0, failed 1)".to_string())
        );
    }

    #[test]
    fn remove_from_disk_handles_files_and_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("f.txt");
        fs::write(&f, "x").unwrap();
        let d = tmp.path().join("d");
        fs::create_dir(&d).unwrap();
        fs::write(d.join("inner"), "y").unwrap();
        remove_from_disk(&f).unwrap();
        remove_from_disk(&d).unwrap();
        assert!(!f.exists());
        assert!(!d.exists());
    }

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
