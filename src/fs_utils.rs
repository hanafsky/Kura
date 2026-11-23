use std::{
    cmp::Reverse,
    ffi::{OsStr, OsString},
    fs::{self, DirEntry},
    io,
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

/// Criteria for sorting the file list.
pub enum SortBy {
    Modified,
    Created,
    Size,
    Name,
}

/// Labels for sort options in the popup.
pub static SORT_OPTIONS: &[&str] = &[
    "Last modified date",
    "Creation date",
    "File size",
    "Alphabetical",
];

/// Apply the chosen sort order to the given pane.
pub fn apply_sort(pane: &mut crate::app::Pane, by: SortBy) {
    match by {
        SortBy::Modified => pane.items.sort_by(|a, b| {
            let ma = a
                .metadata()
                .and_then(|m| m.modified())
                .unwrap_or(UNIX_EPOCH);
            let mb = b
                .metadata()
                .and_then(|m| m.modified())
                .unwrap_or(UNIX_EPOCH);
            ma.cmp(&mb)
        }),
        SortBy::Created => pane.items.sort_by(|a, b| {
            let ca = a.metadata().and_then(|m| m.created()).unwrap_or(UNIX_EPOCH);
            let cb = b.metadata().and_then(|m| m.created()).unwrap_or(UNIX_EPOCH);
            ca.cmp(&cb)
        }),
        SortBy::Size => pane
            .items
            .sort_by_key(|e| Reverse(e.metadata().map(|m| m.len()).unwrap_or(0))),
        SortBy::Name => pane
            .items
            .sort_by_key(|e| e.file_name().to_string_lossy().to_lowercase()),
    }
    pane.selected = 0;
    pane.marked.clear();
}

/// Find the next entry matching `query` (case-insensitive) after `start`, wrapping around.
pub fn find_match(entries: &[DirEntry], query: &str, start: usize) -> Option<usize> {
    if query.is_empty() || entries.is_empty() {
        return None;
    }
    let q = query.to_lowercase();
    let total = entries.len();
    for i in 1..=total {
        let idx = (start + i) % total;
        let name = entries[idx].file_name().to_string_lossy().to_lowercase();
        if name.contains(&q) {
            return Some(idx);
        }
    }
    None
}

/// Build the `n`-th "copy" variant of a file name: `name (copy).ext` for
/// `n == 1`, `name (copy n).ext` for `n >= 2`. Directories keep their whole
/// name as the stem (`dir.d (copy)`).
pub fn copy_name(file_name: &OsStr, is_dir: bool, n: usize) -> OsString {
    let (stem, ext) = if is_dir {
        (file_name, None)
    } else {
        split_extension(file_name)
    };
    let mut out = OsString::from(stem);
    if n <= 1 {
        out.push(" (copy)");
    } else {
        out.push(format!(" (copy {n})"));
    }
    if let Some(ext) = ext {
        out.push(".");
        out.push(ext);
    }
    out
}

/// Compression suffixes that form a single extension together with `.tar`.
const TAR_COMPRESSIONS: &[&str] = &["gz", "bz2", "xz", "zst", "lz4", "lz", "lzma", "z", "br"];

/// Split a file name into stem and extension at the last `.`, except that
/// common double extensions such as `.tar.gz` are kept together.
fn split_extension(file_name: &OsStr) -> (&OsStr, Option<OsString>) {
    let as_path = Path::new(file_name);
    let stem = as_path.file_stem().unwrap_or(file_name);
    let Some(ext) = as_path.extension() else {
        return (stem, None);
    };
    let inner = Path::new(stem);
    let is_tar = inner
        .extension()
        .and_then(OsStr::to_str)
        .is_some_and(|e| e.eq_ignore_ascii_case("tar"));
    let is_compression = ext
        .to_str()
        .is_some_and(|e| TAR_COMPRESSIONS.iter().any(|c| e.eq_ignore_ascii_case(c)));
    if is_tar && is_compression {
        if let (Some(base), Some(tar)) = (inner.file_stem(), inner.extension()) {
            let mut double = OsString::from(tar);
            double.push(".");
            double.push(ext);
            return (base, Some(double));
        }
    }
    (stem, Some(ext.to_os_string()))
}

/// Decide where a pasted entry named `file_name` should land in `dst_dir`.
///
/// `same_location` tells whether the source already lives at
/// `dst_dir/file_name`. In that case the first free `(copy)` / `(copy n)`
/// name is chosen using `exists`; otherwise the plain name is returned.
/// Pure function: no filesystem access besides the injected `exists`.
pub fn paste_destination(
    file_name: &OsStr,
    dst_dir: &Path,
    is_dir: bool,
    same_location: bool,
    exists: impl Fn(&Path) -> bool,
) -> PathBuf {
    if !same_location {
        return dst_dir.join(file_name);
    }
    (1..)
        .map(|n| dst_dir.join(copy_name(file_name, is_dir, n)))
        .find(|p| !exists(p))
        .expect("an unused copy name always exists")
}

/// Resolve `path` to an absolute path without requiring the final component
/// to exist (its parent must exist).
fn canonicalize_lenient(path: &Path) -> io::Result<PathBuf> {
    match fs::canonicalize(path) {
        Ok(p) => Ok(p),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            let parent = path.parent().filter(|p| !p.as_os_str().is_empty());
            let parent = fs::canonicalize(parent.unwrap_or(Path::new(".")))?;
            match path.file_name() {
                Some(name) => Ok(parent.join(name)),
                None => Err(e),
            }
        }
        Err(e) => Err(e),
    }
}

/// Whether `src` (an existing entry) and `dst_dir/file_name` refer to the
/// same location on disk.
pub fn is_same_location(src: &Path, dst_dir: &Path) -> io::Result<bool> {
    let (Some(parent), Some(_)) = (src.parent(), src.file_name()) else {
        return Ok(false);
    };
    let parent = if parent.as_os_str().is_empty() {
        Path::new(".")
    } else {
        parent
    };
    Ok(fs::canonicalize(parent)? == fs::canonicalize(dst_dir)?)
}

/// Copy a single entry (file, directory or symlink) from `src` to `dst`.
/// Symlinks are never followed; they are recreated as links.
pub fn copy_entry(src: &Path, dst: &Path) -> io::Result<()> {
    let meta = fs::symlink_metadata(src)?;
    if meta.file_type().is_symlink() {
        copy_symlink(src, dst)
    } else if meta.is_dir() {
        copy_dir_recursively(src, dst)
    } else {
        if canonicalize_lenient(src)? == canonicalize_lenient(dst)? {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "cannot copy a file onto itself",
            ));
        }
        fs::copy(src, dst).map(|_| ())
    }
}

fn copy_symlink(src: &Path, dst: &Path) -> io::Result<()> {
    let target = fs::read_link(src)?;
    std::os::unix::fs::symlink(target, dst)
}

/// Recursively copy a directory without following symlinks.
///
/// Fails with `InvalidInput` if `dst` is `src` itself or lies inside it,
/// which would otherwise recurse forever.
pub fn copy_dir_recursively(src: &Path, dst: &Path) -> io::Result<()> {
    let src_abs = fs::canonicalize(src)?;
    let dst_abs = canonicalize_lenient(dst)?;
    if dst_abs.starts_with(&src_abs) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "cannot copy a folder into itself",
        ));
    }
    copy_dir_inner(src, dst)
}

fn copy_dir_inner(src: &Path, dst: &Path) -> io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let path = entry.path();
        let dst_path = dst.join(entry.file_name());
        let ft = entry.file_type()?;
        if ft.is_symlink() {
            copy_symlink(&path, &dst_path)?;
        } else if ft.is_dir() {
            copy_dir_inner(&path, &dst_path)?;
        } else {
            fs::copy(&path, &dst_path)?;
        }
    }
    Ok(())
}

/// Check that renaming `old` to `new` would not replace another entry.
/// Unix `rename` silently replaces an existing file, so a taken name is
/// refused. The same entry under another spelling (e.g. a case-only change
/// on a case-insensitive filesystem) is allowed.
pub fn ensure_rename_target_free(old: &Path, new: &Path) -> io::Result<()> {
    use std::os::unix::fs::MetadataExt;
    let Ok(new_meta) = fs::symlink_metadata(new) else {
        return Ok(());
    };
    let old_meta = fs::symlink_metadata(old)?;
    if old_meta.dev() == new_meta.dev() && old_meta.ino() == new_meta.ino() {
        return Ok(());
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "already exists",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::os::unix::fs::symlink;

    #[test]
    fn copy_name_variants() {
        let n = |s: &str, d, i| copy_name(OsStr::new(s), d, i);
        assert_eq!(n("a.txt", false, 1), "a (copy).txt");
        assert_eq!(n("a.txt", false, 2), "a (copy 2).txt");
        assert_eq!(n("Makefile", false, 1), "Makefile (copy)");
        assert_eq!(n(".bashrc", false, 3), ".bashrc (copy 3)");
        assert_eq!(n("conf.d", true, 1), "conf.d (copy)");
    }

    #[test]
    fn copy_name_keeps_tar_double_extensions() {
        let n = |s: &str, i| copy_name(OsStr::new(s), false, i);
        assert_eq!(n("archive.tar.gz", 1), "archive (copy).tar.gz");
        assert_eq!(n("archive.tar.gz", 2), "archive (copy 2).tar.gz");
        assert_eq!(n("a.tar.bz2", 1), "a (copy).tar.bz2");
        assert_eq!(n("a.tar.xz", 1), "a (copy).tar.xz");
        assert_eq!(n("a.tar.zst", 1), "a (copy).tar.zst");
        assert_eq!(n("a.tar.lz4", 1), "a (copy).tar.lz4");
        assert_eq!(n("A.TAR.GZ", 1), "A (copy).TAR.GZ");
        assert_eq!(n("v1.2.tar.gz", 1), "v1.2 (copy).tar.gz");
    }

    #[test]
    fn copy_name_splits_other_names_at_last_dot() {
        let n = |s: &str| copy_name(OsStr::new(s), false, 1);
        assert_eq!(n("a.tar"), "a (copy).tar");
        assert_eq!(n("a.txt.gz"), "a.txt (copy).gz");
        assert_eq!(n("a.tar.txt"), "a.tar (copy).txt");
        assert_eq!(n("a.gz"), "a (copy).gz");
        assert_eq!(n(".tar.gz"), ".tar (copy).gz");
    }

    #[test]
    fn copy_name_does_not_split_directories() {
        assert_eq!(
            copy_name(OsStr::new("src.tar.gz"), true, 1),
            "src.tar.gz (copy)"
        );
    }

    #[test]
    fn paste_destination_numbers_tar_gz() {
        let taken: HashSet<PathBuf> = ["/d/archive.tar.gz", "/d/archive (copy).tar.gz"]
            .iter()
            .map(PathBuf::from)
            .collect();
        let dst = paste_destination(
            OsStr::new("archive.tar.gz"),
            Path::new("/d"),
            false,
            true,
            |p| taken.contains(p),
        );
        assert_eq!(dst, Path::new("/d/archive (copy 2).tar.gz"));
    }

    #[test]
    fn paste_destination_keeps_name_for_other_dir() {
        let dst = paste_destination(OsStr::new("a.txt"), Path::new("/d"), false, false, |_| true);
        assert_eq!(dst, Path::new("/d/a.txt"));
    }

    #[test]
    fn paste_destination_picks_first_free_copy_name() {
        let taken: HashSet<PathBuf> = ["/d/a.txt", "/d/a (copy).txt", "/d/a (copy 2).txt"]
            .iter()
            .map(PathBuf::from)
            .collect();
        let dst = paste_destination(OsStr::new("a.txt"), Path::new("/d"), false, true, |p| {
            taken.contains(p)
        });
        assert_eq!(dst, Path::new("/d/a (copy 3).txt"));
    }

    #[test]
    fn copy_entry_onto_itself_is_rejected_and_keeps_content() {
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("a.txt");
        fs::write(&f, "hello").unwrap();
        let err = copy_entry(&f, &f).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        assert_eq!(fs::read_to_string(&f).unwrap(), "hello");
    }

    #[test]
    fn copy_dir_into_itself_is_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("dir");
        fs::create_dir(&src).unwrap();
        fs::write(src.join("f"), "x").unwrap();
        let err = copy_dir_recursively(&src, &src).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        assert_eq!(fs::read_to_string(src.join("f")).unwrap(), "x");
    }

    #[test]
    fn copy_dir_into_descendant_is_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("dir");
        fs::create_dir_all(src.join("sub")).unwrap();
        let dst = src.join("sub").join("dir");
        let err = copy_dir_recursively(&src, &dst).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        assert!(!dst.exists());
    }

    #[test]
    fn copy_dir_to_sibling_with_similar_prefix_is_allowed() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("dir");
        fs::create_dir(&src).unwrap();
        fs::write(src.join("f"), "x").unwrap();
        let dst = tmp.path().join("dir (copy)");
        copy_dir_recursively(&src, &dst).unwrap();
        assert_eq!(fs::read_to_string(dst.join("f")).unwrap(), "x");
    }

    #[test]
    fn symlinks_are_copied_as_links() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("dir");
        fs::create_dir(&src).unwrap();
        fs::write(src.join("target.txt"), "t").unwrap();
        symlink("target.txt", src.join("link")).unwrap();
        let dst = tmp.path().join("out");
        copy_dir_recursively(&src, &dst).unwrap();
        let copied = dst.join("link");
        assert!(fs::symlink_metadata(&copied)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(fs::read_link(&copied).unwrap(), Path::new("target.txt"));
    }

    #[test]
    fn symlink_loop_does_not_recurse() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("dir");
        fs::create_dir_all(src.join("sub")).unwrap();
        symlink("..", src.join("sub").join("up")).unwrap();
        symlink(&src, src.join("self")).unwrap();
        let dst = tmp.path().join("out");
        copy_dir_recursively(&src, &dst).unwrap();
        assert_eq!(fs::read_link(dst.join("sub/up")).unwrap(), Path::new(".."));
        assert_eq!(fs::read_link(dst.join("self")).unwrap(), src);
    }

    #[test]
    fn top_level_symlink_is_copied_as_link() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("real");
        fs::create_dir(&dir).unwrap();
        let link = tmp.path().join("link");
        symlink(&dir, &link).unwrap();
        let dst = tmp.path().join("link2");
        copy_entry(&link, &dst).unwrap();
        assert!(fs::symlink_metadata(&dst).unwrap().file_type().is_symlink());
        assert_eq!(fs::read_link(&dst).unwrap(), dir);
    }

    #[test]
    fn rename_onto_existing_name_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path().join("a.txt");
        let b = tmp.path().join("b.txt");
        fs::write(&a, "a").unwrap();
        fs::write(&b, "b").unwrap();
        let err = ensure_rename_target_free(&a, &b).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
        assert!(ensure_rename_target_free(&a, &tmp.path().join("c.txt")).is_ok());
        // Renaming to its own name is not a collision.
        assert!(ensure_rename_target_free(&a, &a).is_ok());
    }

    #[test]
    fn rename_onto_existing_dangling_symlink_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path().join("a");
        fs::write(&a, "a").unwrap();
        let link = tmp.path().join("link");
        symlink(tmp.path().join("missing"), &link).unwrap();
        assert!(ensure_rename_target_free(&a, &link).is_err());
    }

    #[test]
    fn copy_into_itself_errors_do_not_contain_paths() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path().join("dir");
        fs::create_dir_all(d.join("sub")).unwrap();
        let err = copy_dir_recursively(&d, &d.join("sub/dir")).unwrap_err();
        assert_eq!(err.to_string(), "cannot copy a folder into itself");
        let f = tmp.path().join("f");
        fs::write(&f, "x").unwrap();
        let err = copy_entry(&f, &f).unwrap_err();
        assert_eq!(err.to_string(), "cannot copy a file onto itself");
    }
}
