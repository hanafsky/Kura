#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::{
    collections::HashSet,
    fs, io,
    path::{Path, PathBuf},
    time::Duration,
};

use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Span, Spans},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap},
    Frame, Terminal,
};
use viuer::{print_from_file, Config};

enum PaneType {
    Left,
    Right,
}

enum Mode {
    Filer,
    /// Visual multi-selection mode (anchor index for selection range)
    Visual {
        anchor: usize,
    },
    Viewer {
        content: String,
        title: String,
        offset: u16,
    },
    ConfirmDelete {
        items: Vec<PathBuf>,
    },
}

struct Pane {
    items: Vec<fs::DirEntry>,
    selected: usize,
    current_dir: PathBuf,
    marked: HashSet<usize>,
}

impl Pane {
    fn new(path: PathBuf) -> io::Result<Self> {
        let mut entries = fs::read_dir(&path)?.collect::<Result<Vec<_>, _>>()?;
        entries.sort_by_key(|e| e.file_name());
        Ok(Self {
            items: entries,
            selected: 0,
            current_dir: path,
            marked: HashSet::new(),
        })
    }

    fn refresh(&mut self) -> io::Result<()> {
        let mut entries = fs::read_dir(&self.current_dir)?.collect::<Result<Vec<_>, _>>()?;
        entries.sort_by_key(|e| e.file_name());
        self.items = entries;
        self.selected = 0;
        self.marked.clear();
        Ok(())
    }

    fn toggle_mark(&mut self) {
        if self.marked.contains(&self.selected) {
            self.marked.remove(&self.selected);
        } else {
            self.marked.insert(self.selected);
        }
    }
}

struct App {
    left: Pane,
    right: Pane,
    active: PaneType,
    mode: Mode,
    clipboard: Vec<PathBuf>,
}

impl App {
    fn new() -> io::Result<Self> {
        let cwd = std::env::current_dir()?;
        Ok(Self {
            left: Pane::new(cwd.clone())?,
            right: Pane::new(cwd)?,
            active: PaneType::Left,
            mode: Mode::Filer,
            clipboard: Vec::new(),
        })
    }

    fn current_pane_mut(&mut self) -> &mut Pane {
        match self.active {
            PaneType::Left => &mut self.left,
            PaneType::Right => &mut self.right,
        }
    }

    fn switch_pane(&mut self) {
        self.active = match self.active {
            PaneType::Left => PaneType::Right,
            PaneType::Right => PaneType::Left,
        };
    }

    fn on_up(&mut self) {
        let pane = self.current_pane_mut();
        if pane.selected > 0 {
            pane.selected -= 1;
        }
    }

    fn on_down(&mut self) {
        let pane = self.current_pane_mut();
        if pane.selected + 1 < pane.items.len() {
            pane.selected += 1;
        }
    }

    fn on_left(&mut self) {
        let pane = self.current_pane_mut();
        if let Some(parent) = pane.current_dir.parent() {
            pane.current_dir = parent.to_path_buf();
            let _ = pane.refresh();
        }
    }

    fn on_enter(&mut self) {
        let pane = self.current_pane_mut();
        if let Some(entry) = pane.items.get(pane.selected) {
            let path = entry.path();
            if path.is_dir() {
                pane.current_dir = path;
                let _ = pane.refresh();
            } else if let Ok(content) = fs::read_to_string(&path) {
                let title = path
                    .file_name()
                    .map(|os_str| os_str.to_string_lossy().into_owned())
                    .unwrap_or_default();
                self.mode = Mode::Viewer {
                    content,
                    title,
                    offset: 0,
                };
            }
        }
    }

    /// Copy marked entries or the current entry into the clipboard.
    fn copy_selection(&mut self) {
        let items = {
            let pane = self.current_pane_mut();
            let mut items = Vec::new();
            if !pane.marked.is_empty() {
                for &i in &pane.marked {
                    if let Some(entry) = pane.items.get(i) {
                        items.push(entry.path());
                    }
                }
            } else if let Some(entry) = pane.items.get(pane.selected) {
                items.push(entry.path());
            }
            pane.marked.clear();
            items
        };
        self.clipboard = items;
    }

    /// Paste clipboard entries into the current directory.
    fn paste(&mut self) {
        let items = self.clipboard.clone();
        let dst_dir = self.current_pane_mut().current_dir.clone();
        for src in &items {
            let file_name = match src.file_name() {
                Some(name) => name,
                None => continue,
            };
            let dst = dst_dir.join(file_name);
            if src.is_dir() {
                if let Err(e) = copy_dir_recursively(src, &dst) {
                    eprintln!("Failed to copy directory {:?}: {}", src, e);
                }
            } else if let Err(e) = fs::copy(src, &dst) {
                eprintln!("Failed to copy file {:?}: {}", src, e);
            }
        }
        let _ = self.current_pane_mut().refresh();
    }

    /// Delete the given files or directories from disk and refresh the pane.
    fn delete_items(&mut self, items: &[PathBuf]) {
        let pane = self.current_pane_mut();
        for path in items {
            if path.is_dir() {
                if let Err(e) = fs::remove_dir_all(path) {
                    eprintln!("Failed to delete directory {:?}: {}", path, e);
                }
            } else {
                if let Err(e) = fs::remove_file(path) {
                    eprintln!("Failed to delete file {:?}: {}", path, e);
                }
            }
        }
        let _ = pane.refresh();
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new()?;
    let res = run_app(&mut terminal, &mut app);

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    if let Err(err) = res {
        eprintln!("{err:?}");
    }
    Ok(())
}

fn is_image(path: &Path) -> bool {
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

fn show_image<B: ratatui::backend::Backend + io::Write>(
    terminal: &mut Terminal<B>,
    path: &Path,
) -> io::Result<()> {
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    let config = Config::default();
    if let Err(err) = print_from_file(path.to_string_lossy().as_ref(), &config) {
        eprintln!("Failed to display image: {}", err);
    }

    loop {
        if event::poll(Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                if key.code == KeyCode::Enter {
                    break;
                }
            }
        }
    }
    // restore alternate screen and clear it before redrawing the UI
    execute!(
        terminal.backend_mut(),
        EnterAlternateScreen,
        EnableMouseCapture
    )?;
    terminal.clear()?;
    <B as ratatui::backend::Backend>::flush(terminal.backend_mut())?;
    enable_raw_mode()?;
    Ok(())
}

/// Recursively copy a directory.
fn copy_dir_recursively(src: &Path, dst: &Path) -> io::Result<()> {
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

fn run_app<B: ratatui::backend::Backend + io::Write>(
    terminal: &mut Terminal<B>,
    app: &mut App,
) -> io::Result<()> {
    let mut prefix: usize = 0;
    // track first 'g' press to detect 'gg' sequence
    let mut last_key_g = false;
    loop {
        terminal.draw(|f| ui(f, app))?;
        if event::poll(Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                if key.code == KeyCode::Char('q') {
                    return Ok(());
                }
                if let KeyCode::Char(c) = key.code {
                    if c.is_ascii_digit() {
                        prefix = prefix
                            .saturating_mul(10)
                            .saturating_add(c.to_digit(10).unwrap() as usize);
                        continue;
                    }
                }
                let count = if prefix > 0 { prefix } else { 1 };
                prefix = 0;
                // Vim-style 'gg' (go top) and 'G' (go bottom) in filer or viewer
                if let KeyCode::Char(c) = key.code {
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
                                    // scroll to bottom (last line at top)
                                    *offset = total.saturating_sub(1) as u16;
                                }
                                _ => {}
                            }
                            continue;
                        }
                        _ => last_key_g = false,
                    }
                }
                // Visual mode: multi-selection movement and exit
                let anchor_opt = if let Mode::Visual { anchor } = &app.mode {
                    Some(*anchor)
                } else {
                    None
                };
                if let Some(anchor) = anchor_opt {
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
                match &mut app.mode {
                    Mode::ConfirmDelete { items } => match key.code {
                        KeyCode::Char('y') | KeyCode::Enter => {
                            let to_delete = items.clone();
                            app.mode = Mode::Filer;
                            app.delete_items(&to_delete);
                        }
                        KeyCode::Char('n') | KeyCode::Esc => {
                            app.mode = Mode::Filer;
                        }
                        _ => {}
                    },
                    Mode::Viewer { offset, .. } => match key.code {
                        KeyCode::Char('j') => *offset = offset.saturating_add(count as u16),
                        KeyCode::Char('k') => *offset = offset.saturating_sub(count as u16),
                        KeyCode::Enter => app.mode = Mode::Filer,
                        _ => {}
                    },
                    Mode::Filer => match key.code {
                        KeyCode::Char('j') => (0..count).for_each(|_| app.on_down()),
                        KeyCode::Char('k') => (0..count).for_each(|_| app.on_up()),
                        KeyCode::Char('x') => {
                            let items = {
                                let pane = app.current_pane_mut();
                                let mut sel = Vec::new();
                                if !pane.marked.is_empty() {
                                    for &i in &pane.marked {
                                        if let Some(e) = pane.items.get(i) {
                                            sel.push(e.path());
                                        }
                                    }
                                } else if let Some(e) = pane.items.get(pane.selected) {
                                    sel.push(e.path());
                                }
                                sel
                            };
                            app.mode = Mode::ConfirmDelete { items };
                        }
                        KeyCode::Char('X') => {
                            let items = {
                                let pane = app.current_pane_mut();
                                let mut sel = Vec::new();
                                if !pane.marked.is_empty() {
                                    for &i in &pane.marked {
                                        if let Some(e) = pane.items.get(i) {
                                            sel.push(e.path());
                                        }
                                    }
                                } else if let Some(e) = pane.items.get(pane.selected) {
                                    sel.push(e.path());
                                }
                                sel
                            };
                            app.delete_items(&items);
                        }
                        KeyCode::Enter => {
                            let (is_img, path) = {
                                let pane = app.current_pane_mut();
                                pane.items
                                    .get(pane.selected)
                                    .map(|entry| {
                                        let p = entry.path();
                                        (p.is_file() && is_image(&p), p)
                                    })
                                    .unwrap_or((false, PathBuf::new()))
                            };
                            if is_img {
                                app.switch_pane();
                                show_image(terminal, &path)?;
                            } else {
                                app.on_enter();
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
                        // start visual multi-selection (Vim 'V')
                        KeyCode::Char('V') => {
                            let pane = app.current_pane_mut();
                            let anchor = pane.selected;
                            pane.marked.clear();
                            pane.marked.insert(anchor);
                            app.mode = Mode::Visual { anchor };
                        }
                        // toggle mark on current entry
                        KeyCode::Char('v') => app.current_pane_mut().toggle_mark(),
                        KeyCode::Char('y') => app.copy_selection(),
                        KeyCode::Char('p') => app.paste(),
                        _ => {}
                    },
                    // other modes (e.g., Visual) are handled above
                    _ => {}
                }
            }
        }
    }
}

fn ui<B: ratatui::backend::Backend>(f: &mut Frame<B>, app: &App) {
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

    let area = chunks[1];
    if let Mode::Viewer {
        content,
        title,
        offset,
    } = &app.mode
    {
        let block = Block::default().borders(Borders::ALL).title(title.as_str());
        let inner_height = area.height.saturating_sub(2) as usize;
        let number_width = inner_height.to_string().len().max(1);
        // clamp offset to valid range to allow 'G' to scroll to bottom
        let total_lines = content.lines().count();
        let max_off = total_lines.saturating_sub(inner_height);
        let start = (*offset as usize).min(max_off) as usize;
        let lines = content.lines().skip(start).take(inner_height);
        let numbered: Vec<Spans> = lines
            .enumerate()
            .map(|(i, line)| {
                let rel = i;
                let num = format!("{:>width$} ", rel, width = number_width);
                Spans::from(vec![
                    Span::styled(num, Style::default().fg(Color::DarkGray)),
                    Span::raw(line),
                ])
            })
            .collect();
        let paragraph = Paragraph::new(numbered)
            .block(block)
            .wrap(Wrap { trim: false });
        f.render_widget(paragraph, area);
    } else {
        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(area);
        draw_pane(
            f,
            chunks[0],
            &app.left,
            matches!(app.active, PaneType::Left),
        );
        draw_pane(
            f,
            chunks[1],
            &app.right,
            matches!(app.active, PaneType::Right),
        );
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
}

fn draw_pane<B: ratatui::backend::Backend>(
    f: &mut Frame<B>,
    area: Rect,
    pane: &Pane,
    active: bool,
) {
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
            let os_name = e.file_name();
            let name = os_name.to_string_lossy().into_owned();
            let path = e.path();
            let style = if path.is_dir() {
                Style::default().fg(Color::Blue)
            } else if name.starts_with('.') {
                Style::default().fg(Color::Red)
            } else if e
                .metadata()
                .map(|m| m.permissions().mode() & 0o111 != 0)
                .unwrap_or(false)
            {
                Style::default().fg(Color::Green)
            } else {
                Style::default()
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
