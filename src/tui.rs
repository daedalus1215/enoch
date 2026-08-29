//! Interactive panel: live running timers, browsable day-by-day history.

use std::collections::{HashMap, HashSet};
use std::io::{self};
use std::time::Duration;

use anyhow::Result;
use chrono::{Datelike, Local, NaiveDate};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{block::Title, Block, BorderType, Borders, List, ListItem, Paragraph};
use ratatui::{Frame, Terminal};

use crate::db::{Db, Running, Scope, SessionRow, StartOutcome, Total};
use crate::theme;
use crate::timefmt::{
    date_of, day_bounds, fmt_clock, fmt_day_title, fmt_hm, fmt_month, fmt_range, fmt_short,
    local_midnight_ms, month_bounds, now_ms, parse_hm_on, week_bounds,
};

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
            Period::Today => "TODAY",
            Period::Week => "WEEK (MON-SUN)",
            Period::AllTime => "ALL-TIME",
        }
    }
    fn bounds(self, d: chrono::NaiveDate) -> (i64, i64) {
        match self {
            Period::Today => day_bounds(d),
            Period::Week => week_bounds(d),
            Period::AllTime => (0, i64::MAX),
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Live,
    Hist,
    Manage,
    Stats,
    Activity,
}

#[derive(Clone, Copy, PartialEq)]
enum PromptKind {
    Start,
    AddActivity,
    AddStart,
    AddEnd,
    EditActivity,
    EditStart,
    EditEnd,
    CreateCategory,
    RenameCategory,
    MoveActivity,
    ConfirmDelete,
}

impl PromptKind {
    fn completes_activity(self) -> bool {
        matches!(
            self,
            PromptKind::Start
                | PromptKind::AddActivity
                | PromptKind::EditActivity
        )
    }
    fn completes_category(self) -> bool {
        matches!(self, PromptKind::MoveActivity)
    }
    fn label(self) -> &'static str {
        match self {
            PromptKind::Start => " start",
            PromptKind::AddActivity => " add · activity",
            PromptKind::AddStart => " add · start (HH:MM)",
            PromptKind::AddEnd => " add · end (HH:MM)",
            PromptKind::EditActivity => " edit · activity",
            PromptKind::EditStart => " edit · start (HH:MM)",
            PromptKind::EditEnd => " edit · end (HH:MM)",
            PromptKind::CreateCategory => " new category",
            PromptKind::RenameCategory => " rename category",
            PromptKind::MoveActivity => " move → category",
            PromptKind::ConfirmDelete => "",
        }
    }
    fn hint(self) -> &'static str {
        match self {
            PromptKind::Start
            | PromptKind::AddActivity
            | PromptKind::EditActivity => "enter next · tab complete · esc cancel",
            PromptKind::AddStart | PromptKind::EditStart => "enter next · esc cancel",
            PromptKind::AddEnd | PromptKind::EditEnd => "enter done · esc cancel",
            PromptKind::CreateCategory | PromptKind::RenameCategory => {
                "enter save · esc cancel"
            }
            PromptKind::MoveActivity => "enter save · tab complete · empty = none",
            PromptKind::ConfirmDelete => "y yes · n/esc no",
        }
    }
}

struct Prompt {
    kind: PromptKind,
    text: String,
    complete_idx: Option<usize>,
    /// True once the user typed; the prefill then no longer shadows input.
    touched: bool,
}

impl Prompt {
    fn new(kind: PromptKind, text: impl Into<String>) -> Self {
        Self {
            kind,
            text: text.into(),
            complete_idx: None,
            touched: false,
        }
    }
}

/// Browsable history: a selected day plus the month grid's data markers.
struct Hist {
    sel: NaiveDate,
    scope: Scope,
    day_totals: Vec<Total>,
    sessions: Vec<SessionRow>,
    month_days: HashMap<i64, i64>,
    /// Year-at-a-glance view: per-month bars for `year`.
    year_view: bool,
    year: i32,
    year_days: Vec<(NaiveDate, i64)>,
}

impl Hist {
    fn new(sel: NaiveDate) -> Self {
        Self {
            sel,
            scope: Scope::All,
            day_totals: Vec::new(),
            sessions: Vec::new(),
            month_days: HashMap::new(),
            year_view: false,
            year: sel.year(),
            year_days: Vec::new(),
        }
    }

    /// Reload the selected day and the month grid's data markers.
    fn refresh(&mut self, db: &Db) -> Result<()> {
        let (f, t) = day_bounds(self.sel);
        self.day_totals = db.totals(f, t, &self.scope)?;
        self.sessions = db.sessions_in(f, t, &self.scope)?;
        let (mf, mt) = month_bounds(self.sel);
        self.month_days = month_day_map(&db.sessions_in(mf, mt, &self.scope)?, self.sel);
        Ok(())
    }

    /// Reload the selected year's per-day totals for the year view.
    fn refresh_year(&mut self, db: &Db) {
        let jan = NaiveDate::from_yo_opt(self.year, 1)
            .unwrap_or_else(|| self.sel.with_year(self.year).unwrap_or(self.sel));
        let next_jan = NaiveDate::from_yo_opt(self.year + 1, 1)
            .unwrap_or(jan + chrono::Duration::days(365));
        let (f, t) = (
            local_midnight_ms(jan),
            local_midnight_ms(next_jan),
        );
        let days = db.day_totals(f, t, &self.scope).unwrap_or_default();
        self.year_days = days
            .into_iter()
            .filter_map(|(s, ms)| {
                NaiveDate::parse_from_str(&s, "%Y-%m-%d")
                    .ok()
                    .map(|d| (d, ms))
            })
            .collect();
    }
}

/// One row in the manage view: a category header or an activity under it.
#[derive(Clone)]
enum ManageRow {
    Category { id: i64, name: String, total: i64 },
    Activity { name: String, total: i64 },
}
struct Manage {
    rows: Vec<ManageRow>,
    sel: usize,
}

impl Manage {
    fn new() -> Self {
        Self {
            rows: Vec::new(),
            sel: 0,
        }
    }

    /// Rebuild the flat row list: unassigned group first, then categories.
    fn refresh(&mut self, db: &Db) -> Result<()> {
        let mut rows: Vec<ManageRow> = Vec::new();
        let unassigned = db.unassigned()?;
        if !unassigned.is_empty() {
            let total: i64 = unassigned.iter().map(|t| t.ms).sum();
            rows.push(ManageRow::Category {
                id: -1,
                name: "No category".to_string(),
                total,
            });
            for t in unassigned {
                rows.push(ManageRow::Activity {
                    name: t.name,
                    total: t.ms,
                });
            }
        }
        for c in db.categories()? {
            rows.push(ManageRow::Category {
                id: c.id,
                name: c.name.clone(),
                total: c.total,
            });
            for a in &c.activities {
                rows.push(ManageRow::Activity {
                    name: a.name.clone(),
                    total: a.total,
                });
            }
        }
        if self.sel >= rows.len() {
            self.sel = rows.len().saturating_sub(1);
        }
        self.rows = rows;
        Ok(())
    }
}

/// One row in the activity rail: the all-time root, a category header, or an activity.
#[derive(Clone, Debug, PartialEq)]
enum RailRow {
    All,
    Header(String),
    Activity(String),
}

/// Activity-history mode: a rail of activities plus the scoped history.
struct Activity {
    rows: Vec<RailRow>,
    sel: usize,
    /// Keyboard focus: the rail, or the history main area.
    focus_rail: bool,
}

impl Activity {
    fn new() -> Self {
        Self {
            rows: Vec::new(),
            sel: 0,
            focus_rail: true,
        }
    }
}

/// 365-day heatmap: per-day totals for the trailing year plus the selected day.
struct Stats {
    days: Vec<(NaiveDate, i64)>,
    sel: NaiveDate,
    detail: Vec<Total>,
}

impl Stats {
    fn new(sel: NaiveDate) -> Self {
        Self {
            days: Vec::new(),
            sel,
            detail: Vec::new(),
        }
    }

    /// Refetch the year window and the selected day's per-activity detail.
    fn refresh(&mut self, db: &Db) {
        let today = Local::now().date_naive();
        if self.sel > today {
            self.sel = today;
        }
        let start = today - chrono::Duration::days(363);
        let (f, t) = (
            local_midnight_ms(start),
            local_midnight_ms(today + chrono::Duration::days(1)),
        );
        let days = db.day_totals(f, t, &Scope::All).unwrap_or_default();
        self.days = days
            .into_iter()
            .filter_map(|(s, ms)| {
                NaiveDate::parse_from_str(&s, "%Y-%m-%d")
                    .ok()
                    .map(|d| (d, ms))
            })
            .collect();
        let (df, dt) = day_bounds(self.sel);
        self.detail = db.totals(df, dt, &Scope::All).unwrap_or_default();
    }

    /// Move the selected day by `d` days, clamped to the year window.
    fn nudge(&mut self, db: &Db, d: i64) {
        let today = Local::now().date_naive();
        let start = today - chrono::Duration::days(363);
        let Some(x) = self.sel.checked_add_signed(chrono::Duration::days(d)) else {
            return;
        };
        if x < start || x > today {
            return;
        }
        self.sel = x;
        let (f, t) = day_bounds(x);
        self.detail = db.totals(f, t, &Scope::All).unwrap_or_default();
    }
}

/// Heatmap intensity: 0 empty, 1..4 increasing time.
fn heat_level(ms: i64) -> u8 {
    const H: i64 = 3_600_000;
    const M: i64 = 60_000;
    if ms >= 8 * H {
        4
    } else if ms >= 4 * H {
        3
    } else if ms >= H {
        2
    } else if ms >= 15 * M {
        1
    } else {
        0
    }
}

const HEAT_COLORS: [Color; 5] = [
    Color::DarkGray,
    Color::Rgb(47, 104, 74),
    Color::Rgb(66, 153, 102),
    Color::Rgb(100, 199, 134),
    Color::Rgb(176, 232, 176),
];

/// Attribute each session's milliseconds to the local days it touches.
fn month_day_map(sess: &[SessionRow], month: NaiveDate) -> HashMap<i64, i64> {
    let now = now_ms();
    let first = month.with_day(1).expect("day 1 exists");
    let last = first
        .with_day(first.num_days_in_month().into())
        .expect("last day exists");
    let mut map: HashMap<i64, i64> = HashMap::new();
    for s in sess {
        let end = s.ended_at.unwrap_or(now).min(now);
        if end <= s.started_at {
            continue;
        }
        let lo = date_of(s.started_at).max(first);
        let hi = date_of(end).min(last);
        let mut d = lo;
        while d <= hi {
            let (fs, fe) = day_bounds(d);
            let (a, b) = (fs.max(s.started_at), fe.min(end));
            if b > a {
                *map.entry(fs).or_insert(0) += b - a;
            }
            d += chrono::Duration::days(1);
        }
    }
    map
}

struct State {
    mode: Mode,
    running: Vec<Running>,
    totals: Vec<Total>,
    names: Vec<String>,
    period: Period,
    selected: usize,
    prompt: Option<Prompt>,
    cat_names: Vec<String>,
    sess_sel: usize,
    pending_name: Option<String>,
    pending_start: Option<i64>,
    pending_edit: Option<SessionRow>,
    pending_cat: Option<(i64, String)>,
    pending_activity: Option<String>,
    manage: Manage,
    activity: Activity,
    msg: Option<String>,
    help: bool,
    last_second: i64,
    hist: Hist,
    stats: Stats,
    quit: bool,
}

impl State {
    fn new(db: &Db) -> Result<Self> {
        let mut s = State {
            mode: Mode::Live,
            running: Vec::new(),
            totals: Vec::new(),
            names: Vec::new(),
            period: Period::Today,
            selected: 0,
            prompt: None,
            cat_names: Vec::new(),
            sess_sel: 0,
            pending_name: None,
            pending_start: None,
            pending_edit: None,
            pending_cat: None,
            pending_activity: None,
            manage: Manage::new(),
            activity: Activity::new(),
            msg: None,
            help: false,
            last_second: now_ms() / 1000,
            hist: Hist::new(Local::now().date_naive()),
            stats: Stats::new(Local::now().date_naive()),
            quit: false,
        };
        s.refresh_live(db)?;
        s.refresh_manage(db);
        Ok(s)
    }

    fn refresh_live(&mut self, db: &Db) -> Result<()> {
        self.running = db.running()?;
        let (f, t) = self.period.bounds(Local::now().naive_local().date());
        self.totals = db.totals(f, t, &Scope::All)?;
        self.names = db.names()?;
        self.cat_names = db.category_names().unwrap_or_default();
        if self.selected >= self.running.len() {
            self.selected = self.running.len().saturating_sub(1);
        }
        Ok(())
    }

    /// Refresh whichever mode is on screen once per tick.
    fn try_tick(&mut self, db: &Db) {
        let today = Local::now().date_naive();
        // Past days are immutable; only today's views need live refresh.
        let needs_refresh = self.mode == Mode::Live
            || self.mode == Mode::Manage
            || self.hist.year_view
            || self.hist.sel == today
            || self.stats.sel == today;
        if !needs_refresh {
            return;
        }
        let r = match self.mode {
            Mode::Live => self.refresh_live(db),
            Mode::Hist | Mode::Activity => {
                if self.hist.year_view {
                    self.hist.refresh_year(db);
                    Ok(())
                } else {
                    self.hist.refresh(db)
                }
            }
            Mode::Manage => {
                self.refresh_manage(db);
                Ok(())
            }
            Mode::Stats => {
                self.stats.refresh(db);
                Ok(())
            }
        };
        if let Err(e) = r {
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

    /// Open the add-session prompt for day `d` (09:00 prefill).
    fn open_add_prompt(&mut self, d: NaiveDate) {
        self.hist.sel = d;
        self.prompt = Some(Prompt::new(PromptKind::AddActivity, String::new()));
        self.pending_start = d
            .and_hms_opt(9, 0, 0)
            .map(|t| t.and_utc().timestamp_millis());
    }

    fn handle_live_key(&mut self, db: &Db, k: &KeyEvent) {
        match k.code {
            KeyCode::Char('q') | KeyCode::Esc => self.quit = true,
            KeyCode::Char('?') => self.help = true,
            KeyCode::Char('2') => {
                self.mode = Mode::Hist;
                self.hist.scope = Scope::All;
                self.hist.year_view = false;
                if let Err(e) = self.hist.refresh(db) {
                    self.msg = Some(format!("error: {e:#}"));
                }
            }
            KeyCode::Char('3') => {
                self.mode = Mode::Manage;
                self.refresh_manage(db);
            }
            KeyCode::Char('4') => {
                self.mode = Mode::Stats;
                self.stats.refresh(db);
            }
            KeyCode::Char('5') => {
                self.mode = Mode::Activity;
                self.refresh_activity(db);
            }
            KeyCode::Char('s') => {
                self.prompt = Some(Prompt::new(PromptKind::Start, String::new()));
            }
            KeyCode::Char('a') => self.open_add_prompt(Local::now().date_naive()),
            KeyCode::Char('t') => {
                self.period = self.period.next();
                self.try_tick(db);
            }
            KeyCode::Char('x') => {
                if let Some(r) = self.running.get(self.selected).cloned() {
                    let dur = fmt_short(now_ms() - r.started_at);
                    if let Err(e) = db.stop(std::slice::from_ref(&r.name)) {
                        self.msg = Some(format!("error: {e:#}"));
                    } else {
                        self.msg = Some(format!("stopped: {} ({dur})", r.name));
                        self.try_tick(db);
                    }
                } else {
                    self.msg = Some("nothing running".to_string());
                }
            },
            KeyCode::Char('j') | KeyCode::Down => self.move_sel(1),
            KeyCode::Char('k') | KeyCode::Up => self.move_sel(-1),
            _ => {}
        }
    }

    fn handle_hist_key(&mut self, db: &Db, k: &KeyEvent) {
        if self.hist.year_view {
            self.handle_hist_year_key(db, k);
            return;
        }
        match k.code {
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Char('?') => self.help = true,
            KeyCode::Char('1') | KeyCode::Esc => self.mode = Mode::Live,
            KeyCode::Char('3') => {
                self.mode = Mode::Manage;
                self.refresh_manage(db);
            }
            KeyCode::Char('4') => {
                self.mode = Mode::Stats;
                self.stats.refresh(db);
            }
            KeyCode::Char('5') => {
                self.mode = Mode::Activity;
                self.refresh_activity(db);
            }
            KeyCode::Char('y') => {
                self.hist.year_view = true;
                self.hist.year = self.hist.sel.year();
                self.hist.refresh_year(db);
            }
            KeyCode::Char('j') | KeyCode::Down => self.move_sess(1),
            KeyCode::Char('k') | KeyCode::Up => self.move_sess(-1),
            KeyCode::Char('a') => self.open_add_prompt(self.hist.sel),
            KeyCode::Char('h') => self.nudge_month(db, -1),
            KeyCode::Char('l') => self.nudge_month(db, 1),
            KeyCode::Char('t') => self.set_sel(db, Local::now().date_naive()),
            KeyCode::Char('n') => self.nudge_day(db, 1),
            KeyCode::Char('p') => self.nudge_day(db, -1),
            KeyCode::Char('e') | KeyCode::Char('d') => {
                let Some(row) = self.hist.sessions.get(self.sess_sel).cloned() else {
                    return;
                };
                if row.ended_at.is_none() {
                    self.msg = Some(
                        "cannot modify a running session - stop it first".to_string(),
                    );
                    return;
                }
                self.pending_edit = Some(row.clone());
                if k.code == KeyCode::Char('e') {
                    self.prompt = Some(Prompt::new(PromptKind::EditActivity, row.name));
                } else {
                    self.prompt = Some(Prompt::new(PromptKind::ConfirmDelete, String::new()));
                }
            }
            _ => {}
        }
    }

    /// Refresh the manage view (all-time rollups per category).
    fn refresh_manage(&mut self, db: &Db) {
        if let Err(e) = self.manage.refresh(db) {
            self.msg = Some(format!("error: {e:#}"));
        }
        self.cat_names = db.category_names().unwrap_or_default();
    }

    fn move_manage_sel(&mut self, d: isize) {
        if self.manage.rows.is_empty() {
            return;
        }
        self.manage.sel = (self.manage.sel as isize + d)
            .clamp(0, (self.manage.rows.len() - 1) as isize)
            as usize;
    }

    fn handle_manage_key(&mut self, db: &Db, k: &KeyEvent) {
        match k.code {
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Char('?') => self.help = true,
            KeyCode::Char('1') => self.mode = Mode::Live,
            KeyCode::Char('2') | KeyCode::Esc => {
                self.mode = Mode::Hist;
                self.hist.scope = Scope::All;
                self.hist.year_view = false;
                if let Err(e) = self.hist.refresh(db) {
                    self.msg = Some(format!("error: {e:#}"));
                }
            }
            KeyCode::Char('4') => {
                self.mode = Mode::Stats;
                self.stats.refresh(db);
            }
            KeyCode::Char('5') => {
                self.mode = Mode::Activity;
                self.refresh_activity(db);
            }
            KeyCode::Char('j') | KeyCode::Down => self.move_manage_sel(1),
            KeyCode::Char('k') | KeyCode::Up => self.move_manage_sel(-1),
            KeyCode::Char('c') => {
                self.prompt = Some(Prompt::new(PromptKind::CreateCategory, String::new()));
            }
            KeyCode::Char('r') | KeyCode::Char('d') => {
                let Some(row) = self.manage.rows.get(self.manage.sel).cloned() else {
                    return;
                };
                let ManageRow::Category { id, name, .. } = row else {
                    self.msg = Some("select a category for that".to_string());
                    return;
                };
                self.pending_cat = Some((id, name.clone()));
                self.prompt = if k.code == KeyCode::Char('r') {
                    Some(Prompt::new(PromptKind::RenameCategory, name))
                } else {
                    Some(Prompt::new(PromptKind::ConfirmDelete, String::new()))
                };
            }
            KeyCode::Char('m') => {
                let Some(row) = self.manage.rows.get(self.manage.sel).cloned() else {
                    return;
                };
                let ManageRow::Activity { name, .. } = row else {
                    self.msg = Some("select an activity for that".to_string());
                    return;
                };
                self.pending_activity = Some(name);
                self.prompt = Some(Prompt::new(PromptKind::MoveActivity, String::new()));
            }
            _ => {}
        }
    }

    /// Year-at-a-glance keys: j/k month, h/l year, enter opens the month.
    fn handle_hist_year_key(&mut self, db: &Db, k: &KeyEvent) {
        match k.code {
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Char('?') => self.help = true,
            KeyCode::Char('1') | KeyCode::Esc => self.mode = Mode::Live,
            KeyCode::Char('3') => {
                self.mode = Mode::Manage;
                self.refresh_manage(db);
            }
            KeyCode::Char('4') => {
                self.mode = Mode::Stats;
                self.stats.refresh(db);
            }
            KeyCode::Char('5') => {
                self.mode = Mode::Activity;
                self.refresh_activity(db);
            }
            KeyCode::Char('y') | KeyCode::Enter => {
                self.hist.year_view = false;
                if let Err(e) = self.hist.refresh(db) {
                    self.msg = Some(format!("error: {e:#}"));
                }
            }
            KeyCode::Char('t') => {
                let today = Local::now().date_naive();
                self.hist.sel = today;
                self.hist.year = today.year();
                self.hist.refresh_year(db);
            }
            KeyCode::Char('j') | KeyCode::Down => self.nudge_hist_month(db, 1),
            KeyCode::Char('k') | KeyCode::Up => self.nudge_hist_month(db, -1),
            KeyCode::Char('h') | KeyCode::Left => self.nudge_hist_year(db, -1),
            KeyCode::Char('l') | KeyCode::Right => self.nudge_hist_year(db, 1),
            _ => {}
        }
    }

    /// Move the selected month (kept on its first day), wrapping across years.
    fn nudge_hist_month(&mut self, db: &Db, d: i64) {
        let m = self.hist.year as i64 * 12 + self.hist.sel.month0() as i64 + d;
        if m < 0 {
            return;
        }
        let y = (m / 12) as i32;
        let mo = (m % 12 + 1) as u32;
        let Some(d1) = NaiveDate::from_ymd_opt(y, mo, 1) else {
            return;
        };
        self.hist.sel = d1;
        self.hist.year = y;
        self.hist.refresh_year(db);
    }

    fn nudge_hist_year(&mut self, db: &Db, d: i64) {
        let y = (self.hist.year as i64 + d) as i32;
        if !(1970..=2999).contains(&y) {
            return;
        }
        self.hist.year = y;
        self.hist.refresh_year(db);
    }

    fn handle_stats_key(&mut self, db: &Db, k: &KeyEvent) {
        match k.code {
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Char('?') => self.help = true,
            KeyCode::Char('1') => self.mode = Mode::Live,
            KeyCode::Char('2') | KeyCode::Enter => {
                self.hist.sel = self.stats.sel;
                self.hist.year_view = false;
                self.mode = Mode::Hist;
                self.hist.scope = Scope::All;
                self.hist.year_view = false;
                if let Err(e) = self.hist.refresh(db) {
                    self.msg = Some(format!("error: {e:#}"));
                }
            }
            KeyCode::Char('3') => {
                self.mode = Mode::Manage;
                self.refresh_manage(db);
            }
            KeyCode::Char('5') => {
                self.mode = Mode::Activity;
                self.refresh_activity(db);
            }
            KeyCode::Char('t') => {
                self.stats.sel = Local::now().date_naive();
                self.stats.refresh(db);
            }
            KeyCode::Char('h') | KeyCode::Left => self.stats.nudge(db, -1),
            KeyCode::Char('l') | KeyCode::Right => self.stats.nudge(db, 1),
            KeyCode::Char('j') | KeyCode::Down => self.stats.nudge(db, 7),
            KeyCode::Char('k') | KeyCode::Up => self.stats.nudge(db, -7),
            _ => {}
        }
    }

    /// Enter activity mode: rebuild the rail and scope history to the selection.
    fn refresh_activity(&mut self, db: &Db) {
        let mut rows = vec![RailRow::All];
        let unassigned = db.unassigned().unwrap_or_default();
        if !unassigned.is_empty() {
            rows.push(RailRow::Header("no category".to_string()));
            for t in unassigned {
                rows.push(RailRow::Activity(t.name));
            }
        }
        for c in db.categories().unwrap_or_default() {
            rows.push(RailRow::Header(c.name));
            for a in &c.activities {
                rows.push(RailRow::Activity(a.name.clone()));
            }
        }
        self.activity.rows = rows;
        if self.activity.sel >= self.activity.rows.len() {
            self.activity.sel = 0;
        }
        self.apply_scope(db);
    }

    /// Scope the shared history cursor to the selected rail row and refetch.
    fn apply_scope(&mut self, db: &Db) {
        self.hist.scope = match self.activity.rows.get(self.activity.sel) {
            Some(RailRow::Activity(n)) => Scope::Activity(n.clone()),
            _ => Scope::All,
        };
        self.sess_sel = 0;
        if let Err(e) = self.hist.refresh(db) {
            self.msg = Some(format!("error: {e:#}"));
        }
    }

    /// Move the rail selection by `d`, skipping category headers.
    fn move_rail(&mut self, db: &Db, d: isize) {
        let n = self.activity.rows.len();
        if n <= 1 {
            return;
        }
        let mut i = self.activity.sel as isize;
        loop {
            let next = (i + d).clamp(0, (n - 1) as isize);
            if next == i {
                break;
            }
            i = next;
            if matches!(self.activity.rows[i as usize], RailRow::All | RailRow::Activity(_)) {
                break;
            }
        }
        if i != self.activity.sel as isize {
            self.activity.sel = i as usize;
            self.apply_scope(db);
        }
    }

    /// Activity mode: `tab` swaps focus between rail and history; the rail
    /// takes j/k, the history takes the usual hist keys.
    fn handle_activity_key(&mut self, db: &Db, k: &KeyEvent) {
        match k.code {
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Char('?') => self.help = true,
            KeyCode::Char('1') | KeyCode::Esc => self.mode = Mode::Live,
            KeyCode::Char('2') => {
                self.mode = Mode::Hist;
                self.hist.scope = Scope::All;
                self.hist.year_view = false;
                if let Err(e) = self.hist.refresh(db) {
                    self.msg = Some(format!("error: {e:#}"));
                }
            }
            KeyCode::Char('3') => {
                self.mode = Mode::Manage;
                self.refresh_manage(db);
            }
            KeyCode::Char('4') => {
                self.mode = Mode::Stats;
                self.stats.refresh(db);
            }
            KeyCode::Char('5') => self.activity.focus_rail = true,
            KeyCode::Tab | KeyCode::BackTab => {
                self.activity.focus_rail = !self.activity.focus_rail;
            }
            _ if self.activity.focus_rail => match k.code {
                KeyCode::Char('j') | KeyCode::Down => self.move_rail(db, 1),
                KeyCode::Char('k') | KeyCode::Up => self.move_rail(db, -1),
                _ => {}
            },
            _ => self.handle_hist_key(db, k),
        }
    }
    fn move_sess(&mut self, d: isize) {
        if self.hist.sessions.is_empty() {
            return;
        }
        self.sess_sel = (self.sess_sel as isize + d)
            .clamp(0, (self.hist.sessions.len() - 1) as isize)
            as usize;
    }

    /// Select a day in history. Navigation always refreshes; only the
    /// per-second tick skips immutable past days.
    fn set_sel(&mut self, db: &Db, d: NaiveDate) {
        self.hist.sel = d;
        self.sess_sel = 0;
        if let Err(e) = self.hist.refresh(db) {
            self.msg = Some(format!("error: {e:#}"));
        }
    }

    fn nudge_day(&mut self, db: &Db, d: i64) {
        if let Some(x) = self.hist.sel.checked_add_signed(chrono::Duration::days(d)) {
            self.set_sel(db, x);
        }
    }

    fn nudge_month(&mut self, db: &Db, d: i64) {
        let cur = self.hist.sel.year() as i64 * 12 + (self.hist.sel.month() as i64 - 1) + d;
        if cur < 0 {
            return;
        }
        let x = self
            .hist
            .sel
            .with_day(1)
            .and_then(|x| x.with_year((cur / 12) as i32))
            .and_then(|x| x.with_month((cur % 12) as u32 + 1));
        if let Some(first) = x {
            let day = self.hist.sel.day().min(first.num_days_in_month().into());
            if let Some(x) = first.with_day(day) {
                self.set_sel(db, x);
            }
        }
    }

    fn hist_refresh(&mut self, db: &Db) {
        if let Err(e) = self.hist.refresh(db) {
            self.msg = Some(format!("error: {e:#}"));
        }
    }

    fn clear_prompt(&mut self) {
        self.prompt = None;
        self.pending_name = None;
        self.pending_start = None;
        self.pending_edit = None;
        self.pending_cat = None;
        self.pending_activity = None;
    }

    fn handle_prompt_char(&mut self, db: &Db, k: &KeyEvent) {
        let Some(kind) = self.prompt.as_ref().map(|p| p.kind) else {
            return;
        };
        if kind == PromptKind::ConfirmDelete {
            match k.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => self.do_delete(db),
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => self.clear_prompt(),
                _ => {}
            }
            return;
        }
        match k.code {
            KeyCode::Esc => self.clear_prompt(),
            KeyCode::Enter => self.commit_prompt(db),
            _ => {
                let Some(prompt) = self.prompt.as_mut() else {
                    return;
                };
                match k.code {
                    KeyCode::Backspace => {
                        prompt.text.pop();
                        prompt.complete_idx = None;
                        prompt.touched = true;
                    }
                    KeyCode::Tab
                        if prompt.kind.completes_activity()
                            || prompt.kind.completes_category() =>
                    {
                        let candidates: &[String] = if prompt.kind.completes_category() {
                            &self.cat_names
                        } else {
                            &self.names
                        };
                        let prefix = prompt.text.to_lowercase();
                        let idxs: Vec<usize> = candidates
                            .iter()
                            .enumerate()
                            .filter(|(_, n)| n.to_lowercase().starts_with(&prefix))
                            .map(|(i, _)| i)
                            .collect();
                        if !idxs.is_empty() {
                            let cur = prompt.complete_idx.unwrap_or(usize::MAX);
                            let next = idxs
                                .iter()
                                .position(|&i| i > cur)
                                .map(|p| idxs[p])
                                .unwrap_or(idxs[0]);
                            prompt.complete_idx = Some(next);
                            prompt.text.clone_from(&candidates[next]);
                        }
                    }
                    KeyCode::Char(c) if !k.modifiers.contains(KeyModifiers::CONTROL) => {
                        // First keystroke replaces the prefill, like typing over a field.
                        if !prompt.touched {
                            prompt.text.clear();
                        }
                        prompt.text.push(c);
                        prompt.touched = true;
                        prompt.complete_idx = None;
                    }
                    _ => {}
                }
            }
        }
    }

    /// Advance or finish the prompt flow on Enter.
    fn commit_prompt(&mut self, db: &Db) {
        let Some(p) = self.prompt.take() else {
            return;
        };
        match p.kind {
            PromptKind::Start => {
                let name = p.text.trim().to_string();
                if !name.is_empty() {
                    match db.start(&name) {
                        Ok(StartOutcome::Started) => self.msg = Some(format!("started: {name}")),
                        Ok(StartOutcome::StartedNew) => {
                            self.msg = Some(format!("started: {name} (new activity)"))
                        }
                        Ok(StartOutcome::AlreadyRunning) => {
                            self.msg = Some(format!("already running: {name}"))
                        }
                        Err(e) => self.msg = Some(format!("error: {e:#}")),
                    }
                    self.try_tick(db);
                }
            }
            PromptKind::AddActivity => {
                let name = p.text.trim().to_string();
                if name.is_empty() {
                    return;
                }
                self.pending_name = Some(name);
                self.prompt = Some(Prompt::new(PromptKind::AddStart, "09:00"));
            }
            PromptKind::AddStart => {
                let Some(name) = self.pending_name.clone() else {
                    self.clear_prompt();
                    return;
                };
                match parse_hm_on(self.hist.sel, &p.text) {
                    Some(s) => {
                        self.pending_start = Some(s);
                        self.prompt =
                            Some(Prompt::new(PromptKind::AddEnd, fmt_hm(s + 3_600_000)));
                    }
                    None => self.bad_time(p, PromptKind::AddStart, &name),
                }
            }
            PromptKind::AddEnd => {
                let Some(name) = self.pending_name.clone() else {
                    self.clear_prompt();
                    return;
                };
                let Some(s) = self.pending_start else {
                    self.clear_prompt();
                    return;
                };
                match parse_hm_on(self.hist.sel, &p.text) {
                    Some(e) if e > s => {
                        let dur = fmt_short(e - s);
                        match db.add_session(&name, s, e) {
                            Err(err) => self.msg = Some(format!("error: {err:#}")),
                            Ok(created) => {
                                self.msg = Some(if created {
                                    format!("added: {name} ({dur}) · new activity")
                                } else {
                                    format!("added: {name} ({dur})")
                                })
                            }
                        }
                        self.clear_prompt();
                        self.hist_refresh(db);
                    }
                    _ => self.bad_time(p, PromptKind::AddEnd, &name),
                }
            }
            PromptKind::EditActivity => {
                let Some(row) = self.pending_edit.clone() else {
                    self.clear_prompt();
                    return;
                };
                let name = p.text.trim().to_string();
                if name.is_empty() {
                    return;
                }
                self.pending_edit = Some(SessionRow { name, ..row });
                self.prompt =
                    Some(Prompt::new(PromptKind::EditStart, fmt_hm(row.started_at)));
            }
            PromptKind::EditStart => {
                let Some(row) = self.pending_edit.clone() else {
                    self.clear_prompt();
                    return;
                };
                match parse_hm_on(date_of(row.started_at), &p.text) {
                    Some(s) => {
                        self.pending_start = Some(s);
                        let e = row.ended_at.unwrap_or(s + 3_600_000);
                        self.prompt = Some(Prompt::new(PromptKind::EditEnd, fmt_hm(e)));
                    }
                    None => self.bad_time(p, PromptKind::EditStart, &row.name),
                }
            }
            PromptKind::EditEnd => {
                let Some(row) = self.pending_edit.clone() else {
                    self.clear_prompt();
                    return;
                };
                let Some(s) = self.pending_start else {
                    self.clear_prompt();
                    return;
                };
                match parse_hm_on(date_of(s), &p.text) {
                    Some(e) if e > s => {
                        let dur = fmt_short(e - s);
                        if let Err(err) = db.update_session(row.id, &row.name, s, e) {
                            self.msg = Some(format!("error: {err:#}"));
                        } else {
                            self.msg = Some(format!("updated: {} ({dur})", row.name));
                        }
                        self.clear_prompt();
                        self.hist_refresh(db);
                    }
                    _ => self.bad_time(p, PromptKind::EditEnd, &row.name),
                }
            }
            PromptKind::CreateCategory => {
                let name = p.text.trim().to_string();
                if name.is_empty() {
                    return;
                }
                match db.create_category(&name) {
                    Err(e) => self.msg = Some(format!("error: {e:#}")),
                    Ok(_) => self.msg = Some(format!("created category: {name}")),
                }
                self.clear_prompt();
                self.refresh_manage(db);
            }
            PromptKind::RenameCategory => {
                let Some((id, _)) = self.pending_cat else {
                    self.clear_prompt();
                    return;
                };
                let name = p.text.trim().to_string();
                if name.is_empty() {
                    return;
                }
                match db.rename_category(id, &name) {
                    Err(e) => self.msg = Some(format!("error: {e:#}")),
                    Ok(()) => self.msg = Some(format!("renamed category: {name}")),
                }
                self.clear_prompt();
                self.refresh_manage(db);
            }
            PromptKind::MoveActivity => {
                let Some(activity) = self.pending_activity.clone() else {
                    self.clear_prompt();
                    return;
                };
                let cat = p.text.trim();
                let cat = if cat.is_empty() {
                    None
                } else {
                    Some(cat)
                };
                match db.set_activity_category(&activity, cat) {
                    Err(e) => self.msg = Some(format!("error: {e:#}")),
                    Ok(()) => {
                        self.msg = Some(match cat {
                            Some(c) => format!("moved {activity} -> {c}"),
                            None => format!("unassigned: {activity}"),
                        });
                    }
                }
                self.clear_prompt();
                self.refresh_manage(db);
            }
            PromptKind::ConfirmDelete => match p.text.trim().to_ascii_lowercase().as_str() {
                "y" | "yes" => self.do_delete(db),
                _ => {}
            },
        }
    }

    /// Reopen the time prompt after a malformed or backwards time.
    fn bad_time(&mut self, p: Prompt, kind: PromptKind, name: &str) {
        if kind == PromptKind::AddEnd || kind == PromptKind::EditEnd {
            self.msg = Some(format!("{name}: end must be after start"));
        } else {
            self.msg = Some("time must be HH:MM".to_string());
        }
        self.prompt = Some(Prompt { kind, ..p });
    }

    fn do_delete(&mut self, db: &Db) {
        if self.mode == Mode::Manage {
            let Some((id, name)) = self.pending_cat.take() else {
                self.clear_prompt();
                return;
            };
            match db.delete_category(id) {
                Err(e) => self.msg = Some(format!("error: {e:#}")),
                Ok(()) => self.msg = Some(format!("deleted category: {name}")),
            }
            self.clear_prompt();
            self.refresh_manage(db);
            return;
        }
        let Some(row) = self.pending_edit.take() else {
            self.clear_prompt();
            return;
        };
        let end = row.ended_at.unwrap_or(now_ms());
        let label = format!("{} {}", fmt_range(row.started_at, end), row.name);
        match db.delete_session(row.id) {
            Err(e) => self.msg = Some(format!("error: {e:#}")),
            Ok(()) => self.msg = Some(format!("deleted: {label}")),
        }
        self.clear_prompt();
        self.hist_refresh(db);
    }
}

fn draw(f: &mut Frame, s: &State) {
    match s.mode {
        Mode::Live => draw_live(f, s),
        Mode::Hist => draw_hist(f, s),
        Mode::Manage => draw_manage(f, s),
        Mode::Stats => draw_stats(f, s),
        Mode::Activity => draw_activity(f, s),
    }
    if s.help {
        draw_help(f);
    }
}

/// Centered key-cheat overlay; any key closes it.
fn draw_help(f: &mut Frame) {
    let area = f.area();
    let v = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Ratio(1, 3),
            Constraint::Min(9),
            Constraint::Ratio(1, 3),
        ])
        .split(area);
    let h = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Min(1),
            Constraint::Length(58),
            Constraint::Min(1),
        ])
        .split(v[1]);
    let raw = [
        " LIVE (1)     s start · x stop · a add · t period · j/k select",
        " HISTORY (2)  j/k select · n/p day · h/l month · t today",
        "              a add session · e edit times · d delete",
        "              y year view (j/k month · h/l year · enter open)",
        " MANAGE (3)   c new cat · r rename · d del cat · m move",
        "              j/k select (m needs an activity row)",
        " STATS (4)    h/l day · j/k week · t today · enter history",
        " ACTIVITY (5) tab rail/history · j/k pick on rail · hist keys",
        " COMMON       1/2/3/4/5 modes · ? help · q quit",
        " PROMPTS      tab complete · enter save · esc cancel",
    ];
    let lines: Vec<Line> = raw
        .iter()
        .map(|l| {
            let (head, rest) = l.split_at(14);
            Line::from(vec![
                Span::styled(
                    head.to_string(),
                    Style::default()
                        .fg(theme::ACCENT)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(rest.to_string(), Style::default().fg(theme::FG)),
            ])
        })
        .collect();
    f.render_widget(
        Paragraph::new(lines).block(panel("HELP", None)),
        h[1],
    );
}

const NAME_W: usize = 14;
const TIME_W: usize = 9;

/// Rounded panel border with a left title and optional right-hand summary.
fn panel(title: &str, right: Option<String>) -> Block<'static> {
    let mut b = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded);
    b = b.title(Span::styled(
        title.to_string(),
        Style::default().fg(theme::MUTED),
    ));
    if let Some(r) = right {
        b = b.title(Title::from(
            Line::from(Span::styled(r, Style::default().fg(theme::DIM)))
                .alignment(Alignment::Right),
        ));
    }
    b
}

/// Bar portion width for a list row, given the panel's content width.
fn bar_width(content: u16) -> usize {
    (content as usize)
        .saturating_sub(1 + NAME_W + 2 + TIME_W)
        .clamp(4, 48)
}

/// Width of the left "now" rail shown while sessions run.
const RAIL_W: u16 = 16;

/// Full frame when idle, or main area + rail rect while sessions run.
fn main_area(s: &State, f: &Frame) -> (Rect, Option<Rect>) {
    let full = f.area();
    if s.running.is_empty() {
        return (full, None);
    }
    let h = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(RAIL_W), Constraint::Min(1)])
        .split(full);
    (h[1], Some(h[0]))
}

/// Left rail: running sessions with live elapsed time, display-only.
fn now_rail(s: &State) -> List<'static> {
    let now = now_ms();
    let items: Vec<ListItem> = s
        .running
        .iter()
        .flat_map(|r| {
            let name_line = Line::from(vec![
                Span::styled(
                    "●".to_string(),
                    Style::default().fg(theme::activity_color(&r.name)),
                ),
                Span::styled(
                    truncate_name(&r.name, RAIL_W as usize - 4),
                    Style::default().fg(theme::FG),
                ),
            ]);
            let elapsed_line = Line::from(Span::styled(
                format!("  {}", fmt_clock(now - r.started_at)),
                Style::default().fg(theme::MUTED),
            ));
            vec![ListItem::new(name_line), ListItem::new(elapsed_line)]
        })
        .collect();
    List::new(items).block(panel("now", None))
}

/// Truncate a name to `max` chars, marking the cut with an ellipsis.
fn truncate_name(name: &str, max: usize) -> String {
    if name.chars().count() <= max {
        name.to_string()
    } else {
        let t: String = name.chars().take(max.saturating_sub(1)).collect();
        format!("{t}…")
    }
}

/// One list row: gutter, name column, proportional bar, right-aligned duration.
fn bar_row(
    name: &str,
    ms: i64,
    max_ms: i64,
    width: usize,
    is_sel: bool,
    indent: usize,
    dot: Option<Color>,
) -> ListItem<'static> {
    let display = if name.chars().count() > NAME_W - 1 {
        let t: String = name.chars().take(NAME_W - 1).collect();
        format!("{t}…")
    } else {
        name.to_string()
    };
    let color = theme::activity_color(&display);
    let mut nstyle = Style::default().fg(color);
    if is_sel {
        nstyle = nstyle.add_modifier(Modifier::BOLD);
    }
    let gutter = if is_sel {
        Span::styled("▸".to_string(), theme::selected_style())
    } else if let Some(c) = dot {
        Span::styled("●".to_string(), Style::default().fg(c))
    } else {
        Span::raw(" ".repeat(indent))
    };
    let pad = NAME_W.saturating_sub(display.chars().count()) + 1;
    let mut spans: Vec<Span> = vec![
        gutter,
        Span::styled(display, nstyle),
        Span::raw(" ".repeat(pad)),
    ];
    spans.extend(theme::bar_spans(ms, max_ms, width, color));
    spans.push(Span::styled(
        format!("{t:>w$}", t = fmt_short(ms), w = TIME_W),
        Style::default().fg(theme::FG),
    ));
    if is_sel {
        ListItem::new(Line::from(spans)).style(Style::default().bg(theme::SELECTED_BG))
    } else {
        ListItem::new(Line::from(spans))
    }
}

/// Bottom bar: mode chips (current one lit) plus right-aligned context.
fn status_bar(s: &State, width: u16) -> Line<'static> {
    let mut spans: Vec<Span> = Vec::new();
    for (k, label, m) in [
        ("1", "live", Mode::Live),
        ("2", "hist", Mode::Hist),
        ("3", "manage", Mode::Manage),
        ("4", "stats", Mode::Stats),
        ("5", "activity", Mode::Activity),
    ] {
        let active = s.mode == m;
        let kstyle = if active {
            Style::default()
                .fg(theme::KEY)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme::DIM)
        };
        let lstyle = if active {
            Style::default()
                .fg(theme::ACCENT)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme::DIM)
        };
        spans.push(Span::styled(format!(" {k} "), kstyle));
        spans.push(Span::styled(label.to_string(), lstyle));
    }
    let context = match s.mode {
        Mode::Live => {
            let total: i64 = s.totals.iter().map(|t| t.ms).sum();
            format!("{} · {}", fmt_short(total), s.period.title())
        }
        Mode::Hist if s.hist.year_view => format!("{s}", s = s.hist.year),
        Mode::Hist => {
            let total: i64 = s.hist.day_totals.iter().map(|t| t.ms).sum();
            format!("{} · {}", fmt_day_title(s.hist.sel), fmt_short(total))
        }
        Mode::Activity if s.hist.year_view => format!("{s}", s = s.hist.year),
        Mode::Activity => {
            let total: i64 = s.hist.day_totals.iter().map(|t| t.ms).sum();
            let scope = match s.activity.rows.get(s.activity.sel) {
                Some(RailRow::Activity(n)) => n.clone(),
                _ => "all".to_string(),
            };
            format!("{scope} · {} · {}", fmt_day_title(s.hist.sel), fmt_short(total))
        }
        Mode::Manage => {
            let total: i64 = s
                .manage
                .rows
                .iter()
                .map(|r| match r {
                    ManageRow::Activity { total, .. } => *total,
                    _ => 0,
                })
                .sum();
            format!("{t} all-time", t = fmt_short(total))
        }
        Mode::Stats => {
            let total: i64 = s.stats.detail.iter().map(|t| t.ms).sum();
            format!("{} · {}", fmt_day_title(s.stats.sel), fmt_short(total))
        }
    };
    let right: Vec<Span> = vec![
        Span::styled(context, Style::default().fg(theme::MUTED)),
        theme::dim_span(" · "),
        theme::key_span("?"),
        theme::dim_span(" help · "),
        theme::key_span("q"),
        theme::dim_span(" quit"),
    ];
    let right_len: usize = right
        .iter()
        .map(|sp| sp.content.chars().count())
        .sum::<usize>()
        + 1;
    let left_len: usize = spans
        .iter()
        .map(|sp| sp.content.chars().count())
        .sum();
    let pad = (width as usize).saturating_sub(left_len).saturating_sub(right_len);
    spans.push(Span::raw(" ".repeat(pad)));
    spans.extend(right);
    Line::from(spans)
}
fn draw_live(f: &mut Frame, s: &State) {
    let area = f.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(4),
            Constraint::Length(1),
            Constraint::Min(4),
            Constraint::Length(1),
        ])
        .split(area);

    let sel = s.prompt.is_none() && s.selected < s.running.len();
    let items: Vec<ListItem> = if s.running.is_empty() {
        vec![ListItem::new(Line::from(
            theme::dim_span("nothing running - press s to start"),
        ))]
    } else {
        let now = now_ms();
        s.running
            .iter()
            .enumerate()
            .map(|(i, r)| {
                let is_sel = sel && i == s.selected;
                let mut tstyle = Style::default().fg(theme::MUTED);
                let mut nstyle = Style::default().fg(theme::activity_color(&r.name));
                if is_sel {
                    tstyle = tstyle.add_modifier(Modifier::BOLD);
                    nstyle = nstyle.add_modifier(Modifier::BOLD);
                }
                let gutter = if is_sel {
                    Span::styled("▸".to_string(), theme::selected_style())
                } else {
                    Span::raw(" ")
                };
                let dot = Span::styled(
                    "● ".to_string(),
                    Style::default().fg(theme::activity_color(&r.name)),
                );
                let line = Line::from(vec![
                    gutter,
                    Span::styled(format!("{:>11}", fmt_clock(now - r.started_at)), tstyle),
                    dot,
                    Span::styled(r.name.clone(), nstyle),
                ]);
                if is_sel {
                    ListItem::new(line).style(Style::default().bg(theme::SELECTED_BG))
                } else {
                    ListItem::new(line)
                }
            })
            .collect()
    };
    let right = if s.running.is_empty() {
        "idle".to_string()
    } else {
        format!("{} running", s.running.len())
    };
    f.render_widget(
        List::new(items).block(panel("RUNNING", Some(right))),
        chunks[0],
    );

    let total: i64 = s.totals.iter().map(|t| t.ms).sum();
    let max_ms = s.totals.iter().map(|t| t.ms).max().unwrap_or(0);
    let bw = bar_width(chunks[2].width);
    let items: Vec<ListItem> = if s.totals.is_empty() {
        vec![ListItem::new(Line::from(
            theme::dim_span("no time recorded in this period yet"),
        ))]
    } else {
        let running_names: HashSet<String> =
            s.running.iter().map(|r| r.name.to_lowercase()).collect();
        s.totals
            .iter()
            .map(|t| {
                let dot = running_names
                    .contains(&t.name.to_lowercase())
                    .then(|| theme::activity_color(&t.name));
                bar_row(&t.name, t.ms, max_ms, bw, false, 1, dot)
            })
            .collect()
    };
    let right = format!("{} · {}", fmt_short(total), s.period.title());
    f.render_widget(
        List::new(items).block(panel(s.period.title(), Some(right))),
        chunks[2],
    );

    let bar = match (&s.prompt, &s.msg) {
        (Some(p), _) => Line::from(vec![
            theme::key_span(p.kind.label()),
            Span::raw(" "),
            Span::styled(p.text.clone(), Style::default().fg(theme::FG)),
            theme::key_span("█"),
            theme::dim_span(&format!("   {}", p.kind.hint())),
        ]),
        (None, Some(m)) => Line::from(vec![
            Span::styled(m.clone(), Style::default().fg(theme::KEY)),
            theme::dim_span(" · ? help · q quit"),
        ]),
        (None, None) => status_bar(s, area.width),
    };
    f.render_widget(Paragraph::new(bar), chunks[3]);
}

fn draw_manage(f: &mut Frame, s: &State) {
    let (area, rail) = main_area(s, f);
    if let Some(r) = rail {
        f.render_widget(now_rail(s), r);
    }
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(5), Constraint::Length(1)])
        .split(area);

    let sel = s.prompt.is_none();
    let max_ms: i64 = s
        .manage
        .rows
        .iter()
        .map(|r| match r {
            ManageRow::Activity { total, .. } => *total,
            _ => 0,
        })
        .max()
        .unwrap_or(0);
    let bw = bar_width(chunks[0].width);
    let content_w = (chunks[0].width as usize).saturating_sub(2);

    let mut items: Vec<ListItem> = Vec::new();
    if s.manage.rows.is_empty() {
        items.push(ListItem::new(Line::from(
            theme::dim_span("no categories yet - press c to create one"),
        )));
    } else {
        let shown = chunks[0].height as usize - 2;
        let offset = s.manage.sel.saturating_sub(shown.saturating_sub(1));
        for (i, row) in s.manage.rows.iter().enumerate().skip(offset) {
            if i >= offset + shown {
                break;
            }
            let is_sel = sel && i == s.manage.sel;
            match row {
                ManageRow::Category { name, total, .. } => {
                    let t = fmt_short(*total);
                    let gutter = if is_sel {
                        Span::styled("▸".to_string(), theme::selected_style())
                    } else {
                        Span::raw(" ")
                    };
                    let pad = content_w
                        .saturating_sub(name.chars().count() + t.chars().count() + 1);
                    let line = Line::from(vec![
                        gutter,
                        Span::styled(
                            name.clone(),
                            Style::default().fg(theme::FG).add_modifier(Modifier::BOLD),
                        ),
                        Span::raw(" ".repeat(pad)),
                        theme::muted_span(&t),
                    ]);
                    items.push(if is_sel {
                        ListItem::new(line).style(Style::default().bg(theme::SELECTED_BG))
                    } else {
                        ListItem::new(line)
                    });
                }
                ManageRow::Activity { name, total } => {
                    items.push(bar_row(name, *total, max_ms, bw, is_sel, 2, None));
                }
            }
        }
    }
    let total: i64 = s
        .manage
        .rows
        .iter()
        .map(|r| match r {
            ManageRow::Activity { total, .. } => *total,
            _ => 0,
        })
        .sum();
    let right = format!("{} all-time", fmt_short(total));
    f.render_widget(
        List::new(items).block(panel("CATEGORIES", Some(right))),
        chunks[0],
    );

    let bar = match (&s.prompt, &s.msg) {
        (Some(p), _) if p.kind == PromptKind::ConfirmDelete => {
            let label = s
                .pending_cat
                .as_ref()
                .map(|(_, n)| n.clone())
                .unwrap_or_default();
            Line::from(vec![
                Span::styled(
                    format!(" delete category {label}?"),
                    Style::default().fg(theme::KEY),
                ),
                theme::dim_span("   y yes · n/esc no"),
            ])
        }
        (Some(p), _) => Line::from(vec![
            theme::key_span(p.kind.label()),
            Span::raw(" "),
            Span::styled(p.text.clone(), Style::default().fg(theme::FG)),
            theme::key_span("█"),
            theme::dim_span(&format!("   {}", p.kind.hint())),
        ]),
        (None, Some(m)) => Line::from(vec![
            Span::styled(m.clone(), Style::default().fg(theme::KEY)),
            theme::dim_span(" · ? help · q quit"),
        ]),
        (None, None) => status_bar(s, area.width),
    };
    f.render_widget(Paragraph::new(bar), chunks[1]);
}

fn draw_hist(f: &mut Frame, s: &State) {
    if s.hist.year_view {
        draw_hist_year(f, s);
        return;
    }
    let (area, rail) = main_area(s, f);
    if let Some(r) = rail {
        f.render_widget(now_rail(s), r);
    }
    draw_hist_main(f, s, area);
}

/// Shared history main area: month grid + day detail + status bar.
fn draw_hist_main(f: &mut Frame, s: &State, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(9),
            Constraint::Length(1),
            Constraint::Min(5),
            Constraint::Length(1),
        ])
        .split(area);

    let first = s.hist.sel.with_day(1).expect("day 1 exists");
    let dim = first.num_days_in_month() as usize;
    let lead = first.weekday().num_days_from_monday() as usize;
    let rows = (lead + dim).div_ceil(7);
    let today = Local::now().date_naive();

    let day_total: i64 = s.hist.day_totals.iter().map(|t| t.ms).sum();
    let right = format!(
        "{} · {}",
        fmt_day_title(s.hist.sel),
        fmt_short(day_total)
    );

    let mut lines: Vec<Line> = vec![Line::from(theme::dim_span(
        "Mo  Tu  We  Th  Fr  Sa  Su",
    ))];
    for w in 0..rows {
        let mut spans: Vec<Span> = Vec::new();
        for c in 0..7 {
            match (w * 7 + c)
                .checked_sub(lead)
                .and_then(|i| i.checked_add(1))
            {
                Some(day) if day <= dim => {
                    let d = first
                        .with_day(day as u32)
                        .expect("valid day of month");
                    let dms = local_midnight_ms(d);
                    let has = s.hist.month_days.contains_key(&dms);
                    let style = if d == s.hist.sel {
                        Style::default()
                            .fg(theme::ACCENT)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        let st = if has {
                            Style::default().fg(theme::FG)
                        } else {
                            Style::default().fg(theme::MUTED)
                        };
                        if d == today {
                            st.add_modifier(Modifier::UNDERLINED)
                        } else {
                            st
                        }
                    };
                    spans.push(Span::styled(
                        format!(" {:>2} {}", d.day(), if has { "·" } else { " " }),
                        style,
                    ));
                }
                _ => spans.push(Span::raw("     ")),
            }
        }
        lines.push(Line::from(spans));
    }
    let title = match &s.hist.scope {
        Scope::All => format!("HISTORY · {}", fmt_month(s.hist.sel)),
        Scope::Activity(name) => format!("{name} · {}", fmt_month(s.hist.sel)),
    };
    f.render_widget(
        Paragraph::new(lines).block(panel(&title, Some(right))),
        chunks[0],
    );

    let max_ms = s
        .hist
        .day_totals
        .iter()
        .map(|t| t.ms)
        .max()
        .unwrap_or(0);
    let bw = bar_width(chunks[2].width);
    let mut items: Vec<ListItem> = Vec::new();
    if s.hist.sessions.is_empty() {
        items.push(ListItem::new(Line::from(
            theme::dim_span("no time recorded on this day - press a to add"),
        )));
    } else {
        let header_rows = s.hist.day_totals.len() + 1;
        let shown = (chunks[2].height as usize)
            .saturating_sub(2)
            .saturating_sub(header_rows);
        let offset = s.sess_sel.saturating_sub(shown.saturating_sub(1));
        let end_idx = s.hist.sessions.len().min(offset + shown);
        for t in &s.hist.day_totals {
            items.push(bar_row(&t.name, t.ms, max_ms, bw, false, 1, None));
        }
        items.push(ListItem::new(Line::default()));
        let now = now_ms();
        for (i, sess) in s.hist.sessions[offset..end_idx].iter().enumerate() {
            let is_sel = offset + i == s.sess_sel;
            let end = sess.ended_at.unwrap_or(now);
            let range_style = if is_sel {
                Style::default().fg(theme::ACCENT).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme::MUTED)
            };
            let name_style = if is_sel {
                Style::default()
                    .fg(theme::activity_color(&sess.name))
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme::activity_color(&sess.name))
            };
            let dur_style = if is_sel {
                Style::default().fg(theme::FG)
            } else {
                Style::default().fg(theme::DIM)
            };
            let line = Line::from(vec![
                Span::styled(format!("{:>17}", fmt_range(sess.started_at, end)), range_style),
                Span::raw("  "),
                Span::styled(sess.name.clone(), name_style),
                Span::styled(
                    format!("   {}", fmt_short(end - sess.started_at)),
                    dur_style,
                ),
            ]);
            items.push(if is_sel {
                ListItem::new(line).style(Style::default().bg(theme::SELECTED_BG))
            } else {
                ListItem::new(line)
            });
        }
    }
    f.render_widget(
        List::new(items).block(panel(
            &fmt_day_title(s.hist.sel),
            Some(fmt_short(day_total)),
        )),
        chunks[2],
    );

    let bar = match (&s.prompt, &s.msg) {
        (Some(p), _) if p.kind == PromptKind::ConfirmDelete => {
            let label = s
                .pending_edit
                .as_ref()
                .map(|row| {
                    let end = row.ended_at.unwrap_or_else(now_ms);
                    format!("{} {}", fmt_range(row.started_at, end), row.name)
                })
                .unwrap_or_default();
            Line::from(vec![
                Span::styled(
                    format!(" delete {label}?"),
                    Style::default().fg(theme::KEY),
                ),
                theme::dim_span("   y yes · n/esc no"),
            ])
        }
        (Some(p), _) => Line::from(vec![
            theme::key_span(p.kind.label()),
            Span::raw(" "),
            Span::styled(p.text.clone(), Style::default().fg(theme::FG)),
            theme::key_span("█"),
            theme::dim_span(&format!("   {}", p.kind.hint())),
        ]),
        (None, Some(m)) => Line::from(vec![
            Span::styled(m.clone(), Style::default().fg(theme::KEY)),
            theme::dim_span(" · ? help · q quit"),
        ]),
        (None, None) => status_bar(s, area.width),
    };
    f.render_widget(Paragraph::new(bar), chunks[3]);
}


fn draw_activity(f: &mut Frame, s: &State) {
    if s.hist.year_view {
        draw_hist_year(f, s);
        return;
    }
    let h = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(RAIL_W), Constraint::Min(1)])
        .split(f.area());
    f.render_widget(activity_rail(s, h[0].height), h[0]);
    draw_hist_main(f, s, h[1]);
}

/// Left rail for activity mode: all + category headers + activities, with
/// a colored dot next to running activities.
fn activity_rail(s: &State, height: u16) -> List<'static> {
    let running: std::collections::HashSet<String> =
        s.running.iter().map(|r| r.name.to_lowercase()).collect();
    let items: Vec<ListItem> = s
        .activity
        .rows
        .iter()
        .enumerate()
        .map(|(i, row)| {
            let line = match row {
                RailRow::All => Line::from(Span::styled(
                    "all activities".to_string(),
                    Style::default().fg(theme::FG),
                )),
                RailRow::Header(h) => Line::from(theme::dim_span(h)),
                RailRow::Activity(n) => {
                    let mut spans: Vec<Span> = Vec::new();
                    if running.contains(&n.to_lowercase()) {
                        spans.push(Span::styled(
                            "●".to_string(),
                            Style::default().fg(theme::activity_color(n)),
                        ));
                    }
                    spans.push(Span::styled(
                        format!(" {}", truncate_name(n, 12)),
                        Style::default().fg(theme::FG),
                    ));
                    Line::from(spans)
                }
            };
            let item = ListItem::new(line);
            if i == s.activity.sel && s.activity.focus_rail {
                item.style(Style::default().bg(theme::SELECTED_BG))
            } else {
                item
            }
        })
        .collect();
    let shown = (height as usize).saturating_sub(2);
    let offset = (s.activity.sel + 1).saturating_sub(shown);
    let items: Vec<ListItem> = items.into_iter().skip(offset).collect();
    List::new(items).block(panel("ACTIVITY", None))
}
/// Year-at-a-glance: one row per month with a scaled activity bar.
fn draw_hist_year(f: &mut Frame, s: &State) {
    let (area, rail) = main_area(s, f);
    if let Some(r) = rail {
        f.render_widget(now_rail(s), r);
    }
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(14), Constraint::Length(1)])
        .split(area);

    let year = s.hist.year;
    let mut months: Vec<(i32, i64, i64)> = (1..=12).map(|m| (m, 0_i64, 0_i64)).collect();
    for (d, ms) in &s.hist.year_days {
        if d.year() == year {
            let i = d.month0() as usize;
            months[i].1 += ms;
            if *ms > 0 {
                months[i].2 += 1;
            }
        }
    }
    let max_ms = months.iter().map(|(_, ms, _)| *ms).max().unwrap_or(0);
    const BAR: i64 = 16;

    let sel_month = s.hist.sel.month0() as i32 + 1;
    let mut lines: Vec<Line> = Vec::new();
    for (m, ms, days) in &months {
        let mut spans: Vec<Span> = Vec::new();
        let label_style = if *m == sel_month {
            Style::default()
                .fg(theme::ACCENT)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme::MUTED)
        };
        spans.push(Span::styled(format!("{m:>2} "), label_style));
        spans.extend(theme::bar_spans(*ms, max_ms, BAR as usize, Color::Green));
        spans.push(Span::styled(fmt_short(*ms), Style::default().fg(theme::FG)));
        spans.push(Span::styled(
            format!("  {days}d"),
            Style::default().fg(theme::DIM),
        ));
        lines.push(Line::from(spans));
    }
    f.render_widget(
        Paragraph::new(lines).block(panel(
            &format!("HISTORY · {year} · year view"),
            Some("y".to_string()),
        )),
        chunks[0],
    );
    f.render_widget(Paragraph::new(status_bar(s, area.width)), chunks[1]);
}

/// 365-day heatmap (trailing year) with the selected day's detail below.
fn draw_stats(f: &mut Frame, s: &State) {
    let (area, rail) = main_area(s, f);
    if let Some(r) = rail {
        f.render_widget(now_rail(s), r);
    }
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(12),
            Constraint::Length(1),
            Constraint::Min(4),
            Constraint::Length(1),
        ])
        .split(area);

    let today = Local::now().date_naive();
    let start = today - chrono::Duration::days(363);
    let grid_monday =
        start - chrono::Duration::days(start.weekday().num_days_from_monday() as i64);
    let map: HashMap<String, i64> = s
        .stats
        .days
        .iter()
        .map(|(d, ms)| (d.format("%Y-%m-%d").to_string(), *ms))
        .collect();

    // Month label row: stamp each month's name where it begins in the grid.
    let mut labels: Vec<char> = vec![' '; 53];
    let mut prev_month: Option<u32> = None;
    for c in 0..53i64 {
        let mon = grid_monday + chrono::Duration::days(7 * c);
        if mon > today {
            break;
        }
        if prev_month == Some(mon.month()) {
            continue;
        }
        let name = mon.format("%b").to_string().to_uppercase();
        // Skip the label rather than garble it when a neighbour owns the cells.
        let fits = name
            .char_indices()
            .all(|(i, _)| c as usize + i < 53 && labels[c as usize + i] == ' ');
        if fits {
            for (i, ch) in name.chars().enumerate() {
                labels[c as usize + i] = ch;
            }
        }
        prev_month = Some(mon.month());
    }

    let total: i64 = s.stats.detail.iter().map(|t| t.ms).sum();
    let right = format!(
        "{} · {}",
        fmt_day_title(s.stats.sel),
        fmt_short(total)
    );

    let mut lines: Vec<Line> =
        vec![Line::from(theme::dim_span(&labels.iter().collect::<String>()))];
    for r in 0..7i64 {
        let mut spans: Vec<Span> = Vec::new();
        for c in 0..53i64 {
            let day = grid_monday + chrono::Duration::days(7 * c + r);
            if day < start || day > today {
                spans.push(Span::raw(" "));
                continue;
            }
            let key = day.format("%Y-%m-%d").to_string();
            let ms = *map.get(&key).unwrap_or(&0);
            let lvl = heat_level(ms);
            let mut style = if day == s.stats.sel {
                Style::default()
                    .fg(theme::ACCENT)
                    .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
            } else if lvl == 0 {
                Style::default().fg(theme::DIM)
            } else {
                Style::default().bg(HEAT_COLORS[lvl as usize])
            };
            if day == today && day != s.stats.sel {
                style = style.add_modifier(Modifier::UNDERLINED);
            }
            spans.push(Span::styled(
                if lvl == 0 { "·" } else { "■" }.to_string(),
                style,
            ));
        }
        lines.push(Line::from(spans));
    }
    let mut legend: Vec<Span> = vec![theme::dim_span(" less  ")];
    for lvl in 0..5u8 {
        legend.push(if lvl == 0 {
            Span::styled("·".to_string(), Style::default().fg(theme::DIM))
        } else {
            Span::styled(
                "■".to_string(),
                Style::default().bg(HEAT_COLORS[lvl as usize]),
            )
        });
    }
    legend.push(theme::dim_span("  more"));
    lines.push(Line::from(legend));
    f.render_widget(
        Paragraph::new(lines).block(panel("STATS · trailing 365 days", Some(right))),
        chunks[0],
    );

    let content_w = (chunks[2].width as usize).saturating_sub(2);
    let mut dl: Vec<Line> = Vec::new();
    if s.stats.detail.is_empty() {
        dl.push(Line::from(theme::dim_span("no time recorded on this day")));
    } else {
        for t in &s.stats.detail {
            let pad = content_w
                .saturating_sub(t.name.chars().count() + fmt_short(t.ms).chars().count() + 1);
            dl.push(Line::from(vec![
                Span::styled(
                    t.name.clone(),
                    Style::default().fg(theme::activity_color(&t.name)),
                ),
                Span::raw(" ".repeat(pad)),
                theme::muted_span(&fmt_short(t.ms)),
            ]));
        }
    }
    f.render_widget(
        Paragraph::new(dl).block(panel("SELECTED DAY", Some(fmt_short(total)))),
        chunks[2],
    );

    f.render_widget(Paragraph::new(status_bar(s, area.width)), chunks[3]);
}

pub fn run() -> Result<()> {
    let db = Db::open()?;
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let mut term = Terminal::new(CrosstermBackend::new(stdout))?;
    let mut state = State::new(&db)?;
    let result: Result<()> = loop {
        // Keep the current view live: refresh once per second.
        let second = now_ms() / 1000;
        if second != state.last_second {
            state.last_second = second;
            state.try_tick(&db);
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
                    } else if state.help {
                        state.help = false;
                    } else {
                        match state.mode {
                            Mode::Live => state.handle_live_key(&db, &k),
                            Mode::Hist => state.handle_hist_key(&db, &k),
                            Mode::Manage => state.handle_manage_key(&db, &k),
                            Mode::Stats => state.handle_stats_key(&db, &k),
                            Mode::Activity => state.handle_activity_key(&db, &k),
                        }
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
