//! Command-line arguments.

use std::path::{Path, PathBuf};

use clap::Parser;

/// A two-pane TUI file manager.
#[derive(Parser, Debug, PartialEq, Eq)]
#[command(name = "kura", version)]
pub struct Cli {
    /// Directory for the left pane [default: current directory]
    pub left: Option<PathBuf>,
    /// Directory for the right pane [default: current directory]
    pub right: Option<PathBuf>,
    /// On exit, write the active pane's directory to FILE (for cd-on-exit)
    #[arg(long, value_name = "FILE")]
    pub last_dir_path: Option<PathBuf>,
}

/// Resolve a pane's starting directory: `arg` relative to `cwd`, or `cwd`
/// itself when not given. The result is canonical and must be a directory.
pub fn resolve_dir(arg: Option<&Path>, cwd: &Path) -> Result<PathBuf, String> {
    let Some(arg) = arg else {
        return Ok(cwd.to_path_buf());
    };
    let path = cwd.join(arg);
    let canonical = path
        .canonicalize()
        .map_err(|e| format!("{}: {e}", arg.display()))?;
    if !canonical.is_dir() {
        return Err(format!("{}: Not a directory", arg.display()));
    }
    Ok(canonical)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn parse(args: &[&str]) -> Result<Cli, clap::Error> {
        Cli::try_parse_from(std::iter::once("kura").chain(args.iter().copied()))
    }

    #[test]
    fn no_arguments() {
        assert_eq!(
            parse(&[]).unwrap(),
            Cli {
                left: None,
                right: None,
                last_dir_path: None
            }
        );
    }

    #[test]
    fn one_path_is_the_left_pane() {
        let cli = parse(&["src"]).unwrap();
        assert_eq!(cli.left, Some(PathBuf::from("src")));
        assert_eq!(cli.right, None);
    }

    #[test]
    fn two_paths_and_last_dir_path() {
        let cli = parse(&["a", "--last-dir-path", "/tmp/x", "b"]).unwrap();
        assert_eq!(cli.left, Some(PathBuf::from("a")));
        assert_eq!(cli.right, Some(PathBuf::from("b")));
        assert_eq!(cli.last_dir_path, Some(PathBuf::from("/tmp/x")));
    }

    #[test]
    fn help_version_and_bad_arguments() {
        use clap::error::ErrorKind;
        assert_eq!(
            parse(&["--help"]).unwrap_err().kind(),
            ErrorKind::DisplayHelp
        );
        assert_eq!(
            parse(&["--version"]).unwrap_err().kind(),
            ErrorKind::DisplayVersion
        );
        assert_eq!(
            parse(&["a", "b", "c"]).unwrap_err().kind(),
            ErrorKind::UnknownArgument
        );
        assert!(parse(&["--last-dir-path"]).is_err());
    }

    #[test]
    fn resolve_dir_defaults_to_cwd_and_joins_relative_paths() {
        let tmp = tempfile::tempdir().unwrap();
        let cwd = tmp.path().canonicalize().unwrap();
        fs::create_dir(cwd.join("sub")).unwrap();
        assert_eq!(resolve_dir(None, &cwd).unwrap(), cwd);
        assert_eq!(
            resolve_dir(Some(Path::new("sub")), &cwd).unwrap(),
            cwd.join("sub")
        );
        assert_eq!(resolve_dir(Some(Path::new("sub/..")), &cwd).unwrap(), cwd);
        assert_eq!(
            resolve_dir(Some(&cwd.join("sub")), Path::new("/")).unwrap(),
            cwd.join("sub")
        );
    }

    #[test]
    fn resolve_dir_rejects_missing_paths_and_files() {
        let tmp = tempfile::tempdir().unwrap();
        let cwd = tmp.path();
        fs::write(cwd.join("file"), "x").unwrap();
        assert!(resolve_dir(Some(Path::new("missing")), cwd).is_err());
        assert_eq!(
            resolve_dir(Some(Path::new("file")), cwd).unwrap_err(),
            "file: Not a directory"
        );
    }
}
