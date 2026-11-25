//! Image / video preview: background loading, prefetch and cache.
//!
//! Decoding (image crate, ffmpeg) and encoding for the terminal graphics
//! protocol (ratatui-image's `resize_encode`) both run on worker threads, so
//! keys keep working while a large image or a video thumbnail loads. The UI
//! shows "Loading…" meanwhile.
//!
//! - Only the latest request matters: [`Previewer::show`] replaces every load
//!   that has not started yet, so pressing `j` repeatedly loads the last
//!   image (plus its neighbours), not every image passed on the way.
//! - The previous / next media entries are prefetched ([`PREFETCH`] in each
//!   direction) and encoded for the last known pane size, so stepping to them
//!   is usually instant.
//! - Loaded previews are cached, bounded by [`CACHE_ENTRIES`] and
//!   [`CACHE_BYTES`]; the cache is dropped when the preview is closed.
//! - The encoding depends on the pane size. When it changes, the decoded image
//!   kept in the protocol is re-encoded on a worker (no decoding again).
//!
//! [`Previewer`] does not depend on the app mode or the file list, so any
//! view can own one (the `Enter` gallery uses `App::preview`):
//!
//! 1. [`Previewer::show`] with the path to show and the paths to prefetch
//!    ([`prefetch_around`] picks them from a list).
//! 2. In the event loop, [`Previewer::poll`] takes finished work (redraw when
//!    it returns true); poll more often while [`Previewer::busy`].
//! 3. [`Previewer::draw`] draws it with a titled border, or
//!    [`Previewer::draw_body`] draws only the image and info lines into an
//!    area. The size drawn into is what the workers encode for.
//! 4. [`Previewer::close`] when it is no longer shown, or
//!    [`Previewer::hide`] to stop showing but keep the cache (the `P` pane
//!    preview, while the cursor is on a non-media entry).

use std::collections::{HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc, Condvar, Mutex};
use std::time::SystemTime;

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect, Size},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};
use ratatui_image::{
    picker::Picker, protocol::StatefulProtocol, Resize, ResizeEncodeRender, StatefulImage,
};

use crate::media;

/// Media entries prefetched on each side of the shown one.
pub const PREFETCH: usize = 1;
/// Most previews kept in memory (the shown one plus its neighbours, and a
/// little room for stepping back and forth).
pub const CACHE_ENTRIES: usize = 5;
/// Rough memory bound for the cache. A decoded image is at most
/// 2048 x 2048 RGBA = 16 MiB; the encoded data is counted as the same size
/// again (see [`Entry::bytes`]).
pub const CACHE_BYTES: usize = 128 << 20;
/// Worker threads: one can run a slow ffmpeg prefetch while the other loads
/// the entry the user is waiting for.
const WORKERS: usize = 2;

const RESIZE: Resize = Resize::Fit(None);

/// Index of the `n`-th media entry after (`forward`) or before `from`, and
/// how many steps were actually taken. Fewer than `n` steps means the end of
/// the list was reached; with 0 steps the index is `from`.
///
/// `is_media[i]` tells whether entry `i` is an image or a video (folders and
/// other files are skipped).
pub fn step_media(is_media: &[bool], from: usize, n: usize, forward: bool) -> (usize, usize) {
    let len = is_media.len();
    let hits: Vec<usize> = if forward {
        (from.saturating_add(1)..len)
            .filter(|&i| is_media[i])
            .take(n)
            .collect()
    } else {
        (0..from.min(len))
            .rev()
            .filter(|&i| is_media[i])
            .take(n)
            .collect()
    };
    (hits.last().copied().unwrap_or(from), hits.len())
}

/// Media entries to prefetch around `idx`: the ones in the direction of
/// travel first.
pub fn prefetch_around(is_media: &[bool], idx: usize, forward: bool) -> Vec<usize> {
    let side = |dir: bool| {
        let mut out = Vec::new();
        let mut at = idx;
        for _ in 0..PREFETCH {
            let (next, steps) = step_media(is_media, at, 1, dir);
            if steps == 0 {
                break;
            }
            out.push(next);
            at = next;
        }
        out
    };
    let mut out = side(forward);
    out.extend(side(!forward));
    out
}

/// Area of the image inside the preview block: everything above the info lines.
pub fn image_area(inner: Rect, info_lines: usize) -> Rect {
    let info_height = (info_lines as u16).min(inner.height);
    Rect {
        height: inner.height - info_height,
        ..inner
    }
}

/// Encode `protocol` for the image area `area`, exactly as the
/// `StatefulImage` widget would when drawing it into the fitted rect (so the
/// draw does not encode again on the UI thread). An empty area is skipped
/// (nothing is drawn there).
fn encode_for(protocol: &mut StatefulProtocol, area: Size) -> Result<(), String> {
    if area.width == 0 || area.height == 0 {
        return Ok(());
    }
    let fitted = protocol.size_for(RESIZE, area);
    match protocol.needs_resize(&RESIZE, fitted) {
        Some(size) => encode(protocol, size),
        None => Ok(()),
    }
}

fn encode(protocol: &mut StatefulProtocol, size: Size) -> Result<(), String> {
    protocol.resize_encode(&RESIZE, size);
    match protocol.last_encoding_result() {
        Some(Err(e)) => Err(format!("Cannot encode image: {e}")),
        _ => Ok(()),
    }
}

/// Modification time and size, to notice files changed since they were cached.
type Stamp = Option<(SystemTime, u64)>;

fn stamp(path: &Path) -> Stamp {
    let m = std::fs::metadata(path).ok()?;
    Some((m.modified().ok()?, m.len()))
}

enum Image {
    /// No image could be produced (the info lines say why).
    None,
    Ready(Box<StatefulProtocol>),
    /// Being re-encoded on a worker; the result carries this token.
    Encoding(u64),
}

struct Entry {
    path: PathBuf,
    stamp: Stamp,
    title: String,
    info: Vec<String>,
    image: Image,
    /// Estimated memory: decoded pixels plus about as much for the encoding.
    bytes: usize,
}

enum Job {
    Load(PathBuf),
    Encode {
        path: PathBuf,
        token: u64,
        protocol: Box<StatefulProtocol>,
        size: Size,
    },
}

enum Done {
    Loaded(Box<Entry>),
    Encoded {
        path: PathBuf,
        token: u64,
        /// The encoded protocol, or why encoding failed.
        result: Result<Box<StatefulProtocol>, String>,
    },
}

#[derive(Default)]
struct Jobs {
    encodes: VecDeque<Job>,
    loads: VecDeque<Job>,
    /// Paths being loaded right now (until the result is received).
    running: HashSet<PathBuf>,
    /// Jobs taken by a worker whose result has not been received yet.
    busy: usize,
    /// Inner area of the preview block at the last draw; loads encode for
    /// it right away (read when decoding is done, so the first draw after
    /// opening the preview is already taken into account).
    area: Option<Size>,
    closed: bool,
}

#[derive(Default)]
struct Queue {
    jobs: Mutex<Jobs>,
    ready: Condvar,
}

impl Queue {
    fn lock(&self) -> std::sync::MutexGuard<'_, Jobs> {
        self.jobs.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Replace the pending loads (in priority order), skipping paths that
    /// are being loaded already.
    fn set_loads(&self, paths: &[PathBuf]) {
        let mut jobs = self.lock();
        jobs.loads = paths
            .iter()
            .filter(|p| !jobs.running.contains(*p))
            .map(|p| Job::Load(p.clone()))
            .collect();
        self.ready.notify_all();
    }

    fn push_encode(&self, job: Job) {
        self.lock().encodes.push_back(job);
        self.ready.notify_one();
    }

    fn clear(&self) {
        let mut jobs = self.lock();
        jobs.loads.clear();
        jobs.encodes.clear();
    }

    fn idle(&self) -> bool {
        let jobs = self.lock();
        jobs.busy == 0 && jobs.loads.is_empty() && jobs.encodes.is_empty()
    }

    /// Next job for a worker (encodes first); None once closed.
    fn take(&self) -> Option<Job> {
        let mut jobs = self.lock();
        loop {
            if jobs.closed {
                return None;
            }
            if let Some(job) = jobs.encodes.pop_front().or_else(|| jobs.loads.pop_front()) {
                if let Job::Load(path) = &job {
                    jobs.running.insert(path.clone());
                }
                jobs.busy += 1;
                return Some(job);
            }
            jobs = self.ready.wait(jobs).unwrap_or_else(|e| e.into_inner());
        }
    }

    /// A job is over: its result was received (or there was none).
    fn finish(&self, loaded: Option<&Path>) {
        let mut jobs = self.lock();
        if let Some(path) = loaded {
            jobs.running.remove(path);
        }
        jobs.busy = jobs.busy.saturating_sub(1);
    }
}

fn worker(queue: Arc<Queue>, picker: Picker, tx: mpsc::Sender<Done>) {
    while let Some(job) = queue.take() {
        let (path, done) = run_job(job, &picker, &queue);
        match done {
            // Finished by the receiver, so the job counts as busy until
            // its result can be picked up.
            Some(done) => {
                if tx.send(done).is_err() {
                    return;
                }
            }
            None => queue.finish(Some(&path)),
        }
    }
}

fn run_job(job: Job, picker: &Picker, queue: &Queue) -> (PathBuf, Option<Done>) {
    match job {
        Job::Load(path) => {
            let stamp = stamp(&path);
            let done = media::load(&path).map(|loaded| {
                let bytes = loaded.image.as_ref().map_or(0, |i| i.as_bytes().len() * 2);
                let mut info = loaded.info;
                let image = match loaded.image {
                    Some(img) => {
                        let mut protocol = picker.new_resize_protocol(img);
                        let area = queue.lock().area;
                        let encoded = area.map_or(Ok(()), |area| {
                            let inner = Rect::new(0, 0, area.width, area.height);
                            encode_for(&mut protocol, image_area(inner, info.len()).as_size())
                        });
                        match encoded {
                            Ok(()) => Image::Ready(Box::new(protocol)),
                            Err(e) => {
                                info.push(e);
                                Image::None
                            }
                        }
                    }
                    None => Image::None,
                };
                Done::Loaded(Box::new(Entry {
                    path: path.clone(),
                    stamp,
                    title: loaded.title,
                    info,
                    image,
                    bytes,
                }))
            });
            (path, done)
        }
        Job::Encode {
            path,
            token,
            mut protocol,
            size,
        } => {
            let result = encode(&mut protocol, size).map(|()| protocol);
            let done = Done::Encoded {
                path: path.clone(),
                token,
                result,
            };
            (path, Some(done))
        }
    }
}

/// The preview shown in the pane opposite to the active one, with its
/// background loader and cache.
pub struct Previewer {
    queue: Arc<Queue>,
    results: mpsc::Receiver<Done>,
    /// Most recently used last.
    entries: Vec<Entry>,
    /// The entry to show, then the ones to prefetch.
    wanted: Vec<PathBuf>,
    /// Nothing is shown, but `wanted` keeps loading into the cache
    /// ([`Previewer::hide`]).
    hidden: bool,
    next_token: u64,
}

impl Previewer {
    pub fn new(picker: Picker) -> Self {
        let queue = Arc::new(Queue::default());
        let (tx, results) = mpsc::channel();
        for _ in 0..WORKERS {
            let (queue, picker, tx) = (queue.clone(), picker.clone(), tx.clone());
            std::thread::spawn(move || worker(queue, picker, tx));
        }
        Self {
            queue,
            results,
            entries: Vec::new(),
            wanted: Vec::new(),
            hidden: false,
            next_token: 0,
        }
    }

    /// Path being shown, if any.
    pub fn current(&self) -> Option<&Path> {
        if self.hidden {
            return None;
        }
        self.wanted.first().map(PathBuf::as_path)
    }

    /// Paths being shown and prefetched, in priority order.
    #[cfg(test)]
    pub fn wanted(&self) -> &[PathBuf] {
        &self.wanted
    }

    /// Show `path` (loading it in the background if needed) and prefetch
    /// `prefetch`. Loads requested earlier that have not started are dropped.
    pub fn show(&mut self, path: PathBuf, prefetch: Vec<PathBuf>) {
        self.hidden = false;
        self.wanted = std::iter::once(path).chain(prefetch).collect();
        // Forget cached entries whose file changed since.
        self.entries
            .retain(|e| !self.wanted.contains(&e.path) || stamp(&e.path) == e.stamp);
        // Most recently used: the shown entry, then its neighbours.
        for p in self.wanted.iter().rev() {
            if let Some(i) = self.entries.iter().position(|e| &e.path == p) {
                let e = self.entries.remove(i);
                self.entries.push(e);
            }
        }
        let missing: Vec<PathBuf> = self
            .wanted
            .iter()
            .filter(|p| !self.entries.iter().any(|e| &e.path == *p))
            .cloned()
            .collect();
        self.queue.set_loads(&missing);
    }

    /// Stop showing, but keep the cache and let the requested loads
    /// (prefetches included) finish into it, so coming back to them is
    /// instant. The next [`Previewer::show`] replaces what is still pending.
    pub fn hide(&mut self) {
        self.hidden = true;
    }

    /// Stop showing and drop the cache and pending work.
    pub fn close(&mut self) {
        self.hidden = false;
        self.wanted.clear();
        self.entries.clear();
        self.queue.clear();
    }

    /// True while the shown preview waits for a worker (the event loop then
    /// checks for results more often).
    pub fn busy(&self) -> bool {
        self.current().is_some() && !self.queue.idle()
    }

    /// Take finished work from the workers; true if anything arrived.
    pub fn poll(&mut self) -> bool {
        let mut any = false;
        while let Ok(done) = self.results.try_recv() {
            any = true;
            match &done {
                Done::Loaded(entry) => self.queue.finish(Some(&entry.path)),
                Done::Encoded { .. } => self.queue.finish(None),
            }
            match done {
                Done::Loaded(entry) => {
                    // Results nobody waits for any more are dropped.
                    if self.wanted.contains(&entry.path) {
                        self.entries.retain(|e| e.path != entry.path);
                        self.entries.push(*entry);
                        self.evict();
                    }
                }
                Done::Encoded {
                    path,
                    token,
                    result,
                } => {
                    if let Some(e) = self.entries.iter_mut().find(|e| {
                        e.path == path && matches!(e.image, Image::Encoding(t) if t == token)
                    }) {
                        e.image = match result {
                            Ok(protocol) => Image::Ready(protocol),
                            Err(msg) => {
                                e.info.push(msg);
                                Image::None
                            }
                        };
                    }
                }
            }
        }
        any
    }

    /// Drop least recently used entries beyond the limits, never the shown one.
    fn evict(&mut self) {
        loop {
            let total: usize = self.entries.iter().map(|e| e.bytes).sum();
            if self.entries.len() <= CACHE_ENTRIES && total <= CACHE_BYTES {
                return;
            }
            let current = self.wanted.first();
            match self.entries.iter().position(|e| Some(&e.path) != current) {
                Some(i) => {
                    self.entries.remove(i);
                }
                None => return,
            }
        }
    }

    /// Title of the shown preview (the file name until it is loaded).
    pub fn title(&self) -> Option<String> {
        let path = self.current()?;
        Some(match self.entries.iter().find(|e| e.path == path) {
            Some(e) => e.title.clone(),
            None => path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
        })
    }

    /// Draw the shown preview into `area`.
    pub fn draw(&mut self, f: &mut Frame, area: Rect) {
        let Some(title) = self.title() else {
            return;
        };
        let block = Block::default().borders(Borders::ALL).title(Span::styled(
            format!(" {title} "),
            Style::default()
                .fg(Color::Magenta)
                .add_modifier(Modifier::BOLD),
        ));
        let inner = block.inner(area);
        f.render_widget(block, area);
        self.draw_body(f, inner);
    }

    /// Draw the shown preview's image ("Loading…" until it is ready) and
    /// info lines into `inner`, without a border or title.
    pub fn draw_body(&mut self, f: &mut Frame, inner: Rect) {
        let Some(path) = self.current().map(Path::to_path_buf) else {
            return;
        };
        self.queue.lock().area = Some(inner.as_size());
        let entry = self.entries.iter_mut().find(|e| e.path == path);
        let Some(entry) = entry else {
            draw_loading(f, inner);
            return;
        };
        let img_area = image_area(inner, entry.info.len());
        // No room for the image (e.g. a low pane filled by video info).
        let room = img_area.width > 0 && img_area.height > 0;
        if let (Image::Ready(protocol), true) = (&entry.image, room) {
            let fitted = protocol.size_for(RESIZE, img_area.as_size());
            // The pane size changed: re-encode on a worker instead of in
            // the widget (which would block the UI thread).
            if let Some(size) = protocol.needs_resize(&RESIZE, fitted) {
                self.next_token += 1;
                let token = self.next_token;
                if let Image::Ready(protocol) =
                    std::mem::replace(&mut entry.image, Image::Encoding(token))
                {
                    self.queue.push_encode(Job::Encode {
                        path: path.clone(),
                        token,
                        protocol,
                        size,
                    });
                }
            }
        }
        match &mut entry.image {
            Image::Ready(protocol) if room => {
                // Center the fitted image inside the available area.
                let fitted = protocol.size_for(RESIZE, img_area.as_size());
                let x = img_area.x + img_area.width.saturating_sub(fitted.width) / 2;
                let y = img_area.y + img_area.height.saturating_sub(fitted.height) / 2;
                let centered = Rect::new(x, y, fitted.width, fitted.height);
                f.render_stateful_widget(
                    StatefulImage::default().resize(RESIZE),
                    centered,
                    protocol.as_mut(),
                );
            }
            Image::Encoding(_) => draw_loading(f, img_area),
            Image::Ready(_) | Image::None => {}
        }

        let lines: Vec<Line> = entry
            .info
            .iter()
            .map(|l| Line::from(Span::styled(l.as_str(), Style::default().fg(Color::Gray))))
            .collect();
        let parts = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(img_area.height), Constraint::Min(0)])
            .split(inner);
        f.render_widget(Paragraph::new(lines), parts[1]);
    }
}

impl Drop for Previewer {
    fn drop(&mut self) {
        self.queue.lock().closed = true;
        self.queue.ready.notify_all();
    }
}

fn draw_loading(f: &mut Frame, area: Rect) {
    if area.height == 0 {
        return;
    }
    let y = area.y + area.height / 2;
    let line = Rect::new(area.x, y, area.width, 1);
    f.render_widget(
        Paragraph::new("Loading…")
            .style(Style::default().fg(Color::Gray))
            .alignment(ratatui::layout::Alignment::Center),
        line,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    const T: bool = true;
    const F: bool = false;

    #[test]
    fn steps_over_non_media() {
        // 0:img 1:dir 2:txt 3:img 4:video
        let m = [T, F, F, T, T];
        assert_eq!(step_media(&m, 0, 1, true), (3, 1));
        assert_eq!(step_media(&m, 3, 1, true), (4, 1));
        assert_eq!(step_media(&m, 4, 1, false), (3, 1));
        assert_eq!(step_media(&m, 3, 1, false), (0, 1));
    }

    #[test]
    fn count_stops_at_the_end() {
        let m = [T, F, T, T, F];
        assert_eq!(step_media(&m, 0, 2, true), (3, 2));
        assert_eq!(step_media(&m, 0, 5, true), (3, 2));
        assert_eq!(step_media(&m, 3, 5, false), (0, 2));
    }

    #[test]
    fn no_step_at_either_end() {
        let m = [F, T, F, T, F];
        assert_eq!(step_media(&m, 3, 1, true), (3, 0));
        assert_eq!(step_media(&m, 1, 1, false), (1, 0));
        assert_eq!(step_media(&[T], 0, 1, true), (0, 0));
        assert_eq!(step_media(&[T], 0, 1, false), (0, 0));
        assert_eq!(step_media(&[], 0, 1, true), (0, 0));
        assert_eq!(step_media(&[], 0, 1, false), (0, 0));
    }

    #[test]
    fn cursor_past_the_list_is_safe() {
        let m = [T, T];
        assert_eq!(step_media(&m, 5, 1, true), (5, 0));
        assert_eq!(step_media(&m, 5, 1, false), (1, 1));
    }

    #[test]
    fn prefetch_direction_of_travel_first() {
        let m = [T, F, T, T, F, T];
        assert_eq!(prefetch_around(&m, 2, true), vec![3, 0]);
        assert_eq!(prefetch_around(&m, 2, false), vec![0, 3]);
        assert_eq!(prefetch_around(&m, 5, true), vec![3]);
        assert_eq!(prefetch_around(&[T], 0, true), Vec::<usize>::new());
    }

    #[test]
    fn image_area_leaves_room_for_info() {
        let inner = Rect::new(2, 3, 40, 20);
        assert_eq!(image_area(inner, 4), Rect::new(2, 3, 40, 16));
        assert_eq!(image_area(inner, 30), Rect::new(2, 3, 40, 0));
    }

    /// The worker must encode for exactly the size the widget asks for, or
    /// the draw encodes again on the UI thread.
    #[test]
    fn worker_encoding_matches_the_draw() {
        let picker = Picker::halfblocks();
        let img = image::DynamicImage::new_rgb8(300, 200);
        let mut protocol = picker.new_resize_protocol(img);
        let area = Size::new(37, 21);
        assert!(protocol
            .needs_resize(&RESIZE, protocol.size_for(RESIZE, area))
            .is_some());
        encode_for(&mut protocol, area).unwrap();
        let fitted = protocol.size_for(RESIZE, area);
        assert_eq!(protocol.needs_resize(&RESIZE, fitted), None);
    }

    #[test]
    fn empty_area_is_not_encoded() {
        let picker = Picker::halfblocks();
        let mut protocol = picker.new_resize_protocol(image::DynamicImage::new_rgb8(30, 20));
        encode_for(&mut protocol, Size::new(40, 0)).unwrap();
        assert!(protocol.last_encoding_result().is_none());
    }

    fn write_png(dir: &Path, name: &str) -> PathBuf {
        let path = dir.join(name);
        image::DynamicImage::new_rgb8(16, 8).save(&path).unwrap();
        path
    }

    fn wait_loaded(p: &mut Previewer, path: &Path) {
        let start = Instant::now();
        while !p.entries.iter().any(|e| e.path == path) {
            assert!(start.elapsed() < Duration::from_secs(10), "load timed out");
            p.poll();
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn loads_in_the_background_and_prefetches() {
        let dir = tempfile::tempdir().unwrap();
        let a = write_png(dir.path(), "a.png");
        let b = write_png(dir.path(), "b.png");
        let mut p = Previewer::new(Picker::halfblocks());
        p.show(a.clone(), vec![b.clone()]);
        assert_eq!(p.current(), Some(a.as_path()));
        wait_loaded(&mut p, &a);
        wait_loaded(&mut p, &b);
        let e = p.entries.iter().find(|e| e.path == a).unwrap();
        assert!(matches!(e.image, Image::Ready(_)));
        assert_eq!(e.info.len(), 1);

        // Stepping to the prefetched one needs no new load.
        p.show(b.clone(), vec![a.clone()]);
        assert_eq!(p.queue.lock().loads.len(), 0);

        p.close();
        assert_eq!(p.current(), None);
        assert!(p.entries.is_empty());
    }

    #[test]
    fn hidden_loads_finish_into_the_cache() {
        let dir = tempfile::tempdir().unwrap();
        let a = write_png(dir.path(), "a.png");
        let b = write_png(dir.path(), "b.png");
        let mut p = Previewer::new(Picker::halfblocks());
        p.show(a.clone(), vec![b.clone()]);
        p.hide();
        assert_eq!(p.current(), None);
        assert_eq!(p.title(), None);
        assert!(!p.busy());
        wait_loaded(&mut p, &a);
        wait_loaded(&mut p, &b);
        // Showing them again needs no new load.
        p.show(b.clone(), vec![a.clone()]);
        assert_eq!(p.title().as_deref(), Some("b.png"));
        assert!(p.queue.lock().loads.is_empty());
    }

    #[test]
    fn a_changed_file_is_loaded_again() {
        let dir = tempfile::tempdir().unwrap();
        let a = write_png(dir.path(), "a.png");
        let mut p = Previewer::new(Picker::halfblocks());
        p.show(a.clone(), vec![]);
        wait_loaded(&mut p, &a);
        let later = SystemTime::now() + std::time::Duration::from_secs(5);
        std::fs::File::options()
            .write(true)
            .open(&a)
            .unwrap()
            .set_modified(later)
            .unwrap();
        p.show(a.clone(), vec![]);
        assert!(p.entries.iter().all(|e| e.path != a));
        assert!(p.busy());
    }

    #[test]
    fn busy_until_results_are_received_and_encoded_for_the_drawn_area() {
        let dir = tempfile::tempdir().unwrap();
        let a = write_png(dir.path(), "a.png");
        let mut p = Previewer::new(Picker::halfblocks());
        let inner = Size::new(30, 12);
        p.queue.lock().area = Some(inner);
        p.show(a.clone(), vec![]);
        assert!(p.busy());
        wait_loaded(&mut p, &a);
        assert!(!p.busy());
        let e = p.entries.iter().find(|e| e.path == a).unwrap();
        let Image::Ready(protocol) = &e.image else {
            panic!("no image");
        };
        // Already encoded for the area the draw will use.
        let img = image_area(Rect::new(0, 0, inner.width, inner.height), e.info.len());
        let fitted = protocol.size_for(RESIZE, img.as_size());
        assert_eq!(protocol.needs_resize(&RESIZE, fitted), None);
    }

    #[test]
    fn undecodable_image_shows_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let bad = dir.path().join("bad.png");
        std::fs::write(&bad, b"not a png").unwrap();
        let mut p = Previewer::new(Picker::halfblocks());
        p.show(bad.clone(), vec![]);
        wait_loaded(&mut p, &bad);
        let e = &p.entries[0];
        assert!(matches!(e.image, Image::None));
        assert!(e.info[0].starts_with("Cannot decode image"));
    }

    #[test]
    fn new_request_replaces_pending_loads() {
        let q = Queue::default();
        let paths: Vec<PathBuf> = ["a", "b", "c"].iter().map(PathBuf::from).collect();
        q.set_loads(&paths);
        q.set_loads(&paths[2..]);
        let jobs = q.lock();
        assert_eq!(jobs.loads.len(), 1);
        assert!(matches!(&jobs.loads[0], Job::Load(path) if path == &paths[2]));
    }

    #[test]
    fn eviction_keeps_the_shown_entry() {
        let mut p = Previewer::new(Picker::halfblocks());
        p.wanted = vec![PathBuf::from("0")];
        for i in 0..CACHE_ENTRIES + 3 {
            p.entries.push(Entry {
                path: PathBuf::from(i.to_string()),
                stamp: None,
                title: String::new(),
                info: vec![],
                image: Image::None,
                bytes: 1,
            });
        }
        p.evict();
        assert_eq!(p.entries.len(), CACHE_ENTRIES);
        assert!(p.entries.iter().any(|e| e.path == Path::new("0")));

        // Byte limit: one huge entry besides the shown one is dropped.
        p.entries[1].bytes = CACHE_BYTES;
        p.evict();
        assert!(p.entries.iter().all(|e| e.bytes < CACHE_BYTES));
        assert!(p.entries.iter().any(|e| e.path == Path::new("0")));
    }
}
