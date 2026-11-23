use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph},
    Frame,
};

use ratatui_image::{Resize, StatefulImage};

use crate::{
    app::{App, MessageLevel, Pane},
    delete_confirm::{dialog_lines, dialog_title, DeleteKind},
    media::Preview,
    mode::Mode,
    mode::PaneType,
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

pub fn ui(f: &mut Frame, app: &mut App) {
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
        content,
        title,
        offset,
    } = &app.mode
    {
        let block = Block::default().borders(Borders::ALL).title(title.as_str());
        // available rows and margin width
        let inner_height = content_area.height.saturating_sub(2) as usize;
        let number_width = inner_height.to_string().len().max(1);
        // wrap each content line into display rows of at most (width - margin) cols
        let text_width = content_area.width.saturating_sub((number_width + 1) as u16) as usize;
        let mut rows: Vec<String> = Vec::new();
        for line in content.lines() {
            // if the line fits, push as-is
            if UnicodeWidthStr::width(line) <= text_width {
                rows.push(line.to_string());
            } else {
                let mut s = line;
                // break into segments that fit
                while UnicodeWidthStr::width(s) > text_width {
                    let mut w = 0;
                    let mut end = 0;
                    for (i, ch) in s.char_indices() {
                        let cw = ch.width().unwrap_or(0);
                        if w + cw > text_width {
                            break;
                        }
                        w += cw;
                        end = i + ch.len_utf8();
                    }
                    if end == 0 {
                        let first = s.chars().next().unwrap();
                        let len = first.len_utf8();
                        rows.push(s[..len].to_string());
                        s = &s[len..];
                    } else {
                        rows.push(s[..end].to_string());
                        s = &s[end..];
                    }
                }
                if !s.is_empty() {
                    rows.push(s.to_string());
                }
            }
        }
        let total_rows = rows.len();
        let max_off = total_rows.saturating_sub(inner_height);
        let start = (*offset as usize).min(max_off);
        let numbered: Vec<Line> = rows
            .iter()
            .skip(start)
            .take(inner_height)
            .enumerate()
            .map(|(i, row)| {
                let num = format!("{:>width$} ", i, width = number_width);
                Line::from(vec![
                    Span::styled(num, Style::default().fg(Color::DarkGray)),
                    Span::raw(row),
                ])
            })
            .collect();
        let paragraph = Paragraph::new(numbered).block(block);
        f.render_widget(paragraph, content_area);
    } else {
        let panes = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(content_area);
        let left_active = app.active == PaneType::Left;
        match (&mut app.preview, &app.mode) {
            // Preview replaces the pane opposite to the active one.
            (Some(preview), Mode::Preview) => {
                let (pane_area, preview_area) = if left_active {
                    (panes[0], panes[1])
                } else {
                    (panes[1], panes[0])
                };
                let pane = if left_active { &app.left } else { &app.right };
                draw_pane(f, pane_area, pane, true);
                draw_preview(f, preview_area, preview);
            }
            _ => {
                draw_pane(f, panes[0], &app.left, left_active);
                draw_pane(f, panes[1], &app.right, !left_active);
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
                "Enter/Esc: close preview   o: open in default app   [{}]",
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

fn draw_pane(f: &mut Frame, area: Rect, pane: &Pane, active: bool) {
    let title = format!(" {} ", pane.current_dir.display());
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
            ListItem::new(Line::from(vec![
                Span::raw(format!("{marker} ")),
                Span::styled(name, style),
            ]))
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

fn draw_preview(f: &mut Frame, area: Rect, preview: &mut Preview) {
    let block = Block::default().borders(Borders::ALL).title(Span::styled(
        format!(" {} ", preview.title),
        Style::default()
            .fg(Color::Magenta)
            .add_modifier(Modifier::BOLD),
    ));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let info_height = (preview.info.len() as u16).min(inner.height);
    let parts = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(info_height)])
        .split(inner);

    if let Some(image) = preview.image.as_mut() {
        // Center the fitted image inside the available area.
        let img_area = parts[0];
        let fitted = image.size_for(Resize::Fit(None), img_area.as_size());
        let x = img_area.x + img_area.width.saturating_sub(fitted.width) / 2;
        let y = img_area.y + img_area.height.saturating_sub(fitted.height) / 2;
        let centered = Rect::new(x, y, fitted.width, fitted.height);
        f.render_stateful_widget(StatefulImage::default(), centered, image);
    }

    let lines: Vec<Line> = preview
        .info
        .iter()
        .map(|l| Line::from(Span::styled(l.as_str(), Style::default().fg(Color::Gray))))
        .collect();
    f.render_widget(Paragraph::new(lines), parts[1]);
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
        assert!(screen.contains("Delete 2 item(s)?"));
        assert!(screen.contains("f1"));
    }
}
