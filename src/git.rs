//! Git status for the directory shown in a pane.
//!
//! `git status --porcelain=v2 --branch -z` is run once when a pane loads a
//! directory and the result is cached in the pane. Parsing and path lookup
//! are pure functions so they can be tested without git or a terminal.

use std::{
    collections::{HashMap, HashSet},
    path::Path,
    process::{Command, Stdio},
};

/// Status of one entry, as shown in front of its name.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FileStatus {
    Modified,
    Added,
    Deleted,
    Renamed,
    Conflicted,
    Untracked,
    Ignored,
    /// A directory that contains changed (non-ignored) entries.
    DirtyDir,
}

impl FileStatus {
    pub fn symbol(self) -> char {
        match self {
            FileStatus::Modified => 'M',
            FileStatus::Added => 'A',
            FileStatus::Deleted => 'D',
            FileStatus::Renamed => 'R',
            FileStatus::Conflicted => 'U',
            FileStatus::Untracked => '?',
            FileStatus::Ignored => '!',
            FileStatus::DirtyDir => '•',
        }
    }

    /// Ordering used when the same path appears in several records
    /// (e.g. `D.` and `?` after `git rm --cached`): a tracked change wins.
    fn rank(self) -> u8 {
        match self {
            FileStatus::Conflicted => 6,
            FileStatus::Deleted => 5,
            FileStatus::Renamed => 4,
            FileStatus::Added => 3,
            FileStatus::Modified => 2,
            FileStatus::Untracked => 1,
            FileStatus::Ignored | FileStatus::DirtyDir => 0,
        }
    }

    /// Whether the entry counts as a change (for `•` and the changed-only filter).
    pub fn is_change(self) -> bool {
        self != FileStatus::Ignored
    }
}

/// Parsed `git status` output for one directory.
#[derive(Debug, Default, PartialEq)]
pub struct GitStatus {
    /// Branch name, or `None` for a detached HEAD.
    pub branch: Option<String>,
    /// Path of the shown directory relative to the repository root, with a
    /// trailing `/` (empty at the root), from `git rev-parse --show-prefix`.
    prefix: String,
    /// Status by repository-relative path (directories without trailing `/`).
    entries: HashMap<String, FileStatus>,
    /// Every directory that contains a changed (non-ignored) entry.
    dirty_dirs: HashSet<String>,
}

/// Map a porcelain v2 `XY` field to a status.
fn status_from_xy(xy: &str) -> FileStatus {
    let has = |c: char| xy.contains(c);
    if has('D') {
        FileStatus::Deleted
    } else if has('A') {
        FileStatus::Added
    } else if has('R') || has('C') {
        FileStatus::Renamed
    } else {
        FileStatus::Modified
    }
}

/// Parse `git status --porcelain=v2 --branch -z` output. `prefix` is the
/// output of `git rev-parse --show-prefix` for the shown directory.
pub fn parse_status(output: &[u8], prefix: &str) -> GitStatus {
    let text = String::from_utf8_lossy(output);
    let mut status = GitStatus {
        prefix: prefix.trim_end_matches(['\n', '\r']).to_string(),
        ..GitStatus::default()
    };
    let mut records = text.split('\0');
    while let Some(record) = records.next() {
        let (kind, path) = match record.as_bytes().first() {
            Some(b'#') => {
                if let Some(head) = record.strip_prefix("# branch.head ") {
                    if head != "(detached)" {
                        status.branch = Some(head.to_string());
                    }
                }
                continue;
            }
            Some(b'1') => match record.splitn(9, ' ').collect::<Vec<_>>()[..] {
                [_, xy, .., path] => (status_from_xy(xy), path),
                _ => continue,
            },
            Some(b'2') => {
                // The original path follows as a separate NUL-terminated token.
                records.next();
                match record.splitn(10, ' ').collect::<Vec<_>>()[..] {
                    [_, xy, .., path] => (
                        if xy.contains('R') || xy.contains('C') {
                            FileStatus::Renamed
                        } else {
                            status_from_xy(xy)
                        },
                        path,
                    ),
                    _ => continue,
                }
            }
            Some(b'u') => match record.splitn(11, ' ').collect::<Vec<_>>()[..] {
                [.., path] => (FileStatus::Conflicted, path),
                _ => continue,
            },
            Some(b'?') => (FileStatus::Untracked, &record[2.min(record.len())..]),
            Some(b'!') => (FileStatus::Ignored, &record[2.min(record.len())..]),
            _ => continue,
        };
        let path = path.trim_end_matches('/');
        if path.is_empty() {
            continue;
        }
        status.insert(path, kind);
    }
    status
}

impl GitStatus {
    fn insert(&mut self, path: &str, kind: FileStatus) {
        let slot = self.entries.entry(path.to_string()).or_insert(kind);
        if kind.rank() > slot.rank() {
            *slot = kind;
        }
        if kind.is_change() {
            let mut dir = path;
            while let Some(idx) = dir.rfind('/') {
                dir = &dir[..idx];
                if !self.dirty_dirs.insert(dir.to_string()) {
                    break;
                }
            }
        }
    }

    /// Status of the entry `name` in the shown directory.
    pub fn lookup(&self, name: &str, is_dir: bool) -> Option<FileStatus> {
        let path = format!("{}{}", self.prefix, name);
        if let Some(&s) = self.entries.get(&path) {
            return Some(s);
        }
        // Inside an untracked or ignored directory, git reports only the
        // directory itself; its contents share that status.
        let mut dir = path.as_str();
        while let Some(idx) = dir.rfind('/') {
            dir = &dir[..idx];
            if let Some(&s) = self.entries.get(dir) {
                if matches!(s, FileStatus::Untracked | FileStatus::Ignored) {
                    return Some(s);
                }
            }
        }
        if is_dir && self.dirty_dirs.contains(&path) {
            return Some(FileStatus::DirtyDir);
        }
        None
    }
}

/// Run `git` in `dir` and return stdout, or `None` on any failure.
fn run_git(dir: &Path, args: &[&str]) -> Option<Vec<u8>> {
    let output = Command::new("git")
        // Don't take index.lock: other tools may be committing concurrently.
        .arg("--no-optional-locks")
        .arg("-C")
        .arg(dir)
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    output.status.success().then_some(output.stdout)
}

/// Read the git status of `dir`. Returns `None` outside a work tree or when
/// git is not available.
pub fn read_status(dir: &Path) -> Option<GitStatus> {
    let prefix = run_git(dir, &["rev-parse", "--show-prefix"])?;
    let output = run_git(
        dir,
        &[
            "status",
            "--porcelain=v2",
            "--branch",
            "-z",
            "--ignored",
            "--",
            ".",
        ],
    )?;
    Some(parse_status(&output, &String::from_utf8_lossy(&prefix)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Real output (NULs shown as `|`) from a repo where `a` is modified,
    /// `b` was renamed to `b2`, `sub/c` was removed from the index
    /// (`git rm --cached`) leaving `sub/` untracked, `un/` is untracked and
    /// `target/` is ignored.
    const ROOT: &str = "# branch.oid a12521d8dbb2b92a049f52b2653e774b599039a3|# branch.head main|1 .M N... 100644 100644 100644 78981922613b2afb6025042ff6bd878ac1994e85 78981922613b2afb6025042ff6bd878ac1994e85 a|2 R. N... 100644 100644 100644 61780798228d17af2d34fce4cfbdf35556832472 61780798228d17af2d34fce4cfbdf35556832472 R100 b2|b|1 D. N... 100644 000000 000000 f2ad6c76f0115a6ba5b00456a849810e7ec0af20 0000000000000000000000000000000000000000 sub/c|? sub/|? un/|! target/|";

    fn parse(sample: &str, prefix: &str) -> GitStatus {
        parse_status(sample.replace('|', "\0").as_bytes(), prefix)
    }

    #[test]
    fn parses_branch_and_records() {
        let st = parse(ROOT, "\n");
        assert_eq!(st.branch.as_deref(), Some("main"));
        assert_eq!(st.lookup("a", false), Some(FileStatus::Modified));
        assert_eq!(st.lookup("b2", false), Some(FileStatus::Renamed));
        // The original path of a rename is not a record of its own.
        assert_eq!(st.lookup("b", false), None);
        assert_eq!(st.lookup("un", true), Some(FileStatus::Untracked));
        assert_eq!(st.lookup("target", true), Some(FileStatus::Ignored));
        assert_eq!(st.lookup(".gitignore", false), None);
    }

    #[test]
    fn record_after_rename_is_parsed() {
        let sample = "2 R. N... 100644 100644 100644 aa bb R100 new name|old name|1 A. N... 000000 100644 100644 00 cc added file|";
        let st = parse(sample, "");
        assert_eq!(st.lookup("new name", false), Some(FileStatus::Renamed));
        assert_eq!(st.lookup("added file", false), Some(FileStatus::Added));
        assert_eq!(st.lookup("old name", false), None);
    }

    #[test]
    fn tracked_change_wins_over_untracked() {
        let st = parse("? x|1 D. N... 100644 000000 000000 aa 00 x|", "");
        assert_eq!(st.lookup("x", false), Some(FileStatus::Deleted));
    }

    #[test]
    fn detached_head_and_conflict() {
        let sample = "# branch.oid abc|# branch.head (detached)|u UU N... 100644 100644 100644 100644 a b c both.txt|";
        let st = parse(sample, "");
        assert_eq!(st.branch, None);
        assert_eq!(st.lookup("both.txt", false), Some(FileStatus::Conflicted));
    }

    #[test]
    fn directory_with_changes_is_dirty() {
        let sample = "1 .M N... 100644 100644 100644 aa aa src/deep/x.rs|! build/out|";
        let st = parse(sample, "");
        assert_eq!(st.lookup("src", true), Some(FileStatus::DirtyDir));
        // Only ignored entries below: not marked as changed.
        assert_eq!(st.lookup("build", true), None);
        let st = parse(sample, "src/\n");
        assert_eq!(st.lookup("deep", true), Some(FileStatus::DirtyDir));
        // The prefix is used, so a same-named entry at the root does not match.
        assert_eq!(st.lookup("src", true), None);
    }

    #[test]
    fn paths_resolve_against_shown_directory() {
        let st = parse(ROOT, "sub/\n");
        // `sub/c` is listed both as deleted and inside the untracked `sub/`.
        assert_eq!(st.lookup("c", false), Some(FileStatus::Deleted));
        // Anything else below the untracked `sub/` is untracked too.
        assert_eq!(st.lookup("new", false), Some(FileStatus::Untracked));
    }

    #[test]
    fn contents_of_untracked_and_ignored_dirs_inherit_status() {
        // Output from inside `un/deep` and `target/x`: only the top directory is listed.
        let st = parse("# branch.head main|? un/deep/|", "un/deep/\n");
        assert_eq!(st.lookup("f", false), Some(FileStatus::Untracked));
        let st = parse("# branch.head main|! target/|", "target/x/\n");
        assert_eq!(st.lookup("t", false), Some(FileStatus::Ignored));
    }

    #[test]
    fn read_status_outside_repo_is_none() {
        let dir = tempfile::tempdir().unwrap();
        // The temp dir may itself sit inside a repo on some machines; only
        // check the outside-repo case when git agrees it is outside.
        if run_git(dir.path(), &["rev-parse", "--show-prefix"]).is_none() {
            assert_eq!(read_status(dir.path()), None);
        }
    }
}
