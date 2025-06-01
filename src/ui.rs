use ratatui::{
    backend::Backend,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Span, Spans},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph},
    Frame,
};

use crate::{app::App, app::Pane, mode::Mode, mode::PaneType};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

pub fn ui<B: Backend>(f: &mut Frame<B>, app: &App) {
    let size = f.size();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(0)])
        .split(size);

    let header = Paragraph::new(Spans::from(vec![
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

    let (content_area, footer_area) =
        if matches!(app.mode, Mode::Search { .. } | Mode::Rename { .. }) {
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
        let start = (*offset as usize).min(max_off) as usize;
        let numbered: Vec<Spans> = rows
            .iter()
            .skip(start)
            .take(inner_height)
            .enumerate()
            .map(|(i, row)| {
                let num = format!("{:>width$} ", i, width = number_width);
                Spans::from(vec![
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
        draw_pane(f, panes[0], &app.left, app.active == PaneType::Left);
        draw_pane(f, panes[1], &app.right, app.active == PaneType::Right);
    }

    if let Mode::ConfirmDelete { items } = &app.mode {
        let popup = centered_rect(40, 20, f.size());
        let block = Block::default()
            .title("Confirm Deletion")
            .borders(Borders::ALL);
        let prompt = format!("Delete {} item(s)? (y/N)", items.len());
        let paragraph = Paragraph::new(prompt)
            .block(block)
            .alignment(Alignment::Center);
        f.render_widget(Clear, popup);
        f.render_widget(paragraph, popup);
    }

    if let Mode::Sort { selected } = &app.mode {
        let popup = centered_rect(40, 20, f.size());
        let block = Block::default().title("Sort By").borders(Borders::ALL);
        let items: Vec<ListItem> = crate::fs_utils::SORT_OPTIONS
            .iter()
            .enumerate()
            .map(|(i, option)| {
                let style = if i == *selected {
                    Style::default().add_modifier(Modifier::REVERSED)
                } else {
                    Style::default()
                };
                ListItem::new(Spans::from(Span::styled(*option, style)))
            })
            .collect();
        let list = List::new(items).block(block);
        f.render_widget(Clear, popup);
        f.render_widget(list, popup);
    }

    if let Some(footer) = footer_area {
        let prompt = match &app.mode {
            Mode::Search { query } => format!("/{query}"),
            Mode::Rename { original, buffer } => format!("rename: {original} -> {buffer}"),
            _ => String::new(),
        };
        let paragraph = Paragraph::new(prompt);
        f.render_widget(paragraph, footer);
    }
}

fn draw_pane<B: Backend>(f: &mut Frame<B>, area: Rect, pane: &Pane, active: bool) {
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
        .enumerate()
        .map(|(i, e)| {
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
            let marker = if pane.marked.contains(&i) { "*" } else { " " };
            ListItem::new(Spans::from(vec![
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

/// Helper to create a centered rect using the given percentage width and height of the available rect
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
