use crossterm::event::{self, Event, KeyCode};
use ratatui::{backend::Backend, Terminal};
use std::time::Duration;
use std::{fs, io, io::Write, path::PathBuf};

use crate::actions::{copy_selection, delete_items, paste, toggle_mark};
use crate::app::{fs_error_message, App};
use crate::delete_confirm::{confirm_key, ConfirmAction, DeleteKind};
use crate::fs_utils::{apply_sort, find_match, SortBy, SORT_OPTIONS};
use crate::media::open_external;
use crate::mode::{Mode, PaneType};

/// Paths to act on: the marked entries (in list order), or the entry under
/// the cursor when nothing is marked.
fn target_paths(app: &App) -> Vec<PathBuf> {
    let pane = app.current_pane();
    if pane.marked.is_empty() {
        return pane
            .items
            .get(pane.selected)
            .map(|e| e.path())
            .into_iter()
            .collect();
    }
    let mut idx: Vec<usize> = pane.marked.iter().copied().collect();
    idx.sort_unstable();
    idx.iter()
        .filter_map(|&i| pane.items.get(i).map(|e| e.path()))
        .collect()
}

/// Main event loop: handles input and dispatches actions.
pub fn run_app<B: Backend<Error = io::Error> + Write>(
    terminal: &mut Terminal<B>,
    app: &mut App,
) -> io::Result<()> {
    let mut prefix: usize = 0;
    let mut last_key_g = false;
    loop {
        terminal.draw(|f| crate::ui::ui(f, app))?;
        if event::poll(Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                // A status message stays until the next key press.
                app.message = None;
                // Text-input modes must receive every character (including 'q', 'g', 'G').
                let text_input = matches!(app.mode, Mode::Search { .. } | Mode::Rename { .. });
                // In the delete dialog 'q' cancels instead of quitting.
                let in_dialog = matches!(app.mode, Mode::ConfirmDelete { .. });
                if key.code == KeyCode::Char('q') && !text_input && !in_dialog {
                    return Ok(());
                }
                let mut rename_target: Option<String> = None;
                let mut sort_choice: Option<SortBy> = None;

                if let KeyCode::Char(c) = key.code {
                    if (matches!(app.mode, Mode::Filer) || matches!(app.mode, Mode::Viewer { .. }))
                        && c.is_ascii_digit()
                    {
                        prefix = prefix
                            .saturating_mul(10)
                            .saturating_add(c.to_digit(10).unwrap() as usize);
                        continue;
                    }
                }
                let count = if prefix > 0 { prefix } else { 1 };
                prefix = 0;

                // Vim-style 'gg' (go to top) and 'G' (go to bottom)
                if let (KeyCode::Char(c), false) = (key.code, text_input) {
                    match c {
                        'g' => {
                            if last_key_g {
                                last_key_g = false;
                                match &mut app.mode {
                                    Mode::Filer => app.current_pane_mut().selected = 0,
                                    Mode::Viewer { offset, .. } => *offset = 0,
                                    _ => {}
                                }
                            } else {
                                last_key_g = true;
                            }
                            continue;
                        }
                        'G' => {
                            last_key_g = false;
                            match &mut app.mode {
                                Mode::Filer => {
                                    let pane = app.current_pane_mut();
                                    pane.selected = pane.items.len().saturating_sub(1);
                                }
                                Mode::Viewer {
                                    offset, content, ..
                                } => {
                                    let total = content.lines().count();
                                    *offset = total.saturating_sub(1) as u16;
                                }
                                _ => {}
                            }
                            continue;
                        }
                        _ => last_key_g = false,
                    }
                }

                // Visual selection mode
                if let Mode::Visual { anchor } = app.mode {
                    match key.code {
                        KeyCode::Char('j') => {
                            for _ in 0..count {
                                app.on_down();
                            }
                            let pane = app.current_pane_mut();
                            pane.marked.clear();
                            let end = pane.selected;
                            let (lo, hi) = if anchor <= end {
                                (anchor, end)
                            } else {
                                (end, anchor)
                            };
                            for i in lo..=hi {
                                pane.marked.insert(i);
                            }
                        }
                        KeyCode::Char('k') => {
                            for _ in 0..count {
                                app.on_up();
                            }
                            let pane = app.current_pane_mut();
                            pane.marked.clear();
                            let end = pane.selected;
                            let (lo, hi) = if anchor <= end {
                                (anchor, end)
                            } else {
                                (end, anchor)
                            };
                            for i in lo..=hi {
                                pane.marked.insert(i);
                            }
                        }
                        KeyCode::Char('V') | KeyCode::Esc => {
                            app.mode = Mode::Filer;
                        }
                        _ => {}
                    }
                    continue;
                }

                // Search mode: edit query and jump to matching entries
                if let Mode::Search { query } = &mut app.mode {
                    match key.code {
                        KeyCode::Char(c) => query.push(c),
                        KeyCode::Backspace => {
                            query.pop();
                        }
                        KeyCode::Enter | KeyCode::Esc => {
                            app.mode = Mode::Filer;
                        }
                        _ => {}
                    }
                }
                let q_opt = if let Mode::Search { query } = &app.mode {
                    Some(query.clone())
                } else {
                    None
                };
                if let Some(q) = q_opt {
                    let pane = app.current_pane_mut();
                    if let Some(idx) = find_match(&pane.items, &q, pane.selected) {
                        pane.selected = idx;
                    }
                    continue;
                }

                // Rename mode
                if let Mode::Rename { buffer, .. } = &mut app.mode {
                    match key.code {
                        KeyCode::Char(c) => buffer.push(c),
                        KeyCode::Backspace => {
                            buffer.pop();
                        }
                        KeyCode::Enter => {
                            rename_target = Some(buffer.clone());
                            app.mode = Mode::Filer;
                        }
                        KeyCode::Esc => {
                            app.mode = Mode::Filer;
                        }
                        _ => {}
                    }
                }

                // Sort mode
                if let Mode::Sort { selected } = &mut app.mode {
                    match key.code {
                        KeyCode::Down | KeyCode::Char('j') => {
                            *selected = (*selected + 1) % SORT_OPTIONS.len();
                        }
                        KeyCode::Up | KeyCode::Char('k') => {
                            *selected = (*selected + SORT_OPTIONS.len() - 1) % SORT_OPTIONS.len();
                        }
                        KeyCode::Enter => {
                            let by = match *selected {
                                0 => SortBy::Modified,
                                1 => SortBy::Created,
                                2 => SortBy::Size,
                                _ => SortBy::Name,
                            };
                            sort_choice = Some(by);
                            app.mode = Mode::Filer;
                        }
                        KeyCode::Esc => {
                            app.mode = Mode::Filer;
                        }
                        _ => {}
                    }
                }

                // Commit rename
                if let Some(new_name) = rename_target {
                    let pane = app.current_pane_mut();
                    let old = pane.items[pane.selected].path();
                    let newp = old.with_file_name(&new_name);
                    let result = fs::rename(&old, &newp);
                    if pane.refresh().is_ok() {
                        if let Some(pos) = pane
                            .items
                            .iter()
                            .position(|e| e.file_name().to_string_lossy() == new_name)
                        {
                            pane.selected = pos;
                        }
                    }
                    match result {
                        Ok(()) => app.notify_info(format!("Renamed to {new_name}")),
                        Err(e) => app.notify_error(fs_error_message(&e, &old)),
                    }
                    continue;
                }

                // Commit sort
                if let Some(by) = sort_choice {
                    apply_sort(app.current_pane_mut(), by);
                    continue;
                }

                match &mut app.mode {
                    Mode::ConfirmDelete { items, .. } => match confirm_key(key.code) {
                        ConfirmAction::Delete => {
                            let to_delete = std::mem::take(items);
                            app.mode = Mode::Filer;
                            delete_items(app, &to_delete);
                        }
                        ConfirmAction::Cancel => {
                            app.mode = Mode::Filer;
                            app.notify_info("Delete cancelled");
                        }
                        ConfirmAction::Ignore => {}
                    },
                    Mode::Viewer { offset, .. } => match key.code {
                        KeyCode::Char('j') => *offset = offset.saturating_add(count as u16),
                        KeyCode::Char('k') => *offset = offset.saturating_sub(count as u16),
                        KeyCode::Enter => app.mode = Mode::Filer,
                        _ => {}
                    },
                    Mode::Preview => match key.code {
                        KeyCode::Enter | KeyCode::Esc => app.close_preview(),
                        KeyCode::Char('o') => {
                            if let Some(p) = app.selected_path() {
                                if let Err(e) = open_external(&p) {
                                    app.notify_error(fs_error_message(&e, &p));
                                }
                            }
                        }
                        _ => {}
                    },
                    Mode::Filer => match key.code {
                        KeyCode::Char('j') => (0..count).for_each(|_| app.on_down()),
                        KeyCode::Char('k') => (0..count).for_each(|_| app.on_up()),
                        KeyCode::Char(c @ ('x' | 'X')) => {
                            let items = target_paths(app);
                            if !items.is_empty() {
                                let kind = if c == 'X' {
                                    DeleteKind::Permanent
                                } else {
                                    DeleteKind::Normal
                                };
                                app.mode = Mode::ConfirmDelete { items, kind };
                            }
                        }
                        KeyCode::Enter => {
                            let previewed =
                                app.selected_path().is_some_and(|p| app.open_preview(&p));
                            if !previewed {
                                app.on_enter();
                            }
                        }
                        KeyCode::Char('o') => {
                            if let Some(p) = app.selected_path() {
                                if let Err(e) = open_external(&p) {
                                    app.notify_error(fs_error_message(&e, &p));
                                }
                            }
                        }
                        KeyCode::Char('h') => match app.active {
                            PaneType::Left => app.on_left(),
                            PaneType::Right => app.switch_pane(),
                        },
                        KeyCode::Char('l') => match app.active {
                            PaneType::Left => app.switch_pane(),
                            PaneType::Right => app.on_left(),
                        },
                        KeyCode::Char('V') => {
                            let pane = app.current_pane_mut();
                            let anchor = pane.selected;
                            pane.marked.clear();
                            pane.marked.insert(anchor);
                            app.mode = Mode::Visual { anchor };
                        }
                        KeyCode::Char('/') => {
                            app.mode = Mode::Search {
                                query: String::new(),
                            };
                        }
                        KeyCode::Char('r') => {
                            let pane = app.current_pane_mut();
                            if let Some(entry) = pane.items.get(pane.selected) {
                                let name = entry.file_name().to_string_lossy().into_owned();
                                app.mode = Mode::Rename {
                                    original: name.clone(),
                                    buffer: name,
                                };
                            }
                        }
                        KeyCode::Char('s') => {
                            app.mode = Mode::Sort { selected: 0 };
                        }
                        KeyCode::Char('v') => {
                            toggle_mark(app.current_pane_mut());
                        }
                        KeyCode::Char('y') => {
                            copy_selection(app);
                        }
                        KeyCode::Char('p') => {
                            let n = app.clipboard.len();
                            let dst_dir = app.current_pane().current_dir.clone();
                            match paste(app) {
                                Ok(()) => app.notify_info(format!("Pasted {n} item(s)")),
                                Err(e) => app.notify_error(fs_error_message(&e, &dst_dir)),
                            }
                        }
                        _ => {}
                    },
                    _ => {}
                }
            }
        }
    }
}
