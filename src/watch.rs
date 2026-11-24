//! Watching the shown directories and highlighting recently changed files.
//!
//! The panes' directories are watched (non-recursively) with `notify`; a
//! debounced batch of changed paths is sent to the event loop, which
//! refreshes the panes that show them. The decisions that do not need a
//! terminal or a real watcher (which panes to refresh, how long a file
//! stays highlighted, the `KURA_WATCH` setting) are plain functions here.

use std::{
    path::{Path, PathBuf},
    sync::mpsc::Sender,
    time::{Duration, SystemTime},
};

use notify_debouncer_mini::{
    new_debouncer_opt,
    notify::{self, PollWatcher, RecommendedWatcher, RecursiveMode, Watcher},
    Config, DebounceEventResult, DebouncedEventBatch, Debouncer,
};

/// Environment variable that selects how directories are watched.
pub const WATCH_ENV: &str = "KURA_WATCH";

/// Files changed less than this long ago are shown as "just changed".
pub const RECENT_WINDOW: Duration = Duration::from_secs(10);

/// Changes closer together than this are reported as one batch.
const DEBOUNCE: Duration = Duration::from_millis(200);

/// How often directories are re-scanned with `KURA_WATCH=poll`.
const POLL_INTERVAL: Duration = Duration::from_secs(1);

/// How the shown directories are watched.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WatchMode {
    /// OS file events (FSEvents / inotify / ReadDirectoryChangesW).
    Native,
    /// Periodic re-scan, for filesystems without events (e.g. WSL2 `/mnt/c`).
    Poll,
    /// No watching; the panes refresh only after kura's own operations.
    Off,
}

/// Read the watch mode from the value of `KURA_WATCH` (case-insensitive).
/// Unset or unknown values use OS events.
pub fn parse_watch_mode(value: Option<&str>) -> WatchMode {
    match value.map(|v| v.trim().to_ascii_lowercase()).as_deref() {
        Some("poll") => WatchMode::Poll,
        Some("off" | "0" | "false" | "no" | "none") => WatchMode::Off,
        _ => WatchMode::Native,
    }
}

/// A change reported by the watcher.
#[derive(Debug)]
pub enum FsEvent {
    /// These paths changed (were created, modified, removed or renamed).
    Changed(Vec<PathBuf>),
    /// Events were missed; every shown directory should be re-read.
    Rescan,
    /// The watcher reported an error.
    Error(String),
}

impl FsEvent {
    fn from_result(result: DebounceEventResult) -> Self {
        match result {
            Ok(DebouncedEventBatch::Events(events)) => {
                FsEvent::Changed(events.into_iter().map(|e| e.path).collect())
            }
            // Rescan, and any kind added later: re-read everything.
            Ok(_) => FsEvent::Rescan,
            Err(e) => FsEvent::Error(e.to_string()),
        }
    }
}

/// Directories to watch for the two panes (one if both show the same).
pub fn watch_targets(left: &Path, right: &Path) -> Vec<PathBuf> {
    if left == right {
        vec![left.to_path_buf()]
    } else {
        vec![left.to_path_buf(), right.to_path_buf()]
    }
}

/// Spellings of `dir` that event paths may use: as shown, and resolved
/// (macOS reports `/private/var/...` for `/var/...`).
pub fn dir_keys(dir: &Path) -> Vec<PathBuf> {
    let mut keys = vec![dir.to_path_buf()];
    if let Ok(real) = dir.canonicalize() {
        if real != dir {
            keys.push(real);
        }
    }
    keys
}

/// Whether a change to one of `changed` should re-read the directory known
/// by `keys` (see [`dir_keys`]): a direct child changed, or the directory
/// itself. Changes deeper down (e.g. `.git/index` written by `git status`)
/// are ignored, so refreshing never triggers itself.
pub fn affects_dir(changed: &[PathBuf], keys: &[PathBuf]) -> bool {
    changed.iter().any(|p| {
        keys.iter()
            .any(|k| p == k || p.parent().is_some_and(|parent| parent == k))
    })
}

/// How a file is highlighted for having changed recently.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Recency {
    /// Changed less than [`RECENT_WINDOW`] ago.
    Recent,
    /// Changed since kura started.
    SinceStart,
}

/// Classify a file by its modification time. Times in the future (clock
/// skew on network drives) count as just now.
pub fn classify(mtime: SystemTime, now: SystemTime, session_start: SystemTime) -> Option<Recency> {
    if mtime < session_start {
        return None;
    }
    let age = now.duration_since(mtime).unwrap_or(Duration::ZERO);
    Some(if age < RECENT_WINDOW {
        Recency::Recent
    } else {
        Recency::SinceStart
    })
}

/// Time until the first [`Recency::Recent`] highlight among `mtimes`
/// expires, i.e. when the screen must be redrawn even without input.
pub fn next_expiry(
    mtimes: impl IntoIterator<Item = SystemTime>,
    now: SystemTime,
    session_start: SystemTime,
) -> Option<Duration> {
    mtimes
        .into_iter()
        .filter(|&m| classify(m, now, session_start) == Some(Recency::Recent))
        .map(|m| {
            let age = now.duration_since(m).unwrap_or(Duration::ZERO);
            RECENT_WINDOW - age
        })
        .min()
}

enum Backend {
    Native(Debouncer<RecommendedWatcher>),
    Poll(Debouncer<PollWatcher>),
}

/// Watches the shown directories and sends [`FsEvent`]s through a channel.
pub struct DirWatcher {
    backend: Backend,
    watched: Vec<PathBuf>,
}

impl DirWatcher {
    /// Start watching (nothing yet; see [`DirWatcher::sync`]). Each event is
    /// wrapped with `wrap` and sent to `tx`. Returns `Ok(None)` for
    /// [`WatchMode::Off`].
    pub fn start<E: Send + 'static>(
        mode: WatchMode,
        tx: Sender<E>,
        wrap: fn(FsEvent) -> E,
    ) -> notify::Result<Option<Self>> {
        let handler = move |res: DebounceEventResult| {
            let _ = tx.send(wrap(FsEvent::from_result(res)));
        };
        let config = Config::default().with_timeout(DEBOUNCE);
        let backend = match mode {
            WatchMode::Off => return Ok(None),
            WatchMode::Native => Backend::Native(new_debouncer_opt(config, handler)?),
            WatchMode::Poll => {
                let config = config.with_notify_config(
                    notify::Config::default().with_poll_interval(POLL_INTERVAL),
                );
                Backend::Poll(new_debouncer_opt(config, handler)?)
            }
        };
        Ok(Some(Self {
            backend,
            watched: Vec::new(),
        }))
    }

    fn watcher(&mut self) -> &mut dyn Watcher {
        match &mut self.backend {
            Backend::Native(d) => d.watcher(),
            Backend::Poll(d) => d.watcher(),
        }
    }

    /// Watch exactly `targets`. Returns the directories that could not be
    /// watched (each is reported once, not retried until it is left).
    pub fn sync(&mut self, targets: &[PathBuf]) -> Vec<(PathBuf, notify::Error)> {
        let old = std::mem::take(&mut self.watched);
        for dir in old.iter().filter(|d| !targets.contains(d)) {
            let _ = self.watcher().unwatch(dir);
        }
        let mut failed = Vec::new();
        for dir in targets {
            if !old.contains(dir) {
                if let Err(e) = self.watcher().watch(dir, RecursiveMode::NonRecursive) {
                    failed.push((dir.clone(), e));
                }
            }
        }
        self.watched = targets.to_vec();
        failed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn watch_mode_from_env_value() {
        assert_eq!(parse_watch_mode(None), WatchMode::Native);
        assert_eq!(parse_watch_mode(Some("")), WatchMode::Native);
        assert_eq!(parse_watch_mode(Some("native")), WatchMode::Native);
        assert_eq!(parse_watch_mode(Some("bogus")), WatchMode::Native);
        assert_eq!(parse_watch_mode(Some("poll")), WatchMode::Poll);
        assert_eq!(parse_watch_mode(Some(" POLL ")), WatchMode::Poll);
        assert_eq!(parse_watch_mode(Some("off")), WatchMode::Off);
        assert_eq!(parse_watch_mode(Some("0")), WatchMode::Off);
    }

    #[test]
    fn same_directory_is_watched_once() {
        let a = Path::new("/a");
        let b = Path::new("/b");
        assert_eq!(watch_targets(a, a), vec![a.to_path_buf()]);
        assert_eq!(watch_targets(a, b), vec![a.to_path_buf(), b.to_path_buf()]);
    }

    #[test]
    fn only_direct_children_or_the_directory_itself_trigger_refresh() {
        let keys = vec![PathBuf::from("/repo"), PathBuf::from("/private/repo")];
        let hit = |p: &str| affects_dir(&[PathBuf::from(p)], &keys);
        assert!(hit("/repo/a.txt"));
        assert!(hit("/private/repo/new"));
        assert!(hit("/repo"));
        assert!(hit("/repo/.git")); // the entry itself is listed
        assert!(!hit("/repo/.git/index"));
        assert!(!hit("/repo/sub/deep.txt"));
        assert!(!hit("/other/a.txt"));
        assert!(!affects_dir(&[], &keys));
        assert!(affects_dir(
            &[PathBuf::from("/x/y"), PathBuf::from("/repo/z")],
            &keys
        ));
    }

    #[test]
    fn dir_keys_include_the_resolved_path() {
        let tmp = tempfile::tempdir().unwrap();
        let keys = dir_keys(tmp.path());
        assert_eq!(keys[0], tmp.path());
        let real = tmp.path().canonicalize().unwrap();
        assert!(keys.contains(&real));
        assert!(affects_dir(&[real.join("f")], &keys));
    }

    #[test]
    fn classify_by_modification_time() {
        let start = SystemTime::UNIX_EPOCH + Duration::from_secs(1000);
        let now = start + Duration::from_secs(100);
        let at = |secs_before_now: u64| now - Duration::from_secs(secs_before_now);
        assert_eq!(classify(at(0), now, start), Some(Recency::Recent));
        assert_eq!(classify(at(9), now, start), Some(Recency::Recent));
        assert_eq!(classify(at(10), now, start), Some(Recency::SinceStart));
        assert_eq!(classify(at(100), now, start), Some(Recency::SinceStart));
        assert_eq!(classify(at(101), now, start), None);
        // A time in the future counts as just changed.
        let future = now + Duration::from_secs(60);
        assert_eq!(classify(future, now, start), Some(Recency::Recent));
    }

    #[test]
    fn next_expiry_is_the_earliest_recent_highlight() {
        let start = SystemTime::UNIX_EPOCH + Duration::from_secs(1000);
        let now = start + Duration::from_secs(100);
        let ago = |s: u64| now - Duration::from_secs(s);
        assert_eq!(next_expiry([], now, start), None);
        // Old files and files from before the session never wake the loop.
        assert_eq!(next_expiry([ago(50), ago(200)], now, start), None);
        assert_eq!(
            next_expiry([ago(2), ago(7), ago(50)], now, start),
            Some(Duration::from_secs(3))
        );
        assert_eq!(
            next_expiry([now + Duration::from_secs(5)], now, start),
            Some(RECENT_WINDOW)
        );
    }

    /// Wait for a batch that touches `dir` from a real watcher.
    fn sees_change(mode: WatchMode) -> bool {
        let tmp = tempfile::tempdir().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let mut w = DirWatcher::start(mode, tx, |e| e).unwrap().unwrap();
        assert!(w.sync(&[tmp.path().to_path_buf()]).is_empty());
        std::thread::sleep(Duration::from_millis(300));
        std::fs::write(tmp.path().join("new.txt"), "x").unwrap();
        let keys = dir_keys(tmp.path());
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while let Some(left) = deadline.checked_duration_since(std::time::Instant::now()) {
            match rx.recv_timeout(left) {
                Ok(FsEvent::Changed(paths)) if affects_dir(&paths, &keys) => return true,
                Ok(FsEvent::Rescan) => return true,
                Ok(_) => {}
                Err(_) => break,
            }
        }
        false
    }

    #[test]
    fn native_watcher_reports_new_files() {
        assert!(sees_change(WatchMode::Native));
    }

    #[test]
    fn poll_watcher_reports_new_files() {
        assert!(sees_change(WatchMode::Poll));
    }

    #[test]
    fn off_starts_no_watcher() {
        let (tx, _rx) = std::sync::mpsc::channel::<FsEvent>();
        assert!(DirWatcher::start(WatchMode::Off, tx, |e| e)
            .unwrap()
            .is_none());
    }
}
