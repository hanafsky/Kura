//! Paste planning and execution.
//!
//! Pasting is split into three phases so the decision logic can be tested
//! without a terminal:
//!
//! 1. [`prepare`] inspects each clipboard entry on disk.
//! 2. [`Planner`] walks the entries, detects name collisions in the
//!    destination and turns each entry into a [`Step`]. Collisions are
//!    resolved one at a time by the user (Overwrite / Skip / Keep both,
//!    optionally "apply to all remaining"). The planner only touches the
//!    filesystem through an injected `exists` function.
//! 3. [`execute`] runs the steps and collects a per-item [`PasteReport`].

use std::collections::HashSet;
use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crossterm::event::KeyCode;

use crate::app::fs_error_message;
use crate::fs_utils::{copy_entry, is_same_location, paste_destination};

/// How to handle an entry whose name is already taken in the destination.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Resolution {
    /// Replace the existing entry with the pasted one.
    Overwrite,
    /// Leave the existing entry alone and do not paste this one.
    Skip,
    /// Paste under a free `name (copy).ext` / `name (copy n).ext` name.
    KeepBoth,
}

/// A clipboard entry, inspected and ready to be planned.
#[derive(Clone, Debug)]
pub struct PasteItem {
    pub src: PathBuf,
    pub file_name: OsString,
    pub is_dir: bool,
    /// The source already lives at `dst_dir/file_name` (pasting onto itself).
    pub same_location: bool,
}

/// What to do with one entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    /// Copy to a destination that is free.
    Copy { src: PathBuf, dst: PathBuf },
    /// Replace the existing entry at `dst`.
    Overwrite { src: PathBuf, dst: PathBuf },
    /// Do nothing.
    Skip { src: PathBuf },
}

/// Inspect a clipboard entry. Returns `Ok(None)` for paths without a file
/// name (such as `/`), which cannot be pasted and are ignored.
pub fn prepare(src: &Path, dst_dir: &Path) -> io::Result<Option<PasteItem>> {
    let Some(file_name) = src.file_name() else {
        return Ok(None);
    };
    let is_dir = fs::symlink_metadata(src)?.is_dir();
    let same_location = is_same_location(src, dst_dir)?;
    Ok(Some(PasteItem {
        src: src.to_path_buf(),
        file_name: file_name.to_os_string(),
        is_dir,
        same_location,
    }))
}

/// Turns [`PasteItem`]s into [`Step`]s, stopping at each collision that
/// needs a decision from the user.
///
/// Usage: call [`Planner::next_conflict`]; while it returns an item, ask the
/// user and pass the answer to [`Planner::resolve`]. When it returns `None`
/// every item is planned and [`Planner::into_steps`] gives the plan.
#[derive(Debug)]
pub struct Planner {
    items: Vec<PasteItem>,
    dst_dir: PathBuf,
    /// Index of the next item to plan.
    next: usize,
    /// Destinations already used by earlier items of this batch.
    claimed: HashSet<PathBuf>,
    steps: Vec<Step>,
    /// Set by "apply to all remaining": resolves later collisions silently.
    apply_all: Option<Resolution>,
}

impl Planner {
    pub fn new(items: Vec<PasteItem>, dst_dir: PathBuf) -> Self {
        Self {
            items,
            dst_dir,
            next: 0,
            claimed: HashSet::new(),
            steps: Vec::new(),
            apply_all: None,
        }
    }

    /// Plan items until one collides with an existing entry (on disk or
    /// claimed earlier in this batch) and no "apply to all" answer is set.
    /// Returns that item, or `None` once every item is planned.
    pub fn next_conflict(&mut self, exists: &dyn Fn(&Path) -> bool) -> Option<&PasteItem> {
        while let Some(item) = self.items.get(self.next) {
            if item.same_location {
                // Pasting onto itself always keeps both; no question needed.
                self.plan_current(Resolution::KeepBoth, exists);
                continue;
            }
            let target = self.dst_dir.join(&item.file_name);
            let taken = exists(&target) || self.claimed.contains(&target);
            match (taken, self.apply_all) {
                (false, _) => {
                    self.claimed.insert(target.clone());
                    self.steps.push(Step::Copy {
                        src: item.src.clone(),
                        dst: target,
                    });
                    self.next += 1;
                }
                (true, Some(res)) => self.plan_current(res, exists),
                (true, None) => return self.items.get(self.next),
            }
        }
        None
    }

    /// Answer the collision returned by the last [`Planner::next_conflict`].
    /// With `apply_to_rest`, later collisions get the same answer.
    pub fn resolve(
        &mut self,
        res: Resolution,
        apply_to_rest: bool,
        exists: &dyn Fn(&Path) -> bool,
    ) {
        if self.next >= self.items.len() {
            return;
        }
        if apply_to_rest {
            self.apply_all = Some(res);
        }
        self.plan_current(res, exists);
    }

    /// The item currently waiting for a decision, if any.
    pub fn current(&self) -> Option<&PasteItem> {
        self.items.get(self.next)
    }

    /// Position of the item currently waiting for a decision (1-based) and
    /// the number of items in the batch, for the dialog.
    pub fn progress(&self) -> (usize, usize) {
        (self.next + 1, self.items.len())
    }

    pub fn into_steps(self) -> Vec<Step> {
        self.steps
    }

    fn plan_current(&mut self, res: Resolution, exists: &dyn Fn(&Path) -> bool) {
        let item = &self.items[self.next];
        let step = match res {
            Resolution::Skip => Step::Skip {
                src: item.src.clone(),
            },
            Resolution::Overwrite => {
                let dst = self.dst_dir.join(&item.file_name);
                self.claimed.insert(dst.clone());
                Step::Overwrite {
                    src: item.src.clone(),
                    dst,
                }
            }
            Resolution::KeepBoth => {
                let claimed = &self.claimed;
                let dst =
                    paste_destination(&item.file_name, &self.dst_dir, item.is_dir, true, |p| {
                        exists(p) || claimed.contains(p)
                    });
                self.claimed.insert(dst.clone());
                Step::Copy {
                    src: item.src.clone(),
                    dst,
                }
            }
        };
        self.steps.push(step);
        self.next += 1;
    }
}

/// A paste waiting for the user's answers in the collision dialog.
#[derive(Debug)]
pub struct PendingPaste {
    pub planner: Planner,
    /// Entries that could not even be inspected (e.g. deleted meanwhile).
    pub failures: Vec<(PathBuf, io::Error)>,
}

/// What a key press in the collision dialog means.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ConflictChoice {
    /// Resolve the current collision; `true` = also for all remaining ones.
    Resolve(Resolution, bool),
    /// Abort the whole paste without copying anything.
    Cancel,
}

/// Key bindings of the collision dialog. Lower case answers for this item,
/// upper case applies the answer to all remaining collisions.
pub fn conflict_choice(code: KeyCode) -> Option<ConflictChoice> {
    let (res, all) = match code {
        KeyCode::Char('o') => (Resolution::Overwrite, false),
        KeyCode::Char('O') => (Resolution::Overwrite, true),
        KeyCode::Char('s') => (Resolution::Skip, false),
        KeyCode::Char('S') => (Resolution::Skip, true),
        KeyCode::Char('b') => (Resolution::KeepBoth, false),
        KeyCode::Char('B') => (Resolution::KeepBoth, true),
        KeyCode::Esc => return Some(ConflictChoice::Cancel),
        _ => return None,
    };
    Some(ConflictChoice::Resolve(res, all))
}

/// Outcome of a paste, item by item.
#[derive(Debug, Default)]
pub struct PasteReport {
    pub pasted: usize,
    pub skipped: usize,
    pub failures: Vec<(PathBuf, io::Error)>,
}

impl PasteReport {
    /// Status line text. On failure: `reason: item (pasted N, failed M)`,
    /// naming the first item that failed.
    pub fn message(&self) -> (bool, String) {
        let skipped = if self.skipped > 0 {
            format!(", skipped {}", self.skipped)
        } else {
            String::new()
        };
        match self.failures.first() {
            None => (true, format!("Pasted {} item(s){skipped}", self.pasted)),
            Some((src, err)) => (
                false,
                format!(
                    "{} (pasted {}, failed {}{skipped})",
                    fs_error_message(err, src),
                    self.pasted,
                    self.failures.len()
                ),
            ),
        }
    }
}

/// Run a plan. Every step is attempted; failures are collected per item.
pub fn execute(steps: Vec<Step>, mut failures: Vec<(PathBuf, io::Error)>) -> PasteReport {
    let mut report = PasteReport::default();
    for step in steps {
        let (src, res) = match step {
            Step::Skip { .. } => {
                report.skipped += 1;
                continue;
            }
            Step::Copy { src, dst } => {
                let res = copy_to_free(&src, &dst);
                (src, res)
            }
            Step::Overwrite { src, dst } => {
                let res = overwrite_entry(&src, &dst);
                (src, res)
            }
        };
        match res {
            Ok(()) => report.pasted += 1,
            Err(e) => failures.push((src, e)),
        }
    }
    report.failures = failures;
    report
}

fn exists_no_follow(p: &Path) -> bool {
    fs::symlink_metadata(p).is_ok()
}

/// Copy to a destination that was free when planned. Refuses instead of
/// silently replacing if something appeared there in the meantime.
fn copy_to_free(src: &Path, dst: &Path) -> io::Result<()> {
    if exists_no_follow(dst) {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "already exists",
        ));
    }
    copy_entry(src, dst)
}

/// Absolute form of `path` without resolving its last component, so a
/// symlink is identified by its own location rather than its target.
fn absolute_no_follow(path: &Path) -> io::Result<PathBuf> {
    let parent = path.parent().filter(|p| !p.as_os_str().is_empty());
    let parent = fs::canonicalize(parent.unwrap_or(Path::new(".")))?;
    Ok(match path.file_name() {
        Some(name) => parent.join(name),
        None => parent,
    })
}

/// A name next to `dst` that is not in use, for staging a copy.
fn staging_name(dst: &Path, tag: &str) -> PathBuf {
    let name = dst.file_name().unwrap_or_default().to_string_lossy();
    (0..)
        .map(|n| dst.with_file_name(format!(".{name}.kura-{tag}-{}-{n}", std::process::id())))
        .find(|p| !exists_no_follow(p))
        .expect("an unused staging name always exists")
}

fn remove_entry(path: &Path) -> io::Result<()> {
    let meta = fs::symlink_metadata(path)?;
    if meta.is_dir() {
        fs::remove_dir_all(path)
    } else {
        // Files and symlinks (the link itself, never its target).
        fs::remove_file(path)
    }
}

/// Replace the existing entry at `dst` with a copy of `src`.
///
/// A directory is replaced as a whole (its old contents are not merged).
/// The copy is made under a temporary name first and swapped in with
/// renames, so the existing entry survives if copying fails.
pub fn overwrite_entry(src: &Path, dst: &Path) -> io::Result<()> {
    let src_abs = absolute_no_follow(src)?;
    let dst_abs = absolute_no_follow(dst)?;
    if src_abs == dst_abs {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "cannot overwrite an item with itself",
        ));
    }
    if src_abs.starts_with(&dst_abs) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "cannot overwrite a folder that contains the source",
        ));
    }

    let staged = staging_name(dst, "new");
    if let Err(e) = copy_entry(src, &staged) {
        let _ = remove_entry(&staged);
        return Err(e);
    }
    let backup = staging_name(dst, "old");
    if let Err(e) = fs::rename(dst, &backup) {
        let _ = remove_entry(&staged);
        return Err(e);
    }
    if let Err(e) = fs::rename(&staged, dst) {
        let _ = fs::rename(&backup, dst);
        let _ = remove_entry(&staged);
        return Err(e);
    }
    remove_entry(&backup)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    fn item(src: &str, is_dir: bool, same: bool) -> PasteItem {
        let src = PathBuf::from(src);
        PasteItem {
            file_name: src.file_name().unwrap().to_os_string(),
            src,
            is_dir,
            same_location: same,
        }
    }

    fn on_disk(names: &'static [&'static str]) -> impl Fn(&Path) -> bool {
        move |p: &Path| names.iter().any(|n| Path::new("/dst").join(n) == p)
    }

    fn copy(src: &str, dst: &str) -> Step {
        Step::Copy {
            src: src.into(),
            dst: dst.into(),
        }
    }

    #[test]
    fn no_conflicts_plans_plain_copies() {
        let exists = on_disk(&[]);
        let mut p = Planner::new(
            vec![item("/a/x.txt", false, false), item("/a/d", true, false)],
            "/dst".into(),
        );
        assert!(p.next_conflict(&exists).is_none());
        assert_eq!(
            p.into_steps(),
            vec![copy("/a/x.txt", "/dst/x.txt"), copy("/a/d", "/dst/d")]
        );
    }

    #[test]
    fn conflict_stops_and_each_answer_is_applied() {
        let exists = on_disk(&["a.txt", "b.txt", "c.txt"]);
        let mut p = Planner::new(
            vec![
                item("/s/a.txt", false, false),
                item("/s/b.txt", false, false),
                item("/s/c.txt", false, false),
                item("/s/new.txt", false, false),
            ],
            "/dst".into(),
        );
        assert_eq!(p.next_conflict(&exists).unwrap().src, Path::new("/s/a.txt"));
        p.resolve(Resolution::Overwrite, false, &exists);
        assert_eq!(p.next_conflict(&exists).unwrap().src, Path::new("/s/b.txt"));
        assert_eq!(p.progress(), (2, 4));
        p.resolve(Resolution::Skip, false, &exists);
        assert_eq!(p.next_conflict(&exists).unwrap().src, Path::new("/s/c.txt"));
        p.resolve(Resolution::KeepBoth, false, &exists);
        assert!(p.next_conflict(&exists).is_none());
        assert_eq!(
            p.into_steps(),
            vec![
                Step::Overwrite {
                    src: "/s/a.txt".into(),
                    dst: "/dst/a.txt".into()
                },
                Step::Skip {
                    src: "/s/b.txt".into()
                },
                copy("/s/c.txt", "/dst/c (copy).txt"),
                copy("/s/new.txt", "/dst/new.txt"),
            ]
        );
    }

    #[test]
    fn apply_to_all_resolves_remaining_conflicts_silently() {
        let exists = on_disk(&["a", "b", "c"]);
        let mut p = Planner::new(
            vec![
                item("/s/a", false, false),
                item("/s/b", false, false),
                item("/s/c", false, false),
            ],
            "/dst".into(),
        );
        assert!(p.next_conflict(&exists).is_some());
        p.resolve(Resolution::Skip, true, &exists);
        assert!(p.next_conflict(&exists).is_none());
        assert!(p
            .into_steps()
            .iter()
            .all(|s| matches!(s, Step::Skip { .. })));
    }

    #[test]
    fn keep_both_avoids_names_on_disk_and_in_batch() {
        // Same name from two different directories, and "a (copy)" exists.
        let exists = on_disk(&["a", "a (copy)"]);
        let mut p = Planner::new(
            vec![item("/s1/a", false, false), item("/s2/a", false, false)],
            "/dst".into(),
        );
        assert!(p.next_conflict(&exists).is_some());
        p.resolve(Resolution::KeepBoth, true, &exists);
        assert!(p.next_conflict(&exists).is_none());
        assert_eq!(
            p.into_steps(),
            vec![
                copy("/s1/a", "/dst/a (copy 2)"),
                copy("/s2/a", "/dst/a (copy 3)"),
            ]
        );
    }

    #[test]
    fn same_name_twice_in_batch_conflicts_with_itself() {
        let exists = on_disk(&[]);
        let mut p = Planner::new(
            vec![item("/s1/a", false, false), item("/s2/a", false, false)],
            "/dst".into(),
        );
        let second = p.next_conflict(&exists).unwrap();
        assert_eq!(second.src, Path::new("/s2/a"));
        p.resolve(Resolution::KeepBoth, false, &exists);
        assert_eq!(
            p.into_steps(),
            vec![copy("/s1/a", "/dst/a"), copy("/s2/a", "/dst/a (copy)")]
        );
    }

    #[test]
    fn same_location_keeps_both_without_asking() {
        let exists = on_disk(&["a.txt"]);
        let mut p = Planner::new(vec![item("/dst/a.txt", false, true)], "/dst".into());
        assert!(p.next_conflict(&exists).is_none());
        assert_eq!(
            p.into_steps(),
            vec![copy("/dst/a.txt", "/dst/a (copy).txt")]
        );
    }

    #[test]
    fn same_location_numbering_avoids_batch_names() {
        let exists = on_disk(&["a.txt"]);
        let mut p = Planner::new(
            vec![
                item("/dst/a.txt", false, true),
                item("/dst/a.txt", false, true),
            ],
            "/dst".into(),
        );
        assert!(p.next_conflict(&exists).is_none());
        assert_eq!(
            p.into_steps(),
            vec![
                copy("/dst/a.txt", "/dst/a (copy).txt"),
                copy("/dst/a.txt", "/dst/a (copy 2).txt"),
            ]
        );
    }

    #[test]
    fn conflict_keys() {
        use ConflictChoice::*;
        use Resolution::*;
        assert_eq!(
            conflict_choice(KeyCode::Char('o')),
            Some(Resolve(Overwrite, false))
        );
        assert_eq!(
            conflict_choice(KeyCode::Char('S')),
            Some(Resolve(Skip, true))
        );
        assert_eq!(
            conflict_choice(KeyCode::Char('b')),
            Some(Resolve(KeepBoth, false))
        );
        assert_eq!(conflict_choice(KeyCode::Esc), Some(Cancel));
        assert_eq!(conflict_choice(KeyCode::Char('x')), None);
    }

    #[test]
    fn report_message_names_failed_item_and_counts() {
        let report = PasteReport {
            pasted: 2,
            skipped: 0,
            failures: vec![(
                PathBuf::from("/very/long/path/broken.txt"),
                io::Error::new(io::ErrorKind::NotFound, "No such file or directory"),
            )],
        };
        assert_eq!(
            report.message(),
            (
                false,
                "No such file or directory: broken.txt (pasted 2, failed 1)".to_string()
            )
        );
        let ok = PasteReport {
            pasted: 3,
            skipped: 1,
            failures: vec![],
        };
        assert_eq!(
            ok.message(),
            (true, "Pasted 3 item(s), skipped 1".to_string())
        );
    }

    #[test]
    fn execute_overwrites_skips_and_reports_each_item() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        let dst = tmp.path().join("dst");
        fs::create_dir_all(&src).unwrap();
        fs::create_dir_all(&dst).unwrap();
        fs::write(src.join("a.txt"), "new").unwrap();
        fs::write(src.join("b.txt"), "new").unwrap();
        fs::write(dst.join("a.txt"), "old").unwrap();
        fs::write(dst.join("b.txt"), "old").unwrap();

        let steps = vec![
            Step::Overwrite {
                src: src.join("a.txt"),
                dst: dst.join("a.txt"),
            },
            Step::Skip {
                src: src.join("b.txt"),
            },
            Step::Copy {
                src: src.join("missing.txt"),
                dst: dst.join("missing.txt"),
            },
        ];
        let report = execute(steps, vec![]);
        assert_eq!(fs::read_to_string(dst.join("a.txt")).unwrap(), "new");
        assert_eq!(fs::read_to_string(dst.join("b.txt")).unwrap(), "old");
        assert_eq!((report.pasted, report.skipped), (1, 1));
        assert_eq!(report.failures.len(), 1);
        assert_eq!(report.failures[0].0, src.join("missing.txt"));
        // No staging leftovers.
        assert_eq!(fs::read_dir(&dst).unwrap().count(), 2);
    }

    #[test]
    fn copy_step_refuses_to_replace_entry_that_appeared() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("s"), "new").unwrap();
        fs::write(tmp.path().join("d"), "old").unwrap();
        let report = execute(
            vec![Step::Copy {
                src: tmp.path().join("s"),
                dst: tmp.path().join("d"),
            }],
            vec![],
        );
        assert_eq!(report.failures.len(), 1);
        assert_eq!(fs::read_to_string(tmp.path().join("d")).unwrap(), "old");
    }

    #[test]
    fn overwrite_replaces_directory_as_a_whole() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src/d");
        let dst = tmp.path().join("dst/d");
        fs::create_dir_all(&src).unwrap();
        fs::create_dir_all(&dst).unwrap();
        fs::write(src.join("new"), "1").unwrap();
        fs::write(dst.join("old"), "1").unwrap();
        overwrite_entry(&src, &dst).unwrap();
        assert!(dst.join("new").exists());
        assert!(!dst.join("old").exists());
        assert_eq!(fs::read_dir(tmp.path().join("dst")).unwrap().count(), 1);
    }

    #[test]
    fn overwrite_file_with_directory_and_back() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir_all(tmp.path().join("s/x")).unwrap();
        fs::create_dir_all(tmp.path().join("d")).unwrap();
        fs::write(tmp.path().join("d/x"), "file").unwrap();
        overwrite_entry(&tmp.path().join("s/x"), &tmp.path().join("d/x")).unwrap();
        assert!(tmp.path().join("d/x").is_dir());

        fs::create_dir_all(tmp.path().join("s2")).unwrap();
        fs::write(tmp.path().join("s2/x"), "file").unwrap();
        overwrite_entry(&tmp.path().join("s2/x"), &tmp.path().join("d/x")).unwrap();
        assert_eq!(fs::read_to_string(tmp.path().join("d/x")).unwrap(), "file");
    }

    #[test]
    fn overwrite_folder_containing_source_is_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        // Paste /t/sub/inner/sub into /t: the target /t/sub contains the source.
        let src = tmp.path().join("sub/inner/sub");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("f"), "keep").unwrap();
        let err = overwrite_entry(&src, &tmp.path().join("sub")).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        assert_eq!(fs::read_to_string(src.join("f")).unwrap(), "keep");
    }

    #[test]
    fn overwrite_symlink_replaces_link_not_target() {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("target.txt");
        fs::write(&target, "target").unwrap();
        fs::create_dir(tmp.path().join("d")).unwrap();
        symlink(&target, tmp.path().join("d/x")).unwrap();
        fs::create_dir(tmp.path().join("s")).unwrap();
        fs::write(tmp.path().join("s/x"), "new").unwrap();
        overwrite_entry(&tmp.path().join("s/x"), &tmp.path().join("d/x")).unwrap();
        assert_eq!(fs::read_to_string(&target).unwrap(), "target");
        let meta = fs::symlink_metadata(tmp.path().join("d/x")).unwrap();
        assert!(meta.is_file());
        assert_eq!(fs::read_to_string(tmp.path().join("d/x")).unwrap(), "new");
    }

    #[test]
    fn failed_overwrite_keeps_existing_entry() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir(tmp.path().join("d")).unwrap();
        fs::write(tmp.path().join("d/x"), "old").unwrap();
        let err = overwrite_entry(&tmp.path().join("missing/x"), &tmp.path().join("d/x"));
        assert!(err.is_err());
        assert_eq!(fs::read_to_string(tmp.path().join("d/x")).unwrap(), "old");
        assert_eq!(fs::read_dir(tmp.path().join("d")).unwrap().count(), 1);
    }
}
