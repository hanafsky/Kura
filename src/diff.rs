//! Diff between the entries under the cursors of the two panes (`D`).
//!
//! Choosing what to diff ([`plan`]), reading git's exit status
//! ([`interpret`]) and classifying output lines for colouring
//! ([`classify_lines`]) are pure functions; [`run`] is the only part that
//! starts a process.

use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

/// What the cursor of one pane is on.
#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    /// Empty pane (nothing under the cursor).
    None,
    Dir,
    /// A file; `in_repo` is true when its directory is inside a git work tree.
    File {
        path: PathBuf,
        in_repo: bool,
    },
}

/// The diff to show for `D`.
#[derive(Debug, PartialEq)]
pub enum Plan {
    /// `git diff --no-index <left> <right>`.
    Files { left: PathBuf, right: PathBuf },
    /// `git diff HEAD -- <file>`: working tree against HEAD.
    Head(PathBuf),
    /// Nothing to diff; the reason for the status line.
    Reason(&'static str),
}

pub const SAME_FILE: &str = "Both panes point to the same file";
pub const DIRECTORY: &str = "Cannot diff a directory";
pub const NO_FILE: &str = "No file under the cursor";

/// Decide what to diff. `active` / `other` are the cursor targets of the
/// active and the other pane. Paths of files are compared as given, so the
/// caller should canonicalize them. When both panes point to the same file
/// (e.g. right after start), that file is diffed against HEAD if it is in a
/// repository.
pub fn plan(active: &Target, other: &Target, active_is_left: bool) -> Plan {
    match (active, other) {
        (Target::File { path: a, in_repo }, Target::File { path: b, .. }) => {
            if a == b {
                if *in_repo {
                    Plan::Head(a.clone())
                } else {
                    Plan::Reason(SAME_FILE)
                }
            } else if active_is_left {
                Plan::Files {
                    left: a.clone(),
                    right: b.clone(),
                }
            } else {
                Plan::Files {
                    left: b.clone(),
                    right: a.clone(),
                }
            }
        }
        (
            Target::File {
                path,
                in_repo: true,
            },
            _,
        ) => Plan::Head(path.clone()),
        (Target::File { .. }, Target::Dir) | (Target::Dir, _) => Plan::Reason(DIRECTORY),
        (Target::File { .. }, Target::None) | (Target::None, _) => Plan::Reason(NO_FILE),
    }
}

/// Arguments to `git` for a plan (after `-C <dir>` for [`Plan::Head`]).
pub fn git_args(plan: &Plan) -> Option<Vec<String>> {
    let mut args: Vec<String> = ["diff", "--no-ext-diff", "--no-textconv", "--color=never"]
        .map(String::from)
        .to_vec();
    match plan {
        Plan::Files { left, right } => {
            args.push("--no-index".into());
            args.push("--".into());
            args.push(left.to_string_lossy().into_owned());
            args.push(right.to_string_lossy().into_owned());
        }
        Plan::Head(path) => {
            args.push("HEAD".into());
            args.push("--".into());
            args.push(file_name(path));
        }
        Plan::Reason(_) => return None,
    }
    Some(args)
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Result of running git for a plan.
#[derive(Debug, PartialEq)]
pub enum Outcome {
    /// The diff text.
    Diff(String),
    /// No differences.
    Same,
    /// git failed; the message for the status line.
    Error(String),
}

/// Interpret git's exit code and output. With `--no-index` git exits with 1
/// when the files differ; otherwise a difference is shown with exit 0.
pub fn interpret(code: Option<i32>, stdout: &[u8], stderr: &[u8]) -> Outcome {
    match code {
        Some(0 | 1) if stdout.is_empty() => Outcome::Same,
        Some(0 | 1) => Outcome::Diff(String::from_utf8_lossy(stdout).into_owned()),
        _ => {
            let err = String::from_utf8_lossy(stderr);
            let line = err.lines().map(str::trim).find(|l| !l.is_empty());
            Outcome::Error(match line {
                Some(l) => format!("git diff failed: {l}"),
                None => "git diff failed".into(),
            })
        }
    }
}

/// Run git for a plan.
pub fn run(plan: &Plan) -> Outcome {
    let Some(args) = git_args(plan) else {
        return Outcome::Same;
    };
    let mut cmd = Command::new("git");
    cmd.args(["--no-optional-locks", "--literal-pathspecs"]);
    if let Plan::Head(path) = plan {
        if let Some(dir) = path.parent() {
            cmd.arg("-C").arg(dir);
        }
    }
    let output = cmd
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output();
    match output {
        Ok(o) => interpret(o.status.code(), &o.stdout, &o.stderr),
        Err(e) => Outcome::Error(format!("git diff failed: {e}")),
    }
}

/// Title of the diff viewer.
pub fn title(plan: &Plan) -> String {
    match plan {
        Plan::Files { left, right } => {
            format!("diff: {} <-> {}", left.display(), right.display())
        }
        Plan::Head(path) => format!("git diff HEAD: {}", path.display()),
        Plan::Reason(r) => r.to_string(),
    }
}

/// Kind of a diff output line, for colouring.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineKind {
    /// `diff --git`, `index`, `---`, `+++` and other file header lines.
    Header,
    /// `@@ -a,b +c,d @@`.
    Hunk,
    Added,
    Removed,
    Context,
}

/// Classify each line of a unified diff. Lines between `diff ` and the first
/// `@@` of a file are headers, so `--- a/x` is never taken for a removed
/// line (and a removed line `-- x` is never taken for a header).
pub fn classify_lines(text: &str) -> Vec<LineKind> {
    let mut in_header = false;
    text.lines()
        .map(|line| {
            if line.starts_with("diff ") {
                in_header = true;
                LineKind::Header
            } else if line.starts_with("@@") {
                in_header = false;
                LineKind::Hunk
            } else if in_header {
                LineKind::Header
            } else if line.starts_with('+') {
                LineKind::Added
            } else if line.starts_with('-') {
                LineKind::Removed
            } else {
                LineKind::Context
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(p: &str, in_repo: bool) -> Target {
        Target::File {
            path: PathBuf::from(p),
            in_repo,
        }
    }

    #[test]
    fn two_files_are_diffed_left_to_right() {
        let files = Plan::Files {
            left: "/l/a".into(),
            right: "/r/a".into(),
        };
        assert_eq!(plan(&file("/l/a", true), &file("/r/a", false), true), files);
        assert_eq!(
            plan(&file("/r/a", false), &file("/l/a", true), false),
            files
        );
    }

    #[test]
    fn same_file_is_diffed_against_head_in_a_repo_and_reported_outside() {
        let a = file("/x/a", true);
        assert_eq!(plan(&a, &a, true), Plan::Head("/x/a".into()));
        let b = file("/x/b", false);
        assert_eq!(plan(&b, &b, false), Plan::Reason(SAME_FILE));
    }

    #[test]
    fn repo_file_against_directory_or_empty_pane_diffs_against_head() {
        let a = file("/x/a", true);
        assert_eq!(plan(&a, &Target::Dir, true), Plan::Head("/x/a".into()));
        assert_eq!(plan(&a, &Target::None, false), Plan::Head("/x/a".into()));
    }

    #[test]
    fn directories_and_empty_panes_are_reported() {
        let out = file("/x/a", false);
        assert_eq!(plan(&out, &Target::Dir, true), Plan::Reason(DIRECTORY));
        assert_eq!(plan(&Target::Dir, &out, true), Plan::Reason(DIRECTORY));
        assert_eq!(
            plan(&Target::Dir, &file("/x/b", true), true),
            Plan::Reason(DIRECTORY)
        );
        assert_eq!(plan(&out, &Target::None, true), Plan::Reason(NO_FILE));
        assert_eq!(plan(&Target::None, &out, true), Plan::Reason(NO_FILE));
    }

    #[test]
    fn git_arguments() {
        let files = Plan::Files {
            left: "/l/a".into(),
            right: "/r/-b".into(),
        };
        assert_eq!(
            git_args(&files).unwrap(),
            [
                "diff",
                "--no-ext-diff",
                "--no-textconv",
                "--color=never",
                "--no-index",
                "--",
                "/l/a",
                "/r/-b"
            ]
        );
        let head = git_args(&Plan::Head("/repo/src/x.rs".into())).unwrap();
        assert_eq!(head[4..], ["HEAD", "--", "x.rs"]);
        assert_eq!(git_args(&Plan::Reason(SAME_FILE)), None);
    }

    #[test]
    fn exit_status_is_interpreted() {
        assert_eq!(interpret(Some(1), b"d\n", b""), Outcome::Diff("d\n".into()));
        assert_eq!(interpret(Some(0), b"d\n", b""), Outcome::Diff("d\n".into()));
        assert_eq!(interpret(Some(0), b"", b""), Outcome::Same);
        assert_eq!(
            interpret(Some(128), b"", b"\nfatal: bad revision 'HEAD'\nmore\n"),
            Outcome::Error("git diff failed: fatal: bad revision 'HEAD'".into())
        );
        assert_eq!(
            interpret(None, b"", b""),
            Outcome::Error("git diff failed".into())
        );
    }

    #[test]
    fn lines_are_classified() {
        let text = "diff --git a/x b/x\nindex 1..2 100644\n--- a/x\n+++ b/x\n@@ -1,3 +1,3 @@ fn\n ctx\n-old\n--- not a header\n+new\n+++ not a header\n\\ No newline at end of file\ndiff --git a/y b/y\nnew file mode 100644\n--- /dev/null\n+++ b/y\n@@ -0,0 +1 @@\n+y\nBinary files a/z and b/z differ\n";
        use LineKind::*;
        assert_eq!(
            classify_lines(text),
            [
                Header, Header, Header, Header, Hunk, Context, Removed, Removed, Added, Added,
                Context, Header, Header, Header, Header, Hunk, Added, Context
            ]
        );
    }
}
