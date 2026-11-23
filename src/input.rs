//! Key input: turns key presses into [`Command`]s.
//!
//! All key bindings live in this file: the per-mode tables below plus the
//! few keys that need special handling in [`KeyState::translate`] (count
//! prefix, `gg` / `G`, `q`, text input, dialogs).

use crossterm::event::{self, Event, KeyCode};
use ratatui::{backend::Backend, Terminal};
use std::time::Duration;
use std::{io, io::Write};

use crate::app::App;
use crate::command::Command;
use crate::delete_confirm::{confirm_key, ConfirmAction, DeleteKind};
use crate::mode::Mode;
use crate::paste::conflict_choice;

type Bindings = &'static [(KeyCode, Command)];

/// File list.
const FILER_KEYS: Bindings = &[
    (KeyCode::Char('j'), Command::MoveDown(1)),
    (KeyCode::Char('k'), Command::MoveUp(1)),
    (KeyCode::Char('h'), Command::MoveLeft),
    (KeyCode::Char('l'), Command::MoveRight),
    (KeyCode::Enter, Command::Open),
    (KeyCode::Char('o'), Command::OpenExternal),
    (KeyCode::Char('v'), Command::ToggleMark),
    (KeyCode::Char('V'), Command::VisualStart),
    (KeyCode::Char('y'), Command::Yank),
    (KeyCode::Char('p'), Command::Paste),
    (KeyCode::Char('x'), Command::Delete(DeleteKind::Normal)),
    (KeyCode::Char('X'), Command::Delete(DeleteKind::Permanent)),
    (KeyCode::Char('r'), Command::RenameStart),
    (KeyCode::Char('/'), Command::SearchStart),
    (KeyCode::Char('s'), Command::SortMenu),
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
    (KeyCode::Enter, Command::Back),
];

/// Image / video preview.
const PREVIEW_KEYS: Bindings = &[
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

fn lookup(table: Bindings, code: KeyCode) -> Option<Command> {
    table.iter().find(|(k, _)| *k == code).map(|&(_, cmd)| cmd)
}

/// Key bindings of the current mode (after the global keys are handled).
fn mode_command(mode: &Mode, code: KeyCode) -> Option<Command> {
    match mode {
        Mode::Filer => lookup(FILER_KEYS, code),
        Mode::Visual { .. } => lookup(VISUAL_KEYS, code),
        Mode::Viewer { .. } => lookup(VIEWER_KEYS, code),
        Mode::Preview => lookup(PREVIEW_KEYS, code),
        Mode::Sort { .. } => lookup(SORT_KEYS, code),
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
    pub fn translate(&mut self, mode: &Mode, code: KeyCode) -> Option<Command> {
        // Text-input modes must receive every character (including 'q', 'g', 'G').
        let text_input = matches!(mode, Mode::Search { .. } | Mode::Rename { .. });
        // In dialogs 'q' never quits (the delete dialog treats it as cancel).
        let in_dialog = matches!(
            mode,
            Mode::ConfirmDelete { .. } | Mode::PasteConflict { .. }
        );
        if code == KeyCode::Char('q') && !text_input && !in_dialog {
            return Some(Command::Quit);
        }

        if let KeyCode::Char(c) = code {
            if matches!(mode, Mode::Filer | Mode::Viewer { .. }) && c.is_ascii_digit() {
                self.prefix = self
                    .prefix
                    .saturating_mul(10)
                    .saturating_add(c.to_digit(10).unwrap() as usize);
                return None;
            }
        }
        let count = if self.prefix > 0 { self.prefix } else { 1 };
        self.prefix = 0;

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

/// Main event loop: draw, read a key, translate it and apply the command.
pub fn run_app<B: Backend<Error = io::Error> + Write>(
    terminal: &mut Terminal<B>,
    app: &mut App,
) -> io::Result<()> {
    let mut keys = KeyState::default();
    loop {
        terminal.draw(|f| crate::ui::ui(f, app))?;
        if event::poll(Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                // A status message stays until the next key press.
                app.message = None;
                if let Some(cmd) = keys.translate(&app.mode, key.code) {
                    if app.apply(cmd).is_break() {
                        return Ok(());
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::delete_confirm::DeleteKind;
    use crate::paste::{ConflictChoice, Resolution};
    use std::path::PathBuf;

    fn ch(c: char) -> KeyCode {
        KeyCode::Char(c)
    }

    /// Translate a key sequence in a fixed mode, collecting the commands.
    fn run(mode: &Mode, keys: &[KeyCode]) -> Vec<Command> {
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
            content: String::new(),
            title: String::new(),
            offset: 0,
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
        assert_eq!(run(&f, &[ch('z')]), []);
        assert_eq!(run(&f, &[KeyCode::Down]), []);
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
        let visual = Mode::Visual { anchor: 0 };
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
            Mode::Visual { anchor: 0 },
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
        let v = Mode::Visual { anchor: 0 };
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
        let s = Mode::Sort { selected: 0 };
        assert_eq!(run(&s, &[KeyCode::Down]), [Command::MoveDown(1)]);
        assert_eq!(run(&s, &[ch('k')]), [Command::MoveUp(1)]);
        assert_eq!(run(&s, &[KeyCode::Enter]), [Command::SortConfirm]);
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
}
