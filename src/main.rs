#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::{fs, io, path::PathBuf, time::Duration};

use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Span, Spans},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap},
    Frame, Terminal,
};

enum PaneType {
    Left,
    Right,
}

enum Mode {
    Filer,
    Viewer {
        content: String,
        title: String,
        offset: u16,
    },
}

struct Pane {
    items: Vec<fs::DirEntry>,
    selected: usize,
    current_dir: PathBuf,
}

impl Pane {
    fn new(path: PathBuf) -> io::Result<Self> {
        let mut entries = fs::read_dir(&path)?.collect::<Result<Vec<_>, _>>()?;
        entries.sort_by_key(|e| e.file_name());
        Ok(Self {
            items: entries,
            selected: 0,
            current_dir: path,
        })
    }

    fn refresh(&mut self) -> io::Result<()> {
        let mut entries = fs::read_dir(&self.current_dir)?.collect::<Result<Vec<_>, _>>()?;
        entries.sort_by_key(|e| e.file_name());
        self.items = entries;
        self.selected = 0;
        Ok(())
    }
}

struct App {
    left: Pane,
    right: Pane,
    active: PaneType,
    mode: Mode,
}

impl App {
    fn new() -> io::Result<Self> {
        let cwd = std::env::current_dir()?;
        Ok(Self {
            left: Pane::new(cwd.clone())?,
            right: Pane::new(cwd)?,
            active: PaneType::Left,
            mode: Mode::Filer,
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

fn run_app<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    app: &mut App,
) -> io::Result<()> {
    loop {
        terminal.draw(|f| ui(f, app))?;
        if event::poll(Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                if key.code == KeyCode::Char('q') {
                    return Ok(());
                }
                match &mut app.mode {
                    Mode::Viewer { offset, .. } => match key.code {
                        KeyCode::Char('j') => *offset = offset.saturating_add(1),
                        KeyCode::Char('k') => *offset = offset.saturating_sub(1),
                        KeyCode::Enter => app.mode = Mode::Filer,
                        _ => {}
                    },
                    Mode::Filer => match key.code {
                        KeyCode::Char('j') => app.on_down(),
                        KeyCode::Char('k') => app.on_up(),
                        KeyCode::Enter => app.on_enter(),
                        KeyCode::Char('h') => match app.active {
                            PaneType::Left => app.on_left(),
                            PaneType::Right => app.switch_pane(),
                        },
                        KeyCode::Char('l') => match app.active {
                            PaneType::Left => app.switch_pane(),
                            PaneType::Right => app.on_left(),
                        },
                        _ => {}
                    },
                }
            }
        }
    }
}

fn ui<B: ratatui::backend::Backend>(f: &mut Frame<B>, app: &App) {
    if let Mode::Viewer {
        content,
        title,
        offset,
    } = &app.mode
    {
        let block = Block::default().borders(Borders::ALL).title(title.as_str());
        let paragraph = Paragraph::new(content.as_str())
            .block(block)
            .wrap(Wrap { trim: false })
            .scroll((*offset, 0));
        f.render_widget(paragraph, f.size());
    } else {
        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(f.size());
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
        .map(|e| {
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
            ListItem::new(Spans::from(vec![Span::styled(name, style)]))
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
