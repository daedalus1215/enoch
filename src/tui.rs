//! Interactive panel: live running timers on top, per-activity totals below.

use std::io::{self};
use std::time::Duration;

use anyhow::Result;
use chrono::Local;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph};
use ratatui::{Frame, Terminal};

use crate::db::{Db, StartOutcome};
use crate::timefmt::{day_bounds, fmt_clock, fmt_short, now_ms, week_bounds};

const TICK: Duration = Duration::from_millis(200);

#[derive(Clone, Copy, PartialEq)]
enum Period {
    Today,
    Week,
    AllTime,
}

impl Period {
    fn next(self) -> Self {
        match self {
            Period::Today => Period::Week,
            Period::Week => Period::AllTime,
            Period::AllTime => Period::Today,
        }
    }
    fn title(self) -> &'static str {
        match self {
            Period::Today => " TODAY",
            Period::Week => " WEEK (MON-SUN)",
            Period::AllTime => " ALL-TIME",
        }
    }
    fn bounds(self, now: chrono::NaiveDateTime) -> (i64, i64) {
        match self {
            Period::Today => day_bounds(now),
            Period::Week => week_bounds(now),
            Period::AllTime => (0, i64::MAX),
        }
    }
}

struct State {
    running: Vec<crate::db::Running>,
    totals: Vec<crate::db::Total>,
    names: Vec<String>,
    period: Period,
    selected: usize,
    prompt: Option<String>,
    complete_idx: Option<usize>,
    msg: Option<String>,
    last_second: i64,
    quit: bool,
}

impl State {
    fn new(db: &Db) -> Result<Self> {
        let mut s = State {
            running: Vec::new(),
            totals: Vec::new(),
            names: Vec::new(),
            period: Period::Today,
            selected: 0,
            prompt: None,
            complete_idx: None,
            msg: None,
            last_second: now_ms() / 1000,
            quit: false,
        };
        s.refresh(db)?;
        Ok(s)
    }

    fn refresh(&mut self, db: &Db) -> Result<()> {
        self.running = db.running()?;
        let (f, t) = self.period.bounds(Local::now().naive_local());
        self.totals = db.totals(f, t)?;
        self.names = db.names()?;
        if self.selected >= self.running.len() {
            self.selected = self.running.len().saturating_sub(1);
        }
        Ok(())
    }

    fn try_refresh(&mut self, db: &Db) {
        if let Err(e) = self.refresh(db) {
            self.msg = Some(format!("error: {e:#}"));
        }
    }

    fn move_sel(&mut self, d: isize) {
        if self.running.is_empty() {
            return;
        }
        self.selected = (self.selected as isize + d)
            .clamp(0, (self.running.len() - 1) as isize)
            as usize;
    }

    fn handle_browse_key(&mut self, db: &Db, k: &KeyEvent) {
        match k.code {
            KeyCode::Char('q') | KeyCode::Esc => self.quit = true,
            KeyCode::Char('s') => {
                self.prompt = Some(String::new());
                self.complete_idx = None;
            }
            KeyCode::Char('t') => {
                self.period = self.period.next();
                self.try_refresh(db);
            }
            KeyCode::Char('x') => {
                if let Some(r) = self.running.get(self.selected).cloned() {
                    let dur = fmt_short(now_ms() - r.started_at);
                    if let Err(e) = db.stop(std::slice::from_ref(&r.name)) {
                        self.msg = Some(format!("error: {e:#}"));
                    } else {
                        self.msg = Some(format!("stopped: {} ({dur})", r.name));
                        self.try_refresh(db);
                    }
                }
            }
            KeyCode::Char('j') | KeyCode::Down => self.move_sel(1),
            KeyCode::Char('k') | KeyCode::Up => self.move_sel(-1),
            _ => {}
        }
    }

    fn handle_prompt_char(&mut self, db: &Db, k: &KeyEvent) {
        let Some(prompt) = self.prompt.as_mut() else {
            return;
        };
        match k.code {
            KeyCode::Esc => {
                self.prompt = None;
            }
            KeyCode::Enter => {
                let name = prompt.trim().to_string();
                if !name.is_empty() {
                    match db.start(&name) {
                        Ok(StartOutcome::Started) => self.msg = Some(format!("started: {name}")),
                        Ok(StartOutcome::AlreadyRunning) => {
                            self.msg = Some(format!("already running: {name}"))
                        }
                        Err(e) => self.msg = Some(format!("error: {e:#}")),
                    }
                    self.try_refresh(db);
                }
                self.prompt = None;
            }
            KeyCode::Backspace => {
                prompt.pop();
                self.complete_idx = None;
            }
            KeyCode::Tab => {
                let prefix = prompt.to_lowercase();
                let idxs: Vec<usize> = self
                    .names
                    .iter()
                    .enumerate()
                    .filter(|(_, n)| n.to_lowercase().starts_with(&prefix))
                    .map(|(i, _)| i)
                    .collect();
                if !idxs.is_empty() {
                    let cur = self.complete_idx.unwrap_or(usize::MAX);
                    let next = idxs
                        .iter()
                        .position(|&i| i > cur)
                        .map(|p| idxs[p])
                        .unwrap_or(idxs[0]);
                    self.complete_idx = Some(next);
                    prompt.clone_from(&self.names[next]);
                }
            }
            KeyCode::Char(c) if !k.modifiers.contains(KeyModifiers::CONTROL) => {
                prompt.push(c);
                self.complete_idx = None;
            }
            _ => {}
        }
    }
}

fn draw(f: &mut Frame, s: &State) {
    let area = f.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(5),
            Constraint::Min(5),
            Constraint::Length(1),
        ])
        .split(area);

    let sel = s.prompt.is_none() && s.selected < s.running.len();
    let items: Vec<ListItem> = if s.running.is_empty() {
        vec![ListItem::new(Line::from(Span::styled(
            "nothing running - press s to start",
            Style::default().fg(Color::DarkGray),
        )))]
    } else {
        let now = now_ms();
        s.running
            .iter()
            .enumerate()
            .map(|(i, r)| {
                let is_sel = sel && i == s.selected;
                let dim = Style::default().fg(if is_sel {
                    Color::Cyan
                } else {
                    Color::Gray
                });
                let name_style = if is_sel {
                    Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::White)
                };
                ListItem::new(Line::from(vec![
                    Span::styled(format!("{:>11}", fmt_clock(now - r.started_at)), dim),
                    Span::raw("  "),
                    Span::styled(r.name.clone(), name_style),
                ]))
            })
            .collect()
    };
    f.render_widget(
        List::new(items).block(
            Block::default()
                .borders(Borders::ALL)
                .title(Span::styled(" RUNNING", Style::default().fg(Color::Gray))),
        ),
        chunks[0],
    );

    let items: Vec<ListItem> = if s.totals.is_empty() {
        vec![ListItem::new(Line::from(Span::styled(
            "no time recorded in this period yet",
            Style::default().fg(Color::DarkGray),
        )))]
    } else {
        s.totals
            .iter()
            .map(|t| {
                ListItem::new(Line::from(vec![
                    Span::styled(
                        format!("{:>11}", fmt_short(t.ms)),
                        Style::default().fg(Color::Gray),
                    ),
                    Span::raw("  "),
                    Span::styled(t.name.clone(), Style::default().fg(Color::White)),
                ]))
            })
            .collect()
    };
    f.render_widget(
        List::new(items).block(
            Block::default()
                .borders(Borders::ALL)
                .title(Span::styled(s.period.title(), Style::default().fg(Color::Gray))),
        ),
        chunks[1],
    );

    let bar = if let Some(p) = &s.prompt {
        Line::from(vec![
            Span::styled(" start: ", Style::default().fg(Color::Yellow)),
            Span::styled(p.clone(), Style::default().fg(Color::White)),
            Span::styled("█", Style::default().fg(Color::Yellow)),
            Span::styled(
                "   enter start · tab complete · esc cancel",
                Style::default().fg(Color::DarkGray),
            ),
        ])
    } else if let Some(m) = &s.msg {
        Line::from(vec![
            Span::styled(m.clone(), Style::default().fg(Color::Yellow)),
            Span::styled(
                "    s start · x stop · t period · j/k select · q quit",
                Style::default().fg(Color::DarkGray),
            ),
        ])
    } else {
        Line::from(vec![
            Span::styled("s", Style::default().fg(Color::Yellow)),
            Span::styled(" start · ", Style::default().fg(Color::DarkGray)),
            Span::styled("x", Style::default().fg(Color::Yellow)),
            Span::styled(" stop · ", Style::default().fg(Color::DarkGray)),
            Span::styled("t", Style::default().fg(Color::Yellow)),
            Span::styled(" period · ", Style::default().fg(Color::DarkGray)),
            Span::styled("j/k", Style::default().fg(Color::Yellow)),
            Span::styled(" select · ", Style::default().fg(Color::DarkGray)),
            Span::styled("q", Style::default().fg(Color::Yellow)),
            Span::styled(" quit", Style::default().fg(Color::DarkGray)),
        ])
    };
    f.render_widget(Paragraph::new(bar), chunks[2]);
}

pub fn run() -> Result<()> {
    let db = Db::open()?;
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let mut term = Terminal::new(CrosstermBackend::new(stdout))?;
    let mut state = State::new(&db)?;
    let result: Result<()> = loop {
        // Keep the report panel live: refresh once per second.
        let second = now_ms() / 1000;
        if second != state.last_second {
            state.last_second = second;
            state.try_refresh(&db);
        }
        term.draw(|f| draw(f, &state))?;
        if state.quit {
            break Ok(());
        }
        if event::poll(TICK)? {
            match event::read()? {
                Event::Key(k) if k.kind == KeyEventKind::Press => {
                    if state.prompt.is_some() {
                        state.handle_prompt_char(&db, &k);
                    } else {
                        state.handle_browse_key(&db, &k);
                    }
                }
                _ => {}
            }
        }
    };
    disable_raw_mode()?;
    execute!(term.backend_mut(), LeaveAlternateScreen)?;
    result
}
