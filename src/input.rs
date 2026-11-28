//! Key input: turns key presses into [`Command`]s.
//!
//! All key bindings live in this file: the per-mode tables below plus the
//! few keys that need special handling in [`KeyState::translate`] (count
//! prefix, `gg` / `G`, `q`, text input, dialogs, `Ctrl` in the viewer).

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use ratatui::{backend::Backend, Terminal};
use std::ops::ControlFlow;
use std::sync::mpsc;
use std::time::{Duration, Instant, SystemTime};
use std::{io, io::Write};

use crate::app::App;
use crate::command::Command;
use crate::delete_confirm::{confirm_key, ConfirmAction, DeleteKind};
use crate::mode::Mode;
use crate::paste::conflict_choice;
use crate::watch::{self, DirWatcher, FsEvent};

type Bindings = &'static [(KeyCode, Command)];

/// File list.
const FILER_KEYS: Bindings = &[
    (KeyCode::Char('j'), Command::MoveDown(1)),
    (KeyCode::Char('k'), Command::MoveUp(1)),
    (KeyCode::Char('h'), Command::MoveLeft),
    (KeyCode::Char('l'), Command::MoveRight),
    (KeyCode::Enter, Command::Open),
    (KeyCode::Char('o'), Command::OpenExternal),
    (KeyCode::Char('e'), Command::OpenEditor),
    (KeyCode::Char('v'), Command::ToggleMark),
    (KeyCode::Char('V'), Command::VisualStart),
    (KeyCode::Char('y'), Command::Yank),
    (KeyCode::Char('p'), Command::Paste),
    (KeyCode::Char('P'), Command::ToggleTextPreview),
    (KeyCode::Char('m'), Command::ToggleMarkdown),
    (KeyCode::Char('Y'), Command::CopyAtPaths),
    (KeyCode::Char('x'), Command::Delete(DeleteKind::Normal)),
    (KeyCode::Char('X'), Command::Delete(DeleteKind::Permanent)),
    (KeyCode::Char('r'), Command::RenameStart),
    (KeyCode::Char('/'), Command::SearchStart),
    (KeyCode::Char('s'), Command::SortMenu),
    (KeyCode::Char('M'), Command::ToggleGitFilter),
    (KeyCode::Char('.'), Command::ToggleHidden),
    (KeyCode::Char('i'), Command::ToggleIgnored),
    (KeyCode::Char('n'), Command::SearchNext),
    (KeyCode::Char('N'), Command::SearchPrev),
    (KeyCode::Char('D'), Command::Diff),
    (KeyCode::Char('R'), Command::Refresh),
];

/// Visual (range) selection.
const VISUAL_KEYS: Bindings = &[
    (KeyCode::Char('j'), Command::MoveDown(1)),
    (KeyCode::Char('k'), Command::MoveUp(1)),
    (KeyCode::Char('V'), Command::Back),
    (KeyCode::Esc, Command::Back),
];

/// Text viewer.
const VIEWER_KEYS: Bindings = &[
    (KeyCode::Char('j'), Command::MoveDown(1)),
    (KeyCode::Char('k'), Command::MoveUp(1)),
    (KeyCode::PageDown, Command::PageDown(1)),
    (KeyCode::PageUp, Command::PageUp(1)),
    (KeyCode::Char('m'), Command::ToggleMarkdown),
    (KeyCode::Enter, Command::Back),
];

/// `Ctrl` + key in the text and diff viewers. Elsewhere `Ctrl` is ignored
/// and the key acts as typed without it.
const VIEWER_CTRL_KEYS: Bindings = &[
    (KeyCode::Char('f'), Command::PageDown(1)),
    (KeyCode::Char('b'), Command::PageUp(1)),
    (KeyCode::Char('d'), Command::HalfPageDown(1)),
    (KeyCode::Char('u'), Command::HalfPageUp(1)),
];

/// Diff viewer (`D`): like the text viewer, but `q` / `Esc` also close it.
const DIFF_KEYS: Bindings = &[
    (KeyCode::Char('j'), Command::MoveDown(1)),
    (KeyCode::Char('k'), Command::MoveUp(1)),
    (KeyCode::PageDown, Command::PageDown(1)),
    (KeyCode::PageUp, Command::PageUp(1)),
    (KeyCode::Enter, Command::Back),
    (KeyCode::Esc, Command::Back),
    (KeyCode::Char('q'), Command::Back),
];

/// Image / video preview.
const PREVIEW_KEYS: Bindings = &[
    (KeyCode::Char('j'), Command::MoveDown(1)),
    (KeyCode::Char('k'), Command::MoveUp(1)),
    (KeyCode::Enter, Command::Back),
    (KeyCode::Esc, Command::Back),
    (KeyCode::Char('o'), Command::OpenExternal),
];

/// Sort menu.
const SORT_KEYS: Bindings = &[
    (KeyCode::Char('j'), Command::MoveDown(1)),
    (KeyCode::Down, Command::MoveDown(1)),
    (KeyCode::Char('k'), Command::MoveUp(1)),
    (KeyCode::Up, Command::MoveUp(1)),
    (KeyCode::Enter, Command::SortConfirm),
    (KeyCode::Esc, Command::Back),
];

/// Agent choice for `Y` (send `@path` in herdr).
const SEND_TARGET_KEYS: Bindings = &[
    (KeyCode::Char('j'), Command::MoveDown(1)),
    (KeyCode::Down, Command::MoveDown(1)),
    (KeyCode::Char('k'), Command::MoveUp(1)),
    (KeyCode::Up, Command::MoveUp(1)),
    (KeyCode::Enter, Command::SendConfirm),
    (KeyCode::Esc, Command::Back),
    (KeyCode::Char('q'), Command::Back),
];

fn lookup(table: Bindings, code: KeyCode) -> Option<Command> {
    table.iter().find(|(k, _)| *k == code).map(|&(_, cmd)| cmd)
}

/// Key bindings of the current mode (after the global keys are handled).
fn mode_command(mode: &Mode, code: KeyCode) -> Option<Command> {
    match mode {
        Mode::Filer => lookup(FILER_KEYS, code),
        Mode::Visual { .. } => lookup(VISUAL_KEYS, code),
        Mode::Viewer { diff: true, .. } => lookup(DIFF_KEYS, code),
        Mode::Viewer { .. } => lookup(VIEWER_KEYS, code),
        Mode::Preview => lookup(PREVIEW_KEYS, code),
        Mode::Sort { .. } => lookup(SORT_KEYS, code),
        Mode::SendTarget { .. } => lookup(SEND_TARGET_KEYS, code),
        Mode::Search { .. } => Some(match code {
            KeyCode::Char(c) => Command::InsertChar(c),
            KeyCode::Backspace => Command::DeleteChar,
            KeyCode::Enter => Command::SearchConfirm,
            KeyCode::Esc => Command::Back,
            // Any other key searches again from the cursor.
            _ => Command::SearchNext,
        }),
        Mode::Rename { .. } => match code {
            KeyCode::Char(c) => Some(Command::InsertChar(c)),
            KeyCode::Backspace => Some(Command::DeleteChar),
            KeyCode::Enter => Some(Command::RenameCommit),
            KeyCode::Esc => Some(Command::Back),
            _ => None,
        },
        Mode::ConfirmDelete { .. } => match confirm_key(code) {
            ConfirmAction::Delete => Some(Command::ConfirmDelete),
            ConfirmAction::Cancel => Some(Command::Back),
            ConfirmAction::Ignore => None,
        },
        Mode::PasteConflict { .. } => conflict_choice(code).map(Command::ResolvePaste),
    }
}

/// Keys typed so far that are waiting for the rest of a sequence.
#[derive(Default)]
pub struct KeyState {
    /// Numeric count prefix (`5j`); 0 = none.
    prefix: usize,
    /// The previous key was `g` (first half of `gg`).
    last_key_g: bool,
}

impl KeyState {
    /// Turn a key press in the given mode into a command, if it completes one.
    pub fn translate(&mut self, mode: &Mode, key: KeyEvent) -> Option<Command> {
        let code = key.code;
        let viewer_ctrl = matches!(mode, Mode::Viewer { .. })
            && key.modifiers.contains(KeyModifiers::CONTROL)
            && lookup(VIEWER_CTRL_KEYS, code).is_some();
        // Text-input modes must receive every character (including 'q', 'g', 'G').
        let text_input = matches!(mode, Mode::Search { .. } | Mode::Rename { .. });
        // In dialogs 'q' never quits (the delete and send target dialogs
        // treat it as cancel).
        let in_dialog = matches!(
            mode,
            Mode::ConfirmDelete { .. } | Mode::PasteConflict { .. } | Mode::SendTarget { .. }
        );
        // In the diff viewer 'q' closes the diff (see DIFF_KEYS).
        let q_closes = matches!(mode, Mode::Viewer { diff: true, .. });
        if code == KeyCode::Char('q') && !text_input && !in_dialog && !q_closes {
            return Some(Command::Quit);
        }

        if let KeyCode::Char(c) = code {
            if matches!(mode, Mode::Filer | Mode::Viewer { .. } | Mode::Preview)
                && c.is_ascii_digit()
            {
                self.prefix = self
                    .prefix
                    .saturating_mul(10)
                    .saturating_add(c.to_digit(10).unwrap() as usize);
                return None;
            }
        }
        let count = if self.prefix > 0 { self.prefix } else { 1 };
        self.prefix = 0;

        if viewer_ctrl {
            self.last_key_g = false;
            return lookup(VIEWER_CTRL_KEYS, code).map(|cmd| cmd.with_count(count));
        }

        // Vim-style 'gg' (go to top) and 'G' (go to bottom), in every
        // non-text mode (they only have an effect in the list and viewer).
        if let (KeyCode::Char(c), false) = (code, text_input) {
            match c {
                'g' => {
                    if self.last_key_g {
                        self.last_key_g = false;
                        return Some(Command::GoTop);
                    }
                    self.last_key_g = true;
                    return None;
                }
                'G' => {
                    self.last_key_g = false;
                    return Some(Command::GoBottom);
                }
                _ => self.last_key_g = false,
            }
        }

        mode_command(mode, code).map(|cmd| cmd.with_count(count))
    }
}

/// Longest wait for a key before checking for directory changes. Input is
/// read on this thread only (never in the background), so an editor started
/// with `e` gets every key; the loop just wakes this often without drawing.
const TICK: Duration = Duration::from_millis(200);

/// Wait while a preview loads in the background, so it shows soon after it
/// is ready (the worker threads cannot wake `event::poll`).
const LOADING_TICK: Duration = Duration::from_millis(20);

/// How long to wait for input: until the next highlight expires, at most
/// [`TICK`] ([`LOADING_TICK`] while a preview loads).
fn poll_timeout(highlight_expiry: Option<Duration>, loading: bool) -> Duration {
    let tick = if loading { LOADING_TICK } else { TICK };
    highlight_expiry.map_or(tick, |d| d.min(tick))
}

/// Main event loop: wait for a key, a directory change or the expiry of a
/// "just changed" highlight, and redraw only when one of them happened.
/// `typeahead` are keys typed before the loop started (see
/// [`crate::graphics::detect`]); they are handled first, in order.
pub fn run_app<B: Backend<Error = io::Error> + Write>(
    terminal: &mut Terminal<B>,
    app: &mut App,
    typeahead: Vec<KeyCode>,
) -> io::Result<()> {
    let (tx, fs_events) = mpsc::channel::<FsEvent>();
    let mode = watch::parse_watch_mode(std::env::var(watch::WATCH_ENV).ok().as_deref());
    let mut watcher = match DirWatcher::start(mode, tx, |e| e) {
        Ok(w) => w,
        Err(e) => {
            app.notify_error(format!("File watching disabled: {e}"));
            None
        }
    };
    let mut keys = KeyState::default();
    for code in typeahead {
        if handle_key(terminal, app, &mut keys, KeyEvent::from(code))?.is_break() {
            return Ok(());
        }
    }
    let mut dirty = true;
    loop {
        if let Some(w) = watcher.as_mut() {
            if let Some((dir, e)) = w.sync(&app.watch_targets()).into_iter().next() {
                let name = dir.file_name().map_or_else(
                    || dir.display().to_string(),
                    |n| n.to_string_lossy().into_owned(),
                );
                app.notify_error(format!("Cannot watch: {e}: {name}"));
            }
        }
        if dirty {
            terminal.draw(|f| crate::ui::ui(f, app))?;
            dirty = false;
        }
        let expiry = app.next_highlight_expiry(SystemTime::now());
        let started = Instant::now();
        if event::poll(poll_timeout(expiry, app.preview.busy()))? {
            match event::read()? {
                Event::Key(key) => {
                    dirty = true;
                    if handle_key(terminal, app, &mut keys, key)?.is_break() {
                        return Ok(());
                    }
                }
                Event::Resize(..) => dirty = true,
                _ => {}
            }
        }
        while let Ok(fs_event) = fs_events.try_recv() {
            app.handle_fs_event(&fs_event);
            dirty = true;
        }
        if app.preview.poll() {
            dirty = true;
        }
        if expiry.is_some_and(|d| started.elapsed() >= d) {
            dirty = true;
        }
    }
}

/// Handle one key press; `Break` when the app should quit.
fn handle_key<B: Backend<Error = io::Error> + Write>(
    terminal: &mut Terminal<B>,
    app: &mut App,
    keys: &mut KeyState,
    key: KeyEvent,
) -> io::Result<ControlFlow<()>> {
    // A status message stays until the next key press.
    app.message = None;
    if let Some(cmd) = keys.translate(&app.mode, key) {
        if app.apply(cmd).is_break() {
            return Ok(ControlFlow::Break(()));
        }
        if let Some(path) = app.pending_edit.take() {
            // Changes made meanwhile are queued and handled in the loop.
            crate::editor::edit(terminal, app, &path)?;
        }
    }
    Ok(ControlFlow::Continue(()))
}

/// Turn raw bytes typed before the event loop started into key presses.
///
/// Covers what is plausible right after launch: text (UTF-8), Enter, Tab,
/// Backspace, Esc and the arrow / navigation keys. Modifiers are not kept
/// (the key tables do not use them); other control bytes and unknown escape
/// sequences are dropped rather than guessed at. Esc followed by a key is
/// read as Esc, then that key.
pub fn typeahead_keys(bytes: &[u8]) -> Vec<KeyCode> {
    let mut keys = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        match b {
            0x1b => {
                let intro = bytes.get(i + 1).copied();
                if matches!(intro, Some(b'[') | Some(b'O')) {
                    let start = i + 2;
                    let end = (start..bytes.len())
                        .find(|&j| !(0x20..=0x3f).contains(&bytes[j]))
                        .unwrap_or(bytes.len());
                    if end < bytes.len() {
                        if let Some(code) = csi_key(&bytes[start..end], bytes[end]) {
                            keys.push(code);
                        }
                        i = end + 1;
                        continue;
                    }
                }
                keys.push(KeyCode::Esc);
                i += 1;
            }
            b'\r' | b'\n' => {
                keys.push(KeyCode::Enter);
                i += 1;
            }
            b'\t' => {
                keys.push(KeyCode::Tab);
                i += 1;
            }
            0x7f | 0x08 => {
                keys.push(KeyCode::Backspace);
                i += 1;
            }
            0x00..=0x1f => i += 1,
            _ => {
                let len = match b {
                    0xc0..=0xdf => 2,
                    0xe0..=0xef => 3,
                    0xf0..=0xf7 => 4,
                    _ => 1,
                };
                let end = (i + len).min(bytes.len());
                if let Some(c) = std::str::from_utf8(&bytes[i..end])
                    .ok()
                    .and_then(|s| s.chars().next())
                {
                    keys.push(KeyCode::Char(c));
                    i = end;
                } else {
                    i += 1;
                }
            }
        }
    }
    keys
}

/// Key for `ESC [ <params> <fin>` / `ESC O <fin>`.
fn csi_key(params: &[u8], fin: u8) -> Option<KeyCode> {
    Some(match (params, fin) {
        (_, b'A') => KeyCode::Up,
        (_, b'B') => KeyCode::Down,
        (_, b'C') => KeyCode::Right,
        (_, b'D') => KeyCode::Left,
        (_, b'H') => KeyCode::Home,
        (_, b'F') => KeyCode::End,
        (_, b'Z') => KeyCode::BackTab,
        (b"1" | b"7", b'~') => KeyCode::Home,
        (b"2", b'~') => KeyCode::Insert,
        (b"3", b'~') => KeyCode::Delete,
        (b"4" | b"8", b'~') => KeyCode::End,
        (b"5", b'~') => KeyCode::PageUp,
        (b"6", b'~') => KeyCode::PageDown,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::delete_confirm::DeleteKind;
    use crate::paste::{ConflictChoice, Resolution};
    use crate::text_view::TextDoc;
    use std::path::PathBuf;

    fn ch(c: char) -> KeyCode {
        KeyCode::Char(c)
    }

    /// Translate a key sequence in a fixed mode, collecting the commands.
    fn run(mode: &Mode, keys: &[KeyCode]) -> Vec<Command> {
        let mut st = KeyState::default();
        keys.iter()
            .filter_map(|&k| st.translate(mode, KeyEvent::from(k)))
            .collect()
    }

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    /// Like [`run`], with modifiers.
    fn run_keys(mode: &Mode, keys: &[KeyEvent]) -> Vec<Command> {
        let mut st = KeyState::default();
        keys.iter().filter_map(|&k| st.translate(mode, k)).collect()
    }

    fn search() -> Mode {
        Mode::Search {
            query: String::new(),
        }
    }

    fn rename() -> Mode {
        Mode::Rename {
            original: "a".into(),
            buffer: "a".into(),
        }
    }

    fn confirm() -> Mode {
        Mode::ConfirmDelete {
            items: vec![PathBuf::from("/x")],
            kind: DeleteKind::Normal,
        }
    }

    fn viewer() -> Mode {
        Mode::Viewer {
            doc: TextDoc::plain(String::new(), ""),
            offset: 0,
            at_end: false,
            diff: false,
        }
    }

    fn diff_viewer() -> Mode {
        Mode::Viewer {
            doc: TextDoc::plain(String::new(), ""),
            offset: 0,
            at_end: false,
            diff: true,
        }
    }

    #[test]
    fn filer_keys() {
        let f = Mode::Filer;
        assert_eq!(run(&f, &[ch('j')]), [Command::MoveDown(1)]);
        assert_eq!(run(&f, &[ch('h')]), [Command::MoveLeft]);
        assert_eq!(run(&f, &[KeyCode::Enter]), [Command::Open]);
        assert_eq!(
            run(&f, &[ch('X')]),
            [Command::Delete(DeleteKind::Permanent)]
        );
        assert_eq!(run(&f, &[ch('x')]), [Command::Delete(DeleteKind::Normal)]);
        assert_eq!(run(&f, &[ch('p')]), [Command::Paste]);
        assert_eq!(run(&f, &[ch('P')]), [Command::ToggleTextPreview]);
        assert!(run(&viewer(), &[ch('P')]).is_empty());
        assert!(run(&Mode::Preview, &[ch('P')]).is_empty());
        assert_eq!(run(&f, &[ch('Y')]), [Command::CopyAtPaths]);
        assert_eq!(run(&f, &[ch('M')]), [Command::ToggleGitFilter]);
        assert_eq!(run(&f, &[ch('D')]), [Command::Diff]);
        assert_eq!(run(&f, &[ch('e')]), [Command::OpenEditor]);
        assert_eq!(run(&search(), &[ch('e')]), [Command::InsertChar('e')]);
        assert!(run(&viewer(), &[ch('e')]).is_empty());
        assert_eq!(run(&f, &[ch('R')]), [Command::Refresh]);
        assert_eq!(run(&search(), &[ch('R')]), [Command::InsertChar('R')]);
        assert_eq!(
            run(
                &Mode::Visual {
                    anchor: PathBuf::new()
                },
                &[ch('R')]
            ),
            []
        );
        assert_eq!(run(&search(), &[ch('M')]), [Command::InsertChar('M')]);
        assert_eq!(run(&f, &[ch('.')]), [Command::ToggleHidden]);
        assert_eq!(run(&f, &[ch('i')]), [Command::ToggleIgnored]);
        assert_eq!(run(&f, &[ch('n')]), [Command::SearchNext]);
        assert_eq!(run(&f, &[ch('N')]), [Command::SearchPrev]);
        for c in ['.', 'i', 'n', 'N'] {
            assert_eq!(run(&search(), &[ch(c)]), [Command::InsertChar(c)]);
        }
        assert_eq!(run(&f, &[ch('z')]), []);
        assert_eq!(run(&f, &[KeyCode::Down]), []);
    }

    #[test]
    fn typeahead_decodes_text_and_common_keys() {
        assert_eq!(
            typeahead_keys("jé\r\t\x7f".as_bytes()),
            [
                ch('j'),
                ch('é'),
                KeyCode::Enter,
                KeyCode::Tab,
                KeyCode::Backspace
            ]
        );
        assert_eq!(
            typeahead_keys(b"\x1b[A\x1bOB\x1b[5~\x1b[3~\x1b[Z"),
            [
                KeyCode::Up,
                KeyCode::Down,
                KeyCode::PageUp,
                KeyCode::Delete,
                KeyCode::BackTab
            ]
        );
    }

    #[test]
    fn typeahead_escape_control_and_unknown_bytes() {
        // Lone Esc, and Esc followed by a key.
        assert_eq!(typeahead_keys(b"\x1b"), [KeyCode::Esc]);
        assert_eq!(typeahead_keys(b"\x1bq"), [KeyCode::Esc, ch('q')]);
        // Other control bytes, unknown sequences and broken UTF-8 are dropped.
        assert_eq!(typeahead_keys(b"\x03\x1b[99~\xffk"), [ch('k')]);
        // A truncated sequence keeps its Esc and the rest as text.
        assert_eq!(typeahead_keys(b"\x1b[1"), [KeyCode::Esc, ch('['), ch('1')]);
    }

    #[test]
    fn typeahead_q_quits_like_a_normal_key_press() {
        // The bytes left over after the terminal replies were split off.
        let scan = crate::graphics::split_replies(b"q\x1b[?62;4c\x1b[0n");
        let keys = typeahead_keys(&scan.input);
        assert_eq!(run(&Mode::Filer, &keys), [Command::Quit]);
        let scan = crate::graphics::split_replies(b"\x1b[0nj2");
        assert_eq!(typeahead_keys(&scan.input), [ch('j'), ch('2')]);
    }

    #[test]
    fn poll_timeout_waits_for_the_next_highlight_expiry_at_most_a_tick() {
        assert_eq!(poll_timeout(None, false), TICK);
        assert_eq!(poll_timeout(None, true), LOADING_TICK);
        assert_eq!(
            poll_timeout(Some(Duration::from_millis(50)), false),
            Duration::from_millis(50)
        );
        assert_eq!(poll_timeout(Some(Duration::from_secs(9)), false), TICK);
    }

    #[test]
    fn count_prefix_applies_to_movement_only() {
        let f = Mode::Filer;
        assert_eq!(
            run(&f, &[ch('1'), ch('2'), ch('j')]),
            [Command::MoveDown(12)]
        );
        assert_eq!(
            run(&f, &[ch('3'), ch('k'), ch('k')]),
            [Command::MoveUp(3), Command::MoveUp(1)]
        );
        assert_eq!(run(&f, &[ch('3'), ch('y')]), [Command::Yank]);
        assert_eq!(run(&viewer(), &[ch('4'), ch('j')]), [Command::MoveDown(4)]);
    }

    #[test]
    fn digits_are_not_a_count_outside_filer_and_viewer() {
        let visual = Mode::Visual {
            anchor: PathBuf::new(),
        };
        assert_eq!(run(&visual, &[ch('5'), ch('j')]), [Command::MoveDown(1)]);
        assert_eq!(run(&search(), &[ch('5')]), [Command::InsertChar('5')]);
    }

    #[test]
    fn gg_and_g() {
        let f = Mode::Filer;
        assert_eq!(run(&f, &[ch('g'), ch('g')]), [Command::GoTop]);
        assert_eq!(run(&f, &[ch('G')]), [Command::GoBottom]);
        assert_eq!(run(&f, &[ch('5'), ch('g'), ch('g')]), [Command::GoTop]);
        // Another character key breaks the sequence ...
        assert_eq!(run(&f, &[ch('g'), ch('v'), ch('g')]), [Command::ToggleMark]);
        // ... but non-character keys and digits do not.
        assert_eq!(
            run(&f, &[ch('g'), KeyCode::Down, ch('g')]),
            [Command::GoTop]
        );
        assert_eq!(run(&f, &[ch('g'), ch('3'), ch('g')]), [Command::GoTop]);
        // g / G are swallowed in dialogs and menus too.
        assert_eq!(run(&confirm(), &[ch('G')]), [Command::GoBottom]);
        assert_eq!(run(&Mode::Sort { selected: 0 }, &[ch('g')]), []);
    }

    #[test]
    fn q_quits_except_in_text_input_and_dialogs() {
        for mode in [
            Mode::Filer,
            Mode::Visual {
                anchor: PathBuf::new(),
            },
            viewer(),
            Mode::Preview,
            Mode::Sort { selected: 0 },
        ] {
            assert_eq!(run(&mode, &[ch('q')]), [Command::Quit]);
        }
        assert_eq!(run(&search(), &[ch('q')]), [Command::InsertChar('q')]);
        assert_eq!(run(&rename(), &[ch('q')]), [Command::InsertChar('q')]);
        assert_eq!(run(&confirm(), &[ch('q')]), [Command::Back]);
    }

    #[test]
    fn text_input_keys() {
        let s = search();
        assert_eq!(
            run(&s, &[ch('g'), ch('G')]),
            [Command::InsertChar('g'), Command::InsertChar('G')]
        );
        assert_eq!(run(&s, &[KeyCode::Backspace]), [Command::DeleteChar]);
        assert_eq!(run(&s, &[KeyCode::Enter]), [Command::SearchConfirm]);
        assert_eq!(run(&s, &[KeyCode::Esc]), [Command::Back]);
        assert_eq!(run(&s, &[KeyCode::Left]), [Command::SearchNext]);

        let r = rename();
        assert_eq!(run(&r, &[KeyCode::Enter]), [Command::RenameCommit]);
        assert_eq!(run(&r, &[KeyCode::Esc]), [Command::Back]);
        assert_eq!(run(&r, &[KeyCode::Left]), []);
    }

    #[test]
    fn visual_swallows_other_keys() {
        let v = Mode::Visual {
            anchor: PathBuf::new(),
        };
        assert_eq!(run(&v, &[ch('x'), KeyCode::Enter, ch('y')]), []);
        assert_eq!(run(&v, &[ch('V')]), [Command::Back]);
        assert_eq!(run(&v, &[KeyCode::Esc]), [Command::Back]);
    }

    #[test]
    fn viewer_preview_and_sort_keys() {
        assert_eq!(run(&viewer(), &[KeyCode::Enter]), [Command::Back]);
        assert_eq!(run(&viewer(), &[KeyCode::Esc]), []);
        assert_eq!(run(&Mode::Preview, &[KeyCode::Esc]), [Command::Back]);
        assert_eq!(run(&Mode::Preview, &[ch('o')]), [Command::OpenExternal]);
        assert_eq!(run(&Mode::Preview, &[KeyCode::Enter]), [Command::Back]);
        assert_eq!(run(&Mode::Preview, &[ch('j')]), [Command::MoveDown(1)]);
        assert_eq!(run(&Mode::Preview, &[ch('k')]), [Command::MoveUp(1)]);
        assert_eq!(
            run(&Mode::Preview, &[ch('1'), ch('2'), ch('j')]),
            [Command::MoveDown(12)]
        );
        assert_eq!(
            run(&Mode::Preview, &[ch('3'), ch('k')]),
            [Command::MoveUp(3)]
        );
        let s = Mode::Sort { selected: 0 };
        assert_eq!(run(&s, &[KeyCode::Down]), [Command::MoveDown(1)]);
        assert_eq!(run(&s, &[ch('k')]), [Command::MoveUp(1)]);
        assert_eq!(run(&s, &[KeyCode::Enter]), [Command::SortConfirm]);
    }

    #[test]
    fn send_target_keys() {
        let t = Mode::SendTarget {
            targets: vec![],
            selected: 0,
            paths: vec![],
        };
        assert_eq!(run(&t, &[ch('j')]), [Command::MoveDown(1)]);
        assert_eq!(run(&t, &[KeyCode::Down]), [Command::MoveDown(1)]);
        assert_eq!(run(&t, &[ch('k')]), [Command::MoveUp(1)]);
        assert_eq!(run(&t, &[KeyCode::Up]), [Command::MoveUp(1)]);
        assert_eq!(run(&t, &[KeyCode::Enter]), [Command::SendConfirm]);
        assert_eq!(run(&t, &[KeyCode::Esc]), [Command::Back]);
        // 'q' cancels instead of quitting kura.
        assert_eq!(run(&t, &[ch('q')]), [Command::Back]);
        // Filer keys do nothing in the dialog.
        assert_eq!(run(&t, &[ch('Y'), ch('x')]), []);
    }

    #[test]
    fn dialogs_use_their_own_key_functions() {
        let c = confirm();
        assert_eq!(run(&c, &[ch('y')]), [Command::ConfirmDelete]);
        assert_eq!(run(&c, &[KeyCode::Enter]), [Command::Back]);
        assert_eq!(run(&c, &[ch('j')]), []);

        let p = Mode::PasteConflict {
            pending: crate::paste::PendingPaste {
                planner: crate::paste::Planner::new(vec![], PathBuf::from("/")),
                failures: vec![],
            },
        };
        assert_eq!(run(&p, &[ch('q')]), []);
        assert_eq!(
            run(&p, &[ch('O')]),
            [Command::ResolvePaste(ConflictChoice::Resolve(
                Resolution::Overwrite,
                true
            ))]
        );
        assert_eq!(
            run(&p, &[KeyCode::Esc]),
            [Command::ResolvePaste(ConflictChoice::Cancel)]
        );
    }

    #[test]
    fn diff_viewer_keys() {
        let d = diff_viewer();
        for key in [ch('q'), KeyCode::Esc, KeyCode::Enter] {
            assert_eq!(run(&d, &[key]), [Command::Back]);
        }
        assert_eq!(run(&d, &[ch('4'), ch('j')]), [Command::MoveDown(4)]);
        assert_eq!(run(&d, &[ch('k')]), [Command::MoveUp(1)]);
        assert_eq!(run(&d, &[ch('g'), ch('g')]), [Command::GoTop]);
        assert_eq!(run(&d, &[ch('G')]), [Command::GoBottom]);
        assert_eq!(run(&d, &[ch('D')]), []);
        // The plain text viewer keeps its keys.
        assert_eq!(run(&viewer(), &[ch('q')]), [Command::Quit]);
    }

    #[test]
    fn viewer_page_keys() {
        for mode in [viewer(), diff_viewer()] {
            assert_eq!(
                run(&mode, &[KeyCode::PageDown, KeyCode::PageUp]),
                [Command::PageDown(1), Command::PageUp(1)]
            );
            assert_eq!(
                run_keys(&mode, &[ctrl('f'), ctrl('b'), ctrl('d'), ctrl('u')]),
                [
                    Command::PageDown(1),
                    Command::PageUp(1),
                    Command::HalfPageDown(1),
                    Command::HalfPageUp(1),
                ]
            );
            // Without Ctrl the letters do nothing.
            assert!(run(&mode, &[ch('f'), ch('b'), ch('d'), ch('u')]).is_empty());
        }
        // A count scrolls that many pages.
        assert_eq!(
            run_keys(&viewer(), &[KeyEvent::from(ch('3')), ctrl('d')]),
            [Command::HalfPageDown(3)]
        );
        assert_eq!(
            run(&viewer(), &[ch('2'), KeyCode::PageDown]),
            [Command::PageDown(2)]
        );
        // Not in the file list or the preview (out of scope there).
        for mode in [Mode::Filer, Mode::Preview] {
            assert!(run(&mode, &[KeyCode::PageDown, KeyCode::PageUp]).is_empty());
            assert!(run_keys(&mode, &[ctrl('f'), ctrl('b'), ctrl('d'), ctrl('u')]).is_empty());
        }
        // Other Ctrl keys still act as the plain key.
        assert_eq!(run_keys(&viewer(), &[ctrl('j')]), [Command::MoveDown(1)]);
    }

    #[test]
    fn m_switches_markdown_in_the_viewer_and_the_file_list() {
        assert_eq!(run(&viewer(), &[ch('m')]), [Command::ToggleMarkdown]);
        assert_eq!(run(&Mode::Filer, &[ch('m')]), [Command::ToggleMarkdown]);
        // `M` stays the git filter; diffs and Visual mode ignore `m`.
        assert_eq!(run(&Mode::Filer, &[ch('M')]), [Command::ToggleGitFilter]);
        assert!(run(&diff_viewer(), &[ch('m')]).is_empty());
        let visual = Mode::Visual {
            anchor: PathBuf::from("/x"),
        };
        assert!(run(&visual, &[ch('m')]).is_empty());
    }
}
