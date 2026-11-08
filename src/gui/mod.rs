//! Desktop window: live timers, start/stop and per-period totals (Live),
//! a drag-to-edit timeline of one day's sessions (Day), and a year of
//! history as a heatmap, months and a breakdown (Stats).
//!
//! Reads the same SQLite file as the CLI and TUI and re-queries it every
//! second, so `enoch start` in a shell shows up here without a restart.

mod day;
mod stats;
mod style;
mod timeline;

use std::time::{Duration, Instant};

use anyhow::Result;
use chrono::{Local, NaiveDate};
use eframe::egui::{self, Align, Layout, RichText, Sense, Ui};
use enoch::db::{Db, Running, Scope, StartOutcome, Total};
use enoch::timefmt::{
    date_of, day_bounds, fmt_clock, fmt_hm, fmt_range, fmt_short, now_ms, week_bounds,
};

/// How long a status-bar message stays up.
const FLASH_FOR: Duration = Duration::from_secs(6);

pub fn run(db: Db) -> Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("enoch")
            .with_app_id("enoch")
            .with_inner_size([960.0, 640.0])
            .with_min_inner_size([560.0, 360.0]),
        ..Default::default()
    };
    eframe::run_native(
        "enoch",
        options,
        Box::new(move |cc| {
            style::apply(&cc.egui_ctx);
            Ok(Box::new(App::new(db)))
        }),
    )
    .map_err(|e| anyhow::anyhow!("gui: {e}"))
}

#[derive(Clone, Copy, PartialEq)]
enum Period {
    Today,
    Week,
    AllTime,
}

impl Period {
    const ALL: [Period; 3] = [Period::Today, Period::Week, Period::AllTime];

    fn label(self) -> &'static str {
        match self {
            Period::Today => "Today",
            Period::Week => "This week",
            Period::AllTime => "All time",
        }
    }

    fn bounds(self) -> (i64, i64) {
        let d = Local::now().date_naive();
        match self {
            Period::Today => day_bounds(d),
            Period::Week => week_bounds(d),
            Period::AllTime => (0, i64::MAX),
        }
    }
}

/// What the window shows, re-read from the database on every refresh.
#[derive(Default)]
struct Snapshot {
    running: Vec<Running>,
    totals: Vec<Total>,
    /// Every activity with its category; the no-category group comes first.
    rail: Vec<(String, Option<String>)>,
}

impl Snapshot {
    fn load(db: &Db, period: Period) -> Result<Self> {
        let (from, to) = period.bounds();
        Ok(Self {
            running: db.running()?,
            totals: db.totals(from, to, &Scope::All)?,
            rail: db.activity_index()?,
        })
    }

    fn started_at(&self, name: &str) -> Option<i64> {
        self.running
            .iter()
            .find(|r| r.name.eq_ignore_ascii_case(name))
            .map(|r| r.started_at)
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Tab {
    Live,
    Day,
    Stats,
}

/// Things the UI asked for this frame, applied once drawing is done.
enum Action {
    Start(String),
    /// Stop these activities; empty stops everything.
    Stop(Vec<String>),
    Period(Period),
    AddSession {
        name: String,
        start: i64,
        end: i64,
    },
    UpdateSession {
        id: i64,
        name: String,
        start: i64,
        end: i64,
    },
    DeleteSession(i64),
    /// Show this day in the Day tab.
    OpenDay(NaiveDate),
}

struct Flash {
    text: String,
    error: bool,
    at: Instant,
}

struct App {
    db: Db,
    tab: Tab,
    day: day::DayView,
    stats: stats::StatsView,
    period: Period,
    snap: Snapshot,
    /// When `snap` was loaded; `None` forces a reload next frame.
    loaded_at: Option<Instant>,
    start_text: String,
    focus_start: bool,
    flash: Option<Flash>,
}

impl App {
    fn new(db: Db) -> Self {
        Self {
            db,
            tab: Tab::Live,
            day: day::DayView::new(),
            stats: stats::StatsView::new(),
            period: Period::Today,
            snap: Snapshot::default(),
            loaded_at: None,
            start_text: String::new(),
            focus_start: true,
            flash: None,
        }
    }

    /// Reload from the database at most once a second (or right after an
    /// action), so changes made by the CLI appear promptly.
    fn refresh_if_due(&mut self) {
        let due = self
            .loaded_at
            .is_none_or(|t| t.elapsed() >= Duration::from_secs(1));
        if !due {
            return;
        }
        match Snapshot::load(&self.db, self.period) {
            Ok(s) => self.snap = s,
            Err(e) => self.error(e),
        }
        if self.tab == Tab::Day {
            let (from, to) = day_bounds(self.day.day);
            match self.db.sessions_in(from, to, &Scope::All) {
                Ok(s) => self.day.sessions = s,
                Err(e) => self.error(e),
            }
        }
        if self.tab == Tab::Stats
            && let Err(e) = self.stats.load(&self.db)
        {
            self.error(e);
        }
        self.loaded_at = Some(Instant::now());
    }

    fn info(&mut self, text: impl Into<String>) {
        self.flash = Some(Flash {
            text: text.into(),
            error: false,
            at: Instant::now(),
        });
    }

    fn error(&mut self, e: anyhow::Error) {
        self.flash = Some(Flash {
            text: format!("error: {e:#}"),
            error: true,
            at: Instant::now(),
        });
    }

    fn apply(&mut self, action: Action) {
        match action {
            Action::Start(name) => {
                let name = name.trim();
                if name.is_empty() {
                    return;
                }
                match self.db.start(name) {
                    Ok(StartOutcome::Started) => self.info(format!("started {name}")),
                    Ok(StartOutcome::StartedNew) => {
                        self.info(format!("started {name} (new activity)"))
                    }
                    Ok(StartOutcome::AlreadyRunning) => {
                        self.info(format!("{name} is already running"))
                    }
                    Err(e) => self.error(e),
                }
            }
            Action::Stop(names) => match self.db.stop(&names) {
                Ok(stopped) if stopped.is_empty() => self.info("nothing to stop"),
                Ok(stopped) => {
                    let parts: Vec<String> = stopped
                        .iter()
                        .map(|s| format!("{} ({})", s.name, fmt_short(s.ended_at - s.started_at)))
                        .collect();
                    self.info(format!("stopped {}", parts.join(", ")));
                }
                Err(e) => self.error(e),
            },
            Action::Period(p) => self.period = p,
            Action::AddSession { name, start, end } => {
                match self.db.add_session(&name, start, end) {
                    Ok(_) => self.info(format!("added {name} {}", fmt_range(start, end))),
                    Err(e) => self.error(e),
                }
            }
            Action::UpdateSession {
                id,
                name,
                start,
                end,
            } => match self.db.update_session(id, &name, start, end) {
                Ok(()) => self.info(format!("{name} now {}", fmt_range(start, end))),
                Err(e) => self.error(e),
            },
            Action::DeleteSession(id) => match self.db.delete_session(id) {
                Ok(()) => self.info("deleted session"),
                Err(e) => self.error(e),
            },
            Action::OpenDay(d) => {
                self.day.set_day(d);
                self.tab = Tab::Day;
            }
        }
        self.loaded_at = None;
    }

    fn status_bar(&self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            match &self.flash {
                Some(f) if f.at.elapsed() < FLASH_FOR => {
                    let color = if f.error {
                        style::ERROR
                    } else {
                        ui.visuals().text_color()
                    };
                    ui.label(RichText::new(&f.text).color(color));
                }
                _ => {
                    ui.weak(match self.tab {
                        Tab::Live => "type an activity and press Enter to start it",
                        Tab::Day => "Left / Right: previous / next day · T: today",
                        Tab::Stats => {
                            "click a day to see it · arrows move · Enter or double-click opens it in Day"
                        }
                    });
                }
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let n = self.snap.running.len();
                ui.weak(match n {
                    0 => "idle".to_string(),
                    1 => "1 running".to_string(),
                    n => format!("{n} running"),
                });
            });
        });
    }

    fn start_box(&mut self, ui: &mut Ui, actions: &mut Vec<Action>) {
        style::section(ui, "START");
        ui.horizontal(|ui| {
            let width = (ui.available_width() - 90.0).max(160.0);
            let resp = ui.add(
                egui::TextEdit::singleline(&mut self.start_text)
                    .hint_text("what are you doing?")
                    .desired_width(width),
            );
            if self.focus_start {
                resp.request_focus();
                self.focus_start = false;
            }
            let entered = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            let can_start = !self.start_text.trim().is_empty();
            let clicked = ui
                .add_enabled(can_start, egui::Button::new("Start"))
                .clicked();
            if (entered || clicked) && can_start {
                actions.push(Action::Start(std::mem::take(&mut self.start_text)));
                self.focus_start = true;
            }
        });

        // Existing activities matching what is typed, as one-click starts.
        let q = self.start_text.trim().to_lowercase();
        if q.is_empty() {
            return;
        }
        let matches: Vec<&String> = self
            .snap
            .rail
            .iter()
            .map(|(n, _)| n)
            .filter(|n| n.to_lowercase().contains(&q))
            .take(8)
            .collect();
        if matches.is_empty() {
            ui.weak("new activity");
            return;
        }
        ui.horizontal_wrapped(|ui| {
            for name in matches {
                let running = self.snap.started_at(name).is_some();
                let resp = ui.add_enabled(!running, egui::Button::new(name.as_str()));
                if resp.clicked() {
                    actions.push(Action::Start(name.clone()));
                    self.start_text.clear();
                    self.focus_start = true;
                }
            }
        });
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        self.refresh_if_due();
        let now = now_ms();
        let mut actions = Vec::new();

        egui::Panel::top("tabs").show(ui, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("enoch").strong().color(style::ACCENT));
                ui.add_space(12.0);
                for (tab, label, key) in [
                    (Tab::Live, "Live", egui::Key::Num1),
                    (Tab::Day, "Day", egui::Key::Num2),
                    (Tab::Stats, "Stats", egui::Key::Num3),
                ] {
                    // Ctrl on Linux, Cmd on macOS; the tooltip says which.
                    let keys = egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, key);
                    let shortcut = ui.input_mut(|i| i.consume_shortcut(&keys));
                    let clicked = ui
                        .selectable_label(self.tab == tab, label)
                        .on_hover_text(ui.ctx().format_shortcut(&keys))
                        .clicked();
                    if (clicked || shortcut) && self.tab != tab {
                        self.tab = tab;
                        self.loaded_at = None;
                        self.focus_start = tab == Tab::Live;
                    }
                }
            });
            ui.add_space(2.0);
        });
        egui::Panel::bottom("status").show(ui, |ui| {
            ui.add_space(2.0);
            self.status_bar(ui);
        });
        egui::Panel::left("rail")
            .resizable(true)
            .default_size(230.0)
            .min_size(160.0)
            .show(ui, |ui| rail(ui, &self.snap, now, &mut actions));
        let shown_day = self.day.day;
        let shown_year = self.stats.year;
        egui::CentralPanel::default().show(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.add_space(4.0);
                match self.tab {
                    Tab::Live => {
                        self.start_box(ui, &mut actions);
                        ui.add_space(18.0);
                        running(ui, &self.snap, now, &mut actions);
                        ui.add_space(18.0);
                        totals(ui, &self.snap, self.period, &mut actions);
                    }
                    Tab::Day => {
                        let names: Vec<String> =
                            self.snap.rail.iter().map(|(n, _)| n.clone()).collect();
                        day::show(ui, &mut self.day, &names, now, &mut actions);
                    }
                    Tab::Stats => stats::show(ui, &mut self.stats, &self.snap.rail, &mut actions),
                }
            });
        });
        if self.day.day != shown_day || self.stats.year != shown_year {
            self.loaded_at = None;
        }

        for a in actions {
            self.apply(a);
        }
        if self.loaded_at.is_none() {
            ui.ctx().request_repaint();
        }
        // Wake on the next whole second so every clock ticks together.
        let wait = 1000 - now_ms().rem_euclid(1000);
        ui.ctx()
            .request_repaint_after(Duration::from_millis(wait as u64 + 5));
    }
}

/// Left rail: every activity grouped by category, with start/stop buttons.
fn rail(ui: &mut Ui, snap: &Snapshot, now: i64, actions: &mut Vec<Action>) {
    ui.add_space(8.0);
    style::section(ui, "ACTIVITIES");
    if snap.rail.is_empty() {
        ui.weak("none yet");
        return;
    }
    egui::ScrollArea::vertical().show(ui, |ui| {
        let mut group: Option<&Option<String>> = None;
        for (name, cat) in &snap.rail {
            if group != Some(cat) {
                ui.add_space(6.0);
                let label = cat.as_deref().unwrap_or("no category");
                ui.label(
                    RichText::new(label)
                        .small()
                        .color(ui.visuals().weak_text_color()),
                );
                group = Some(cat);
            }
            rail_row(ui, name, snap.started_at(name), now, actions);
        }
    });
}

fn rail_row(ui: &mut Ui, name: &str, started: Option<i64>, now: i64, actions: &mut Vec<Action>) {
    let color = style::activity(name);
    // A fixed-height row: a bare right-to-left layout would claim all the
    // remaining height and center itself in it.
    let size = egui::vec2(ui.available_width(), 24.0);
    ui.allocate_ui_with_layout(size, Layout::right_to_left(Align::Center), |ui| {
        match started {
            Some(s) => {
                if style::icon_button(ui, style::Icon::Stop, &format!("stop {name}")).clicked() {
                    actions.push(Action::Stop(vec![name.to_string()]));
                }
                ui.label(RichText::new(fmt_short(now - s)).monospace().color(color));
            }
            None => {
                if style::icon_button(ui, style::Icon::Play, &format!("start {name}")).clicked() {
                    actions.push(Action::Start(name.to_string()));
                }
            }
        }
        // Dot and name take whatever width is left, truncating long names.
        ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
            let (r, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), Sense::hover());
            if started.is_some() {
                ui.painter().circle_filled(r.center(), 4.5, color);
            } else {
                ui.painter()
                    .circle_stroke(r.center(), 3.5, egui::Stroke::new(1.5, color));
            }
            ui.add(egui::Label::new(name).truncate());
        });
    });
}

/// One card per running activity with its live clock and a stop button.
fn running(ui: &mut Ui, snap: &Snapshot, now: i64, actions: &mut Vec<Action>) {
    style::section(ui, "RUNNING");
    if snap.running.is_empty() {
        ui.weak("Nothing running.");
        return;
    }
    for r in &snap.running {
        let color = style::activity(&r.name);
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let (stripe, _) = ui.allocate_exact_size(egui::vec2(4.0, 38.0), Sense::hover());
                ui.painter().rect_filled(stripe, 2.0, color);
                ui.vertical(|ui| {
                    ui.label(RichText::new(&r.name).strong().size(16.0));
                    ui.weak(format!("since {}", since(r.started_at)));
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.button("Stop").clicked() {
                        actions.push(Action::Stop(vec![r.name.clone()]));
                    }
                    ui.add_space(8.0);
                    ui.label(
                        RichText::new(fmt_clock(now - r.started_at))
                            .monospace()
                            .size(22.0),
                    );
                });
            });
        });
    }
    if snap.running.len() > 1 {
        ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
            if ui.button("Stop all").clicked() {
                actions.push(Action::Stop(Vec::new()));
            }
        });
    }
}

/// `09:12`, or `6 Nov 21:40` when the session started on an earlier day.
fn since(ms: i64) -> String {
    let d = date_of(ms);
    if d == Local::now().date_naive() {
        fmt_hm(ms)
    } else {
        format!("{} {}", d.format("%-d %b"), fmt_hm(ms))
    }
}

/// Per-activity bars for the chosen period.
fn totals(ui: &mut Ui, snap: &Snapshot, period: Period, actions: &mut Vec<Action>) {
    ui.horizontal(|ui| {
        style::section(ui, "TOTALS");
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            for p in Period::ALL.iter().rev() {
                if ui.selectable_label(*p == period, p.label()).clicked() {
                    actions.push(Action::Period(*p));
                }
            }
        });
    });
    if snap.totals.is_empty() {
        ui.weak("No time recorded yet.");
        return;
    }
    let max = snap.totals.iter().map(|t| t.ms).max().unwrap_or(1).max(1);
    let sum: i64 = snap.totals.iter().map(|t| t.ms).sum();
    let bar_w = (ui.available_width() * 0.55).max(80.0);
    egui::Grid::new("totals")
        .num_columns(3)
        .spacing([14.0, 8.0])
        .show(ui, |ui| {
            for t in &snap.totals {
                ui.label(&t.name);
                let (rect, resp) = ui.allocate_exact_size(egui::vec2(bar_w, 10.0), Sense::hover());
                style::bar(ui, rect, t.ms as f32 / max as f32, style::activity(&t.name));
                resp.on_hover_text(format!(
                    "{:.0}% of {}",
                    100.0 * t.ms as f64 / sum.max(1) as f64,
                    fmt_short(sum)
                ));
                ui.label(RichText::new(fmt_short(t.ms)).monospace());
                ui.end_row();
            }
            ui.label(RichText::new("total").weak());
            ui.label("");
            ui.label(RichText::new(fmt_short(sum)).monospace().strong());
            ui.end_row();
        });
}
