use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph},
    Frame,
};

use crate::{
    app::{App, MessageLevel, Pane},
    delete_confirm::{dialog_lines, dialog_title, DeleteKind},
    git::FileStatus,
    mode::Mode,
    mode::PaneType,
    text_view::{self, PreviewContent},
    watch::{self, Recency},
};
use std::time::SystemTime;
use unicode_width::UnicodeWidthStr;

pub fn ui(f: &mut Frame, app: &mut App) {
    app.sync_text_preview();
    let size = f.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .split(size);

    let header = Paragraph::new(Line::from(vec![
        Span::styled(
            "蔵",
            Style::default()
                .fg(Color::Magenta)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        Span::styled("kura", Style::default().add_modifier(Modifier::BOLD)),
    ]))
    .alignment(Alignment::Center);
    f.render_widget(header, chunks[0]);

    let (content_area, footer_area) = if matches!(
        app.mode,
        Mode::Search { .. } | Mode::Rename { .. } | Mode::Preview
    ) {
        let v = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(0), Constraint::Length(1)])
            .split(chunks[1]);
        (v[0], Some(v[1]))
    } else {
        (chunks[1], None)
    };

    if let Mode::Viewer {
        doc,
        offset,
        at_end,
        ..
    } = &mut app.mode
    {
        let block = Block::default()
            .borders(Borders::ALL)
            .title(doc.title.as_str());
        // The offset is limited to the current size here, so that it also
        // follows a terminal resize; at the end it stays at the end.
        let wanted = if *at_end { usize::MAX } else { *offset };
        let (size, clamped) = text_view::draw(f, content_area, block, doc, wanted);
        *offset = clamped;
        app.viewer_size = Some(size);
    } else {
        let panes = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(content_area);
        let left_active = app.active == PaneType::Left;
        match &app.mode {
            // Preview replaces the pane opposite to the active one.
            Mode::Preview => {
                let (pane_area, preview_area) = if left_active {
                    (panes[0], panes[1])
                } else {
                    (panes[1], panes[0])
                };
                let pane = if left_active { &app.left } else { &app.right };
                draw_pane(f, pane_area, pane, true, app.session_start);
                app.preview.draw(f, preview_area);
            }
            // Text preview (`P`) of the entry under the cursor.
            _ if app.text_preview.is_some() => {
                let (pane_area, preview_area) = if left_active {
                    (panes[0], panes[1])
                } else {
                    (panes[1], panes[0])
                };
                let pane = if left_active { &app.left } else { &app.right };
                draw_pane(f, pane_area, pane, true, app.session_start);
                if app.pane_preview_shows_media() {
                    draw_media_preview(f, preview_area, app);
                } else if let Some(preview) = &app.text_preview {
                    draw_text_preview(f, preview_area, &preview.content);
                }
            }
            _ => {
                draw_pane(f, panes[0], &app.left, left_active, app.session_start);
                draw_pane(f, panes[1], &app.right, !left_active, app.session_start);
            }
        }
    }

    if let Mode::ConfirmDelete { items, kind } = &app.mode {
        draw_delete_dialog(f, items, *kind);
    }

    if let Mode::PasteConflict { pending } = &app.mode {
        draw_paste_conflict(f, &pending.planner);
    }

    if let Mode::Sort { selected } = &app.mode {
        // Tall enough for every row (borders included), within the screen.
        let area = f.area();
        let mut popup = centered_rect(40, 20, area);
        popup.height = (crate::fs_utils::SORT_MENU_ROWS as u16 + 2).min(area.height);
        popup.y = area.y + (area.height - popup.height) / 2;
        let block = Block::default().title("Sort By").borders(Borders::ALL);
        let dirs_first = format!(
            "Directories first: {}",
            if app.current_pane().dirs_first {
                "on"
            } else {
                "off"
            }
        );
        let items: Vec<ListItem> = crate::fs_utils::SORT_OPTIONS
            .iter()
            .map(|option| option.to_string())
            .chain(std::iter::once(dirs_first))
            .enumerate()
            .map(|(i, option)| {
                let style = if i == *selected {
                    Style::default().add_modifier(Modifier::REVERSED)
                } else {
                    Style::default()
                };
                ListItem::new(Line::from(Span::styled(option, style)))
            })
            .collect();
        let list = List::new(items).block(block);
        f.render_widget(Clear, popup);
        f.render_widget(list, popup);
    }

    draw_status_line(f, chunks[2], app);

    if let Some(footer) = footer_area {
        let prompt = match &app.mode {
            Mode::Search { query } => format!("/{query}"),
            Mode::Rename { original, buffer } => format!("rename: {original} -> {buffer}"),
            Mode::Preview => format!(
                "j/k: next/prev   Enter/Esc: close preview   o: open in default app   [{}]",
                app.graphics_info
            ),
            _ => String::new(),
        };
        let paragraph = Paragraph::new(prompt);
        f.render_widget(paragraph, footer);
    }
}

/// Collision dialog shown while pasting: names the colliding entry and
/// lists the answers.
fn draw_paste_conflict(f: &mut Frame, planner: &crate::paste::Planner) {
    let Some(item) = planner.current() else {
        return;
    };
    let (pos, total) = planner.progress();
    let name = item.file_name.to_string_lossy();
    let kind = if item.is_dir { "folder" } else { "file" };
    let lines = vec![
        Line::from(format!("A {kind} named")),
        Line::from(Span::styled(
            name.into_owned(),
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from("already exists here."),
        Line::from(""),
        Line::from("o: Overwrite   s: Skip   b: Keep both"),
        Line::from("O / S / B: apply to all remaining"),
        Line::from("Esc: cancel paste"),
    ];
    let popup = centered_rect(50, 40, f.area());
    let block = Block::default()
        .title(format!(" Already exists ({pos}/{total}) "))
        .borders(Borders::ALL);
    let paragraph = Paragraph::new(lines)
        .block(block)
        .alignment(Alignment::Center)
        .wrap(ratatui::widgets::Wrap { trim: false });
    f.render_widget(Clear, popup);
    f.render_widget(paragraph, popup);
}

/// Bottom status line: the latest message on the left, cursor position and
/// selection count on the right.
fn draw_status_line(f: &mut Frame, area: Rect, app: &App) {
    let pane = app.current_pane();
    let mut info = if pane.items.is_empty() {
        "0/0".to_string()
    } else {
        format!("{}/{}", pane.selected + 1, pane.items.len())
    };
    let marked = pane.marked_count();
    if marked > 0 {
        info = format!("{marked} selected  {info}");
    }
    if let Some(git) = &pane.git {
        let branch = git.branch.as_deref().unwrap_or("(detached)");
        info = format!("git:{branch}  {info}");
    }
    let info_width = (UnicodeWidthStr::width(info.as_str()) as u16).min(area.width);
    let parts = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(0), Constraint::Length(info_width + 1)])
        .split(area);

    if let Some(message) = &app.message {
        let style = match message.level {
            MessageLevel::Error => Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            MessageLevel::Info => Style::default().fg(Color::Green),
        };
        f.render_widget(
            Paragraph::new(Span::styled(message.text.as_str(), style)),
            parts[0],
        );
    }
    f.render_widget(
        Paragraph::new(Span::styled(info, Style::default().fg(Color::DarkGray)))
            .alignment(Alignment::Right),
        parts[1],
    );
}

/// Color of a git status symbol.
fn git_status_style(status: FileStatus) -> Style {
    let color = match status {
        FileStatus::Modified | FileStatus::DirtyDir => Color::Yellow,
        FileStatus::Added => Color::Green,
        FileStatus::Deleted | FileStatus::Conflicted => Color::Red,
        FileStatus::Renamed => Color::Cyan,
        FileStatus::Untracked => Color::Magenta,
        FileStatus::Ignored => Color::DarkGray,
    };
    Style::default().fg(color)
}

/// Extra style for a file changed recently (keeps the type colour).
fn recency_style(recency: Option<Recency>) -> Style {
    match recency {
        Some(Recency::Recent) => Style::default()
            .bg(Color::DarkGray)
            .add_modifier(Modifier::BOLD),
        Some(Recency::SinceStart) => Style::default().add_modifier(Modifier::UNDERLINED),
        None => Style::default(),
    }
}

fn draw_pane(f: &mut Frame, area: Rect, pane: &Pane, active: bool, session_start: SystemTime) {
    let now = SystemTime::now();
    let mut title = format!(" {} ", pane.current_dir.display());
    if pane.git_filter_active() {
        title.push_str("[changed only] ");
    }
    if !pane.show_hidden {
        title.push_str("[no hidden] ");
    }
    if pane.ignore_filter_active() {
        title.push_str("[no ignored] ");
    }
    let block = Block::default().borders(Borders::ALL).title(Span::styled(
        title,
        Style::default()
            .fg(if active { Color::Yellow } else { Color::White })
            .add_modifier(Modifier::BOLD),
    ));
    let items: Vec<ListItem> = pane
        .items
        .iter()
        .map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let path = e.path();
            let style = if path.is_dir() {
                Style::default().fg(Color::Blue)
            } else if name.starts_with('.') {
                Style::default().fg(Color::Red)
            } else {
                let is_executable = {
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        e.metadata()
                            .map(|m| m.permissions().mode() & 0o111 != 0)
                            .unwrap_or(false)
                    }
                    #[cfg(windows)]
                    {
                        path.extension().map_or(false, |ext| ext == "exe")
                    }
                };
                if is_executable {
                    Style::default().fg(Color::Green)
                } else {
                    Style::default()
                }
            };
            let marker = if pane.marked.contains(&path) {
                "*"
            } else {
                " "
            };
            let mut spans = vec![Span::raw(format!("{marker} "))];
            // The status column only appears inside a repository.
            if pane.git.is_some() {
                spans.push(match pane.git_status_of(e) {
                    Some(s) => Span::styled(format!("{} ", s.symbol()), git_status_style(s)),
                    None => Span::raw("  "),
                });
            }
            let recency = e
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|m| watch::classify(m, now, session_start));
            spans.push(Span::styled(name, style.patch(recency_style(recency))));
            ListItem::new(Line::from(spans))
        })
        .collect();
    let mut state = ListState::default();
    state.select(Some(pane.selected));
    let list = List::new(items)
        .block(block)
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED))
        .highlight_symbol(">> ");
    f.render_stateful_widget(list, area, &mut state);
}

/// Pane preview (`P`) of an image / video, loaded in the background by
/// `app.preview` ("Loading…" until it is ready).
fn draw_media_preview(f: &mut Frame, area: Rect, app: &mut App) {
    let title = app.preview.title().unwrap_or_default();
    let block = Block::default()
        .borders(Borders::ALL)
        .title(Span::styled(
            format!(" {title} "),
            Style::default()
                .fg(Color::Magenta)
                .add_modifier(Modifier::BOLD),
        ))
        .title_bottom(Line::from(" P: close preview ").right_aligned());
    let inner = block.inner(area);
    f.render_widget(block, area);
    app.preview.draw_body(f, inner);
}

/// Text preview pane (`P`): the file's first lines, or why there are none.
fn draw_text_preview(f: &mut Frame, area: Rect, content: &PreviewContent) {
    let title = match content {
        PreviewContent::Text(doc) => doc.title.as_str(),
        PreviewContent::Message(_) => "Preview",
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .title(Span::styled(
            format!(" {title} "),
            Style::default()
                .fg(Color::Magenta)
                .add_modifier(Modifier::BOLD),
        ))
        .title_bottom(Line::from(" P: close preview ").right_aligned());
    match content {
        PreviewContent::Text(doc) => {
            text_view::draw(f, area, block, doc, 0);
        }
        PreviewContent::Message(msg) => f.render_widget(
            Paragraph::new(Span::styled(msg.as_str(), Style::default().fg(Color::Gray)))
                .block(block),
            area,
        ),
    }
}

/// Helper to create a centered rect using the given percentage width and height of the available rect
/// Delete confirmation popup sized to its contents.
fn draw_delete_dialog(f: &mut Frame, items: &[std::path::PathBuf], kind: DeleteKind) {
    let area = f.area();
    // Rows available inside the borders.
    let max_lines = area.height.saturating_sub(2) as usize;
    let lines = dialog_lines(items, kind, max_lines);
    let content_w = lines.iter().map(|l| l.width()).max().unwrap_or(0) as u16;
    let width = (content_w + 4).max(40).min(area.width);
    let height = (lines.len() as u16 + 2).min(area.height);
    let popup = Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    };
    let border_style = match kind {
        DeleteKind::Normal => Style::default(),
        DeleteKind::Permanent => Style::default().fg(Color::Red),
    };
    let block = Block::default()
        .title(dialog_title(kind))
        .borders(Borders::ALL)
        .border_style(border_style);
    let text: Vec<Line> = lines
        .into_iter()
        .map(|l| Line::from(format!(" {l}")))
        .collect();
    f.render_widget(Clear, popup);
    f.render_widget(Paragraph::new(text).block(block), popup);
}

fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let vertical_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);
    let middle = vertical_chunks[1];
    let horizontal_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(middle);
    horizontal_chunks[1]
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{backend::TestBackend, Terminal};
    use std::path::PathBuf;

    fn render(items: &[PathBuf], kind: DeleteKind, w: u16, h: u16) -> String {
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| draw_delete_dialog(f, items, kind)).unwrap();
        let buf = term.backend().buffer();
        (0..h)
            .map(|y| (0..w).map(|x| buf[(x, y)].symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn delete_dialog_shows_names_and_fits_small_terminal() {
        let items: Vec<PathBuf> = (0..25).map(|i| PathBuf::from(format!("/d/f{i}"))).collect();
        let screen = render(&items, DeleteKind::Permanent, 60, 10);
        assert!(screen.contains("Permanently delete 25 item(s)?"));
        assert!(screen.contains("f0"));
        assert!(screen.contains("more"));
        assert!(screen.contains("(y/N)"));

        let screen = render(&items[..2], DeleteKind::Normal, 80, 30);
        assert!(screen.contains("Move 2 item(s) to Trash?"));
        assert!(screen.contains("f1"));
        assert!(screen.contains("(y/N)"));
        assert!(!screen.contains("cannot be undone"));
    }

    #[test]
    fn pane_title_shows_active_filters() {
        let tmp = tempfile::tempdir().unwrap();
        let mut pane = Pane::new(tmp.path().to_path_buf()).unwrap();
        let title = |pane: &Pane| {
            let mut term = Terminal::new(TestBackend::new(200, 3)).unwrap();
            term.draw(|f| draw_pane(f, f.area(), pane, true, SystemTime::now()))
                .unwrap();
            let buf = term.backend().buffer();
            (0..200).map(|x| buf[(x, 0)].symbol()).collect::<String>()
        };
        assert!(!title(&pane).contains("[no hidden]"));
        pane.toggle_hidden().unwrap();
        assert!(title(&pane).contains("[no hidden]"));
        // The ignore filter is shown only when it is in effect (in a repo).
        pane.hide_ignored = true;
        assert_eq!(
            title(&pane).contains("[no ignored]"),
            pane.ignore_filter_active()
        );
    }

    /// Every row of the screen.
    fn full_screen(app: &mut App, w: u16, h: u16) -> Vec<String> {
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| ui(f, app)).unwrap();
        let buf = term.backend().buffer();
        (0..h)
            .map(|y| (0..w).map(|x| buf[(x, y)].symbol()).collect::<String>())
            .collect()
    }

    #[test]
    fn viewer_go_bottom_puts_the_last_wrapped_row_at_the_bottom() {
        let tmp = tempfile::tempdir().unwrap();
        let long = "あいうえおかきくけこ".repeat(6);
        std::fs::write(tmp.path().join("t.txt"), format!("{long}\nLAST\n")).unwrap();
        let mut app = App::with_dir(
            ratatui_image::picker::Picker::halfblocks(),
            tmp.path().to_path_buf(),
        )
        .unwrap();
        let _ = app.apply(crate::command::Command::Open);
        let _ = app.apply(crate::command::Command::GoBottom);
        // Header, viewer border, 4 text rows, border, status line.
        let rows = full_screen(&mut app, 24, 8);
        assert!(rows[5].contains("LAST"), "{rows:#?}");
        assert!(!rows[4].contains("LAST"));
        let Mode::Viewer { offset, doc, .. } = &app.mode else {
            panic!("not in the viewer");
        };
        // The draw kept the offset it showed, for `k` to start from.
        assert_eq!(app.viewer_size, Some((22, 4)));
        assert_eq!(*offset, text_view::max_offset(doc, 22, 4));
        // A wider terminal has fewer rows: the offset follows it.
        let rows = full_screen(&mut app, 60, 8);
        assert!(rows[5].contains("LAST"), "{rows:#?}");
        let Mode::Viewer { offset, doc, .. } = &app.mode else {
            panic!("not in the viewer");
        };
        assert_eq!(*offset, text_view::max_offset(doc, 58, 4));
    }

    /// The viewer on a 24-column screen with the last row at the bottom,
    /// for the resize tests below.
    fn viewer_on_long_text(rows: usize) -> App {
        let tmp = tempfile::tempdir().unwrap();
        let long = "あいうえおかきくけこ".repeat(6);
        let text = format!("{}LAST\n", format!("{long}\n").repeat(rows));
        std::fs::write(tmp.path().join("t.txt"), text).unwrap();
        let mut app = App::with_dir(
            ratatui_image::picker::Picker::halfblocks(),
            tmp.path().to_path_buf(),
        )
        .unwrap();
        let _ = app.apply(crate::command::Command::Open);
        app
    }

    #[test]
    fn viewer_stays_at_the_end_when_the_terminal_gets_narrower() {
        use crate::command::Command;
        // `G` on a wide screen, then narrower: more rows, still at the end.
        let mut app = viewer_on_long_text(3);
        full_screen(&mut app, 60, 8);
        let _ = app.apply(Command::GoBottom);
        assert!(full_screen(&mut app, 60, 8)[5].contains("LAST"));
        let rows = full_screen(&mut app, 24, 8);
        assert!(rows[5].contains("LAST"), "{rows:#?}");
        // And wider again.
        assert!(full_screen(&mut app, 60, 8)[5].contains("LAST"));

        // Reaching the end with `j` (or a page down) does the same.
        let mut app = viewer_on_long_text(3);
        full_screen(&mut app, 60, 8);
        let Mode::Viewer { doc, .. } = &app.mode else {
            panic!("not in the viewer");
        };
        let max = text_view::max_offset(doc, 58, 4);
        for _ in 0..max - 1 {
            let _ = app.apply(Command::MoveDown(1));
        }
        assert!(!full_screen(&mut app, 60, 8)[5].contains("LAST"));
        let _ = app.apply(Command::MoveDown(1));
        assert!(full_screen(&mut app, 60, 8)[5].contains("LAST"));
        let rows = full_screen(&mut app, 24, 8);
        assert!(rows[5].contains("LAST"), "{rows:#?}");
        let _ = app.apply(Command::PageUp(1));
        full_screen(&mut app, 60, 8);
        let _ = app.apply(Command::PageDown(1));
        assert!(full_screen(&mut app, 60, 8)[5].contains("LAST"));
        assert!(full_screen(&mut app, 24, 8)[5].contains("LAST"));

        // After `k` it no longer follows the end.
        let mut app = viewer_on_long_text(3);
        full_screen(&mut app, 60, 8);
        let _ = app.apply(Command::GoBottom);
        full_screen(&mut app, 60, 8);
        let _ = app.apply(Command::MoveUp(1));
        let shows_last = |rows: Vec<String>| rows.iter().any(|r| r.contains("LAST"));
        assert!(!shows_last(full_screen(&mut app, 60, 8)));
        let Mode::Viewer { offset, .. } = &app.mode else {
            panic!("not in the viewer");
        };
        let before = *offset;
        assert!(!shows_last(full_screen(&mut app, 24, 8)));
        let Mode::Viewer { offset, .. } = &app.mode else {
            panic!("not in the viewer");
        };
        assert_eq!(*offset, before);
    }

    /// The right half of the screen (the preview, with the left pane active).
    fn screen(app: &mut App, w: u16, h: u16) -> String {
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| ui(f, app)).unwrap();
        let buf = term.backend().buffer();
        (0..h)
            .map(|y| (w / 2..w).map(|x| buf[(x, y)].symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn pane_preview_draws_images_then_text_without_leftovers() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("a.png"), "").unwrap();
        std::fs::write(tmp.path().join("b.txt"), "hello text\n").unwrap();
        let mut app = App::with_dir(
            ratatui_image::picker::Picker::halfblocks(),
            tmp.path().to_path_buf(),
        )
        .unwrap();
        app.toggle_text_preview();
        let s = screen(&mut app, 80, 12);
        assert!(s.contains(" a.png "));
        assert!(s.contains("Loading…") || s.contains("Cannot decode"));
        assert!(s.contains("P: close preview"));
        app.current_pane_mut().selected = 1;
        let s = screen(&mut app, 80, 12);
        assert!(s.contains(" b.txt "));
        assert!(s.contains("hello text"));
        assert!(!s.contains("Loading…"));
        assert!(!s.contains(" a.png "));
    }
}
