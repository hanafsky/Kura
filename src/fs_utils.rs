use std::{
    cmp::Reverse,
    fs::{self, DirEntry},
    io,
    path::Path,
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

/// Recursively copy a directory.
pub fn copy_dir_recursively(src: &Path, dst: &Path) -> io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let path = entry.path();
        let dst_path = dst.join(entry.file_name());
        if path.is_dir() {
            copy_dir_recursively(&path, &dst_path)?;
        } else {
            fs::copy(&path, &dst_path)?;
        }
    }
    Ok(())
}

/// Simple image-detection by file extension.
pub fn is_image(path: &Path) -> bool {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_lowercase)
    {
        Some(ext) => matches!(
            ext.as_str(),
            "png" | "jpg" | "jpeg" | "gif" | "bmp" | "tiff" | "tif" | "webp"
        ),
        None => false,
    }
}
