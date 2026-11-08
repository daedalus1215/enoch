//! Stats tab: a year of days as a heatmap, with totals and streaks, time
//! per month, and a breakdown by category and activity.
//!
//! - click a day to see what was done that day; double-click it (or press
//!   Enter) to open it in the Day tab
//! - arrow keys move the selected day, T selects today
//! - the menus at the top pick what counts (everything, a category or one
//!   activity) and which year; clicking a row in the breakdown does too

use std::collections::{BTreeMap, HashMap};

use anyhow::Result;
use chrono::{Datelike, Duration, Local, NaiveDate};
use eframe::egui::{
    self, Align, Align2, Color32, CursorIcon, FontId, Layout, Pos2, Rect, RichText, Sense, Stroke,
    Ui,
};
use enoch::db::{Db, Scope, Total};
use enoch::stats::{self, Ledger, Summary, Year};
use enoch::timefmt::{date_of, day_bounds, fmt_hours, now_ms};

use super::{Action, style};

/// Heatmap and chart color when counting more than one activity; a single
/// activity uses its own color.
const HEAT: Color32 = Color32::from_rgb(72, 187, 120);

/// How strongly heat levels 1–3 show `HEAT` over an empty cell; level 4
/// goes past it.
const SHADES: [f32; 4] = [0.0, 0.3, 0.6, 1.0];

/// Width of the weekday labels left of the heatmap.
const WEEKDAY_W: f32 = 30.0;
/// Height of the month labels above it.
const MONTH_H: f32 = 16.0;

/// What the tab counts.
#[derive(Clone, Debug, PartialEq)]
pub enum Filter {
    All,
    /// The activities in a category; `None` is those without one.
    Category(Option<String>),
    Activity(String),
}

impl Filter {
    fn label(&self) -> &str {
        match self {
            Filter::All => "All activities",
            Filter::Category(Some(c)) => c,
            Filter::Category(None) => "No category",
            Filter::Activity(a) => a,
        }
    }
}

pub struct StatsView {
    pub year: Year,
    filter: Filter,
    sel: NaiveDate,
    /// The year's time per day, reloaded with the rest of the window.
    ledger: Ledger,
    /// The day of the first session ever, for the list of years.
    first_day: Option<NaiveDate>,
}

impl StatsView {
    pub fn new() -> Self {
        Self {
            year: Year::Last12,
            filter: Filter::All,
            sel: Local::now().date_naive(),
            ledger: Ledger::default(),
            first_day: None,
        }
    }

    /// First and last day shown, both inclusive.
    fn days(&self) -> (NaiveDate, NaiveDate) {
        self.year.days(Local::now().date_naive())
    }

    pub fn load(&mut self, db: &Db) -> Result<()> {
        let (first, last) = self.days();
        let (from, _) = day_bounds(first);
        let (_, to) = day_bounds(last);
        let sessions = db.sessions_in(from, to, &Scope::All)?;
        self.ledger = Ledger::new(&sessions, first, last, now_ms());
        self.first_day = db.first_started_at()?.map(date_of);
        Ok(())
    }

    fn set_year(&mut self, year: Year) {
        self.year = year;
        let (first, last) = self.days();
        if !(first..=last).contains(&self.sel) {
            self.sel = last;
        }
    }

    fn select(&mut self, d: NaiveDate) {
        let (first, last) = self.days();
        self.sel = d.clamp(first, last);
    }
}

pub fn show(
    ui: &mut Ui,
    view: &mut StatsView,
    rail: &[(String, Option<String>)],
    actions: &mut Vec<Action>,
) {
    // Forget a filter whose activity or category no longer exists.
    let exists = match &view.filter {
        Filter::All => true,
        Filter::Category(c) => rail.iter().any(|(_, rc)| rc == c),
        Filter::Activity(a) => rail.iter().any(|(n, _)| n == a),
    };
    if !exists {
        view.filter = Filter::All;
    }
    let today = Local::now().date_naive();
    view.select(view.sel);
    keys(ui, view, today, actions);
    controls(ui, view, rail, today);

    let (first, last) = view.days();
    let cats: HashMap<&str, Option<&str>> = rail
        .iter()
        .map(|(n, c)| (n.as_str(), c.as_deref()))
        .collect();
    let filter = view.filter.clone();
    let keep = |n: &str| match &filter {
        Filter::All => true,
        Filter::Category(c) => cats.get(n).copied().flatten() == c.as_deref(),
        Filter::Activity(a) => n == a,
    };
    let base = match &filter {
        Filter::Activity(a) => style::activity(a),
        _ => HEAT,
    };
    let per_day = view.ledger.per_day(keep);
    let summary = stats::summarize(&per_day, today);

    ui.add_space(10.0);
    tiles(ui, &summary, (first, last), today);

    ui.add_space(16.0);
    style::section(ui, "DAYS");
    heatmap(ui, view, &per_day, (first, last), today, base, actions);
    legend(ui, base);

    ui.add_space(12.0);
    selected_day(ui, view.sel, &view.ledger.on(view.sel, keep), actions);

    ui.add_space(18.0);
    style::section(ui, "BY MONTH");
    months_chart(ui, &stats::months(&per_day, first, last), view.sel, base);

    ui.add_space(18.0);
    let has_categories = rail.iter().any(|(_, c)| c.is_some());
    style::section(
        ui,
        if has_categories {
            "BY CATEGORY"
        } else {
            "BY ACTIVITY"
        },
    );
    let totals = view.ledger.totals(keep);
    if let Some(f) = breakdown(ui, &totals, &cats, has_categories, &filter) {
        view.filter = f;
    }
}

fn keys(ui: &Ui, view: &mut StatsView, today: NaiveDate, actions: &mut Vec<Action>) {
    if ui.ctx().egui_wants_keyboard_input() {
        return;
    }
    let (left, right, up, down, t, enter) = ui.input(|i| {
        use egui::Key;
        (
            i.key_pressed(Key::ArrowLeft),
            i.key_pressed(Key::ArrowRight),
            i.key_pressed(Key::ArrowUp),
            i.key_pressed(Key::ArrowDown),
            i.key_pressed(Key::T),
            i.key_pressed(Key::Enter),
        )
    });
    // Spatial, as on the grid: weeks are columns, weekdays are rows.
    let step = 7 * (right as i64 - left as i64) + (down as i64 - up as i64);
    if step != 0 {
        view.select(view.sel + Duration::days(step));
    }
    if t {
        let (first, last) = view.days();
        if !(first..=last).contains(&today) {
            view.set_year(Year::Last12);
        }
        view.select(today);
    }
    if enter {
        actions.push(Action::OpenDay(view.sel));
    }
}

fn controls(
    ui: &mut Ui,
    view: &mut StatsView,
    rail: &[(String, Option<String>)],
    today: NaiveDate,
) {
    ui.horizontal(|ui| {
        let show = ui.label("Show");
        egui::ComboBox::from_id_salt("stats-filter")
            .selected_text(view.filter.label().to_string())
            .width(200.0)
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut view.filter, Filter::All, "All activities");
                let mut cats: Vec<Option<&String>> = Vec::new();
                for (_, c) in rail {
                    if !cats.contains(&c.as_ref()) {
                        cats.push(c.as_ref());
                    }
                }
                // Only worth listing when there is more than the no-category group.
                if cats.iter().any(Option::is_some) {
                    ui.separator();
                    ui.weak("Categories");
                    for c in cats {
                        let f = Filter::Category(c.cloned());
                        let label = f.label().to_string();
                        ui.selectable_value(&mut view.filter, f, label);
                    }
                }
                ui.separator();
                ui.weak("Activities");
                let mut names: Vec<&String> = rail.iter().map(|(n, _)| n).collect();
                names.sort_by_key(|n| n.to_lowercase());
                for n in names {
                    ui.selectable_value(&mut view.filter, Filter::Activity(n.clone()), n.as_str());
                }
            })
            .response
            .labelled_by(show.id);
        let mut year = view.year;
        let over = ui.label("in");
        egui::ComboBox::from_id_salt("stats-year")
            .selected_text(year.label())
            .show_ui(ui, |ui| {
                for y in Year::choices(view.first_day, today) {
                    ui.selectable_value(&mut year, y, y.label());
                }
            })
            .response
            .labelled_by(over.id);
        if year != view.year {
            view.set_year(year);
        }
        let (first, last) = view.days();
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.weak(format!(
                "{} – {}",
                first.format("%-d %b %Y"),
                last.format("%-d %b %Y")
            ));
        });
    });
}

fn tiles(ui: &mut Ui, s: &Summary, (first, last): (NaiveDate, NaiveDate), today: NaiveDate) {
    let span = (last - first).num_days() + 1;
    let per_day = match s.active_days {
        0 => "–".to_string(),
        n => fmt_hours(s.total / n as i64),
    };
    ui.horizontal_wrapped(|ui| {
        tile(ui, &fmt_hours(s.total), "tracked");
        tile(
            ui,
            &s.active_days.to_string(),
            &format!("active days of {span}"),
        );
        tile(ui, &per_day, "per active day");
        if (first..=last).contains(&today) {
            tile(ui, &days(s.current_streak), "current streak");
        }
        tile(ui, &days(s.longest_streak), "longest streak");
    });
}

fn tile(ui: &mut Ui, value: &str, label: &str) {
    egui::Frame::group(ui.style())
        .inner_margin(egui::Margin::symmetric(12, 8))
        .show(ui, |ui| {
            ui.set_min_width(112.0);
            ui.vertical(|ui| {
                ui.label(RichText::new(value).size(20.0).strong());
                ui.weak(label);
            });
        });
}

fn days(n: usize) -> String {
    if n == 1 {
        "1 day".to_string()
    } else {
        format!("{n} days")
    }
}

/// An empty heatmap cell, which the heat colors blend from.
fn empty_cell(ui: &Ui) -> Color32 {
    ui.visuals().widgets.inactive.bg_fill
}

/// The color for a heat level: from an empty cell towards `base`, and past
/// it towards the text color (lighter on dark, darker on light) at the top.
fn shade(ui: &Ui, base: Color32, level: u8) -> Color32 {
    match level {
        0 => empty_cell(ui),
        1..=3 => empty_cell(ui).lerp_to_gamma(base, SHADES[level as usize]),
        _ => base.lerp_to_gamma(ui.visuals().strong_text_color(), 0.35),
    }
}

fn monday(d: NaiveDate) -> NaiveDate {
    d - Duration::days(d.weekday().num_days_from_monday() as i64)
}

/// Where each day sits in the heatmap: one column per week, Monday on top,
/// below a row of month names and right of a column of weekday names.
#[derive(Clone, Copy)]
struct Grid {
    first: NaiveDate,
    last: NaiveDate,
    /// The Monday of the first column.
    monday: NaiveDate,
    cols: i64,
    /// Distance between neighbouring cells' corners.
    pitch: f32,
    /// Top-left corner of the first column's Monday.
    origin: Pos2,
}

impl Grid {
    /// Lay out `first..=last` in `width`, with the labels' corner at `min`.
    fn new(first: NaiveDate, last: NaiveDate, width: f32, min: Pos2) -> Self {
        let cols = (monday(last) - monday(first)).num_days() / 7 + 1;
        Self {
            first,
            last,
            monday: monday(first),
            cols,
            pitch: ((width - WEEKDAY_W) / cols as f32).floor().clamp(8.0, 20.0),
            origin: min + egui::vec2(WEEKDAY_W, MONTH_H),
        }
    }

    fn size(&self) -> egui::Vec2 {
        egui::vec2(
            WEEKDAY_W + self.pitch * self.cols as f32,
            MONTH_H + self.pitch * 7.0,
        )
    }

    fn cell_size(&self) -> f32 {
        self.pitch - (self.pitch * 0.18).round().max(2.0)
    }

    fn contains(&self, d: NaiveDate) -> bool {
        (self.first..=self.last).contains(&d)
    }

    fn cell(&self, d: NaiveDate) -> Rect {
        let off = (d - self.monday).num_days();
        let at =
            self.origin + egui::vec2((off / 7) as f32 * self.pitch, (off % 7) as f32 * self.pitch);
        Rect::from_min_size(at, egui::vec2(self.cell_size(), self.cell_size()))
    }

    /// The day whose cell (or the gap after it) is at `p`.
    fn day_at(&self, p: Pos2) -> Option<NaiveDate> {
        let v = p - self.origin;
        if v.x < 0.0 || v.y < 0.0 {
            return None;
        }
        let (c, r) = ((v.x / self.pitch) as i64, (v.y / self.pitch) as i64);
        let d = self.monday + Duration::days(c * 7 + r);
        (r < 7 && c < self.cols && self.contains(d)).then_some(d)
    }
}

fn heatmap(
    ui: &mut Ui,
    view: &mut StatsView,
    per_day: &BTreeMap<NaiveDate, i64>,
    (first, last): (NaiveDate, NaiveDate),
    today: NaiveDate,
    base: Color32,
    actions: &mut Vec<Action>,
) {
    let width = ui.available_width();
    let size = Grid::new(first, last, width, Pos2::ZERO).size();
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let grid = Grid::new(first, last, width, rect.min);
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Grid, true, "Day heatmap"));

    let hovered = resp.hover_pos().and_then(|p| grid.day_at(p));
    let pressed = resp.interact_pointer_pos().and_then(|p| grid.day_at(p));
    if let Some(d) = pressed.filter(|_| resp.clicked()) {
        view.select(d);
    }
    if let Some(d) = pressed.filter(|_| resp.double_clicked()) {
        actions.push(Action::OpenDay(d));
    }

    let painter = ui.painter_at(rect);
    let text = ui.visuals().text_color();
    let weak = ui.visuals().weak_text_color();
    let round = (grid.cell_size() * 0.2).round();
    let mut d = first;
    while d <= last {
        let ms = per_day.get(&d).copied().unwrap_or(0);
        painter.rect_filled(grid.cell(d), round, shade(ui, base, stats::heat_level(ms)));
        d += Duration::days(1);
    }
    let outline = |d: NaiveDate, stroke: Stroke| {
        if grid.contains(d) {
            painter.rect_stroke(
                grid.cell(d).expand(1.0),
                round + 1.0,
                stroke,
                egui::StrokeKind::Outside,
            );
        }
    };
    outline(today, Stroke::new(1.0, weak));
    if let Some(h) = hovered {
        outline(h, Stroke::new(1.0, text));
    }
    outline(view.sel, Stroke::new(2.0, style::ACCENT));

    let font = FontId::proportional(11.0);
    for (row, name) in [(0, "Mon"), (2, "Wed"), (4, "Fri")] {
        let y = grid.origin.y + row as f32 * grid.pitch + grid.cell_size() / 2.0;
        painter.text(
            Pos2::new(rect.left(), y),
            Align2::LEFT_CENTER,
            name,
            font.clone(),
            weak,
        );
    }
    // Each month's name over the week its first day falls in, unless the
    // previous name would run into it. The first month and each January
    // also say the year.
    let first_month = first.with_day(1).expect("day 1 exists");
    let mut free_x = f32::MIN;
    let mut m = first_month;
    while m <= last {
        let x = grid.cell(m.max(first)).left();
        if x >= free_x {
            let label = if m.month() == 1 || m == first_month {
                m.format("%b %Y").to_string()
            } else {
                m.format("%b").to_string()
            };
            let r = painter.text(
                Pos2::new(x, rect.top()),
                Align2::LEFT_TOP,
                label,
                font.clone(),
                weak,
            );
            free_x = r.right() + 6.0;
        }
        m = m + chrono::Months::new(1);
    }

    if let Some(h) = hovered {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
        let ms = per_day.get(&h).copied().unwrap_or(0);
        resp.on_hover_ui_at_pointer(|ui| {
            ui.label(RichText::new(h.format("%a %-d %b %Y").to_string()).strong());
            ui.label(if ms > 0 {
                fmt_hours(ms)
            } else {
                "nothing recorded".to_string()
            });
        });
    }
}

fn legend(ui: &mut Ui, base: Color32) {
    const TIPS: [&str; 5] = ["nothing", "under 1h", "1–4h", "4–8h", "8h or more"];
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 3.0;
        ui.weak("less");
        for (level, tip) in TIPS.iter().enumerate() {
            let (r, resp) = ui.allocate_exact_size(egui::vec2(11.0, 11.0), Sense::hover());
            ui.painter()
                .rect_filled(r, 2.0, shade(ui, base, level as u8));
            resp.on_hover_text(*tip);
        }
        ui.weak("more");
    });
}

fn selected_day(ui: &mut Ui, day: NaiveDate, totals: &[Total], actions: &mut Vec<Action>) {
    let sum: i64 = totals.iter().map(|t| t.ms).sum();
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(day.format("%A %-d %B %Y").to_string())
                .strong()
                .size(16.0),
        );
        if sum > 0 {
            ui.label(RichText::new(fmt_hours(sum)).monospace());
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if ui
                .button("Open in Day")
                .on_hover_text("Enter, or double-click the day")
                .clicked()
            {
                actions.push(Action::OpenDay(day));
            }
        });
    });
    if totals.is_empty() {
        ui.weak("Nothing recorded.");
        return;
    }
    let max = totals.iter().map(|t| t.ms).max().unwrap_or(1).max(1);
    let bar_w = (ui.available_width() * 0.5).max(80.0);
    egui::Grid::new("stats-day")
        .num_columns(3)
        .spacing([14.0, 6.0])
        .show(ui, |ui| {
            for t in totals {
                ui.label(&t.name);
                let (r, _) = ui.allocate_exact_size(egui::vec2(bar_w, 10.0), Sense::hover());
                style::bar(ui, r, t.ms as f32 / max as f32, style::activity(&t.name));
                ui.label(RichText::new(fmt_hours(t.ms)).monospace());
                ui.end_row();
            }
        });
}

/// A column per month; the selected day's month is drawn at full strength.
fn months_chart(ui: &mut Ui, months: &[(NaiveDate, i64)], sel: NaiveDate, base: Color32) {
    const BARS_H: f32 = 110.0;
    const LABEL_H: f32 = 18.0;
    const VALUE_H: f32 = 16.0;
    let size = egui::vec2(ui.available_width(), VALUE_H + BARS_H + LABEL_H);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::hover());
    if months.is_empty() {
        return;
    }
    let painter = ui.painter_at(rect);
    let weak = ui.visuals().weak_text_color();
    let pitch = rect.width() / months.len() as f32;
    let bar_w = (pitch * 0.6).min(44.0);
    let base_y = rect.bottom() - LABEL_H;
    let max = months.iter().map(|(_, ms)| *ms).max().unwrap_or(0).max(1);
    let sel_month = sel.with_day(1).expect("day 1 exists");
    let hovered = resp
        .hover_pos()
        .map(|p| (((p.x - rect.left()) / pitch) as usize).min(months.len() - 1));

    painter.hline(
        rect.left()..=rect.right(),
        base_y,
        Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color),
    );
    for (i, (m, ms)) in months.iter().enumerate() {
        let cx = rect.left() + pitch * (i as f32 + 0.5);
        let h = BARS_H * (*ms as f32 / max as f32);
        let color = if *m == sel_month || hovered == Some(i) {
            base
        } else {
            empty_cell(ui).lerp_to_gamma(base, 0.6)
        };
        if *ms > 0 {
            let bar = Rect::from_min_max(
                Pos2::new(cx - bar_w / 2.0, base_y - h.max(2.0)),
                Pos2::new(cx + bar_w / 2.0, base_y),
            );
            painter.rect_filled(bar, 3.0, color);
            if pitch >= 44.0 {
                painter.text(
                    Pos2::new(cx, bar.top() - 2.0),
                    Align2::CENTER_BOTTOM,
                    fmt_hours(*ms),
                    FontId::monospace(10.5),
                    weak,
                );
            }
        }
        painter.text(
            Pos2::new(cx, base_y + 3.0),
            Align2::CENTER_TOP,
            m.format("%b").to_string(),
            FontId::proportional(11.0),
            if *m == sel_month {
                ui.visuals().text_color()
            } else {
                weak
            },
        );
    }
    if let Some(i) = hovered {
        let (m, ms) = months[i];
        resp.on_hover_ui_at_pointer(|ui| {
            ui.label(RichText::new(m.format("%B %Y").to_string()).strong());
            ui.label(fmt_hours(ms));
        });
    }
}

/// Time per activity, under its category when there are categories.
/// Returns the filter for a row the user clicked.
fn breakdown(
    ui: &mut Ui,
    totals: &[Total],
    cats: &HashMap<&str, Option<&str>>,
    grouped: bool,
    filter: &Filter,
) -> Option<Filter> {
    if totals.is_empty() {
        ui.weak("Nothing recorded.");
        return None;
    }
    // Groups in order of time, activities within them likewise (`totals`
    // comes largest first).
    let mut groups: Vec<(Option<&str>, i64, Vec<&Total>)> = Vec::new();
    for t in totals {
        let cat = if grouped {
            cats.get(t.name.as_str()).copied().flatten()
        } else {
            None
        };
        match groups.iter_mut().find(|(c, _, _)| *c == cat) {
            Some(g) => {
                g.1 += t.ms;
                g.2.push(t);
            }
            None => groups.push((cat, t.ms, vec![t])),
        }
    }
    groups.sort_by_key(|g| std::cmp::Reverse(g.1));

    let sum: i64 = totals.iter().map(|t| t.ms).sum::<i64>().max(1);
    let max = totals[0].ms.max(1);
    let bar_w = (ui.available_width() * 0.45).max(80.0);
    let share = |ms: i64| format!("{:.0}%", 100.0 * ms as f64 / sum as f64);
    let mut picked = None;
    egui::Grid::new("stats-breakdown")
        .num_columns(4)
        .spacing([14.0, 6.0])
        .show(ui, |ui| {
            for (cat, total, acts) in &groups {
                if grouped {
                    let f = Filter::Category(cat.map(str::to_string));
                    let name = RichText::new(f.label()).strong();
                    if ui
                        .selectable_label(*filter == f, name)
                        .on_hover_text(toggle_tip(filter, &f, "category"))
                        .clicked()
                    {
                        picked = Some(toggle(filter, f));
                    }
                    ui.label("");
                    ui.label(RichText::new(fmt_hours(*total)).monospace().strong());
                    ui.weak(share(*total));
                    ui.end_row();
                }
                for t in acts {
                    let f = Filter::Activity(t.name.clone());
                    ui.horizontal(|ui| {
                        if grouped {
                            ui.add_space(14.0);
                        }
                        if ui
                            .selectable_label(*filter == f, t.name.as_str())
                            .on_hover_text(toggle_tip(filter, &f, "activity"))
                            .clicked()
                        {
                            picked = Some(toggle(filter, f.clone()));
                        }
                    });
                    let (r, _) = ui.allocate_exact_size(egui::vec2(bar_w, 10.0), Sense::hover());
                    style::bar(ui, r, t.ms as f32 / max as f32, style::activity(&t.name));
                    ui.label(RichText::new(fmt_hours(t.ms)).monospace());
                    ui.weak(share(t.ms));
                    ui.end_row();
                }
            }
        });
    picked
}

/// Clicking a row shows only it; clicking it again shows everything.
fn toggle(current: &Filter, clicked: Filter) -> Filter {
    if *current == clicked {
        Filter::All
    } else {
        clicked
    }
}

fn toggle_tip(current: &Filter, row: &Filter, what: &str) -> String {
    if current == row {
        "show all activities".to_string()
    } else {
        format!("show only this {what}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::{Harness, Node, kittest::Queryable};
    use enoch::db::SessionRow;
    use enoch::timefmt::local_midnight_ms;

    struct Tab {
        view: StatsView,
        rail: Vec<(String, Option<String>)>,
        actions: Vec<Action>,
    }

    fn today() -> NaiveDate {
        Local::now().date_naive()
    }

    fn week_ago() -> NaiveDate {
        today() - Duration::days(7)
    }

    /// Two hours from 09:00 on `d`.
    fn morning(id: i64, name: &str, d: NaiveDate) -> SessionRow {
        let nine = local_midnight_ms(d) + 9 * 3_600_000;
        SessionRow {
            id,
            name: name.to_string(),
            started_at: nine,
            ended_at: Some(nine + 2 * 3_600_000),
        }
    }

    /// The Stats tab over coding (work) today, and review (work) and guitar
    /// (no category) a week ago.
    fn harness() -> Harness<'static, Tab> {
        let mut view = StatsView::new();
        let (first, last) = view.days();
        let sessions = [
            morning(1, "coding", today()),
            morning(2, "review", week_ago()),
            morning(3, "guitar", week_ago()),
        ];
        view.ledger = Ledger::new(&sessions, first, last, now_ms());
        let rail = vec![
            ("guitar".to_string(), None),
            ("coding".to_string(), Some("work".to_string())),
            ("review".to_string(), Some("work".to_string())),
        ];
        Harness::builder()
            .with_size(egui::vec2(1000.0, 1400.0))
            // Real frame times, so two quick clicks make a double click.
            .with_step_dt(1.0 / 60.0)
            .build_ui_state(
                |ui, t: &mut Tab| show(ui, &mut t.view, &t.rail, &mut t.actions),
                Tab {
                    view,
                    rail,
                    actions: Vec::new(),
                },
            )
    }

    /// The breakdown row labelled `name`: the lowest such widget, since the
    /// filter menu above may show the same name.
    fn row<'h>(h: &'h Harness<'_, Tab>, name: &'h str) -> Node<'h> {
        h.query_all_by_label(name)
            .max_by(|a, b| a.rect().top().total_cmp(&b.rect().top()))
            .unwrap_or_else(|| panic!("no {name:?} row"))
    }

    fn opened(h: &Harness<'_, Tab>) -> Vec<NaiveDate> {
        h.state()
            .actions
            .iter()
            .filter_map(|a| match a {
                Action::OpenDay(d) => Some(*d),
                _ => None,
            })
            .collect()
    }

    fn click_at(h: &Harness<'_, Tab>, p: Pos2) {
        h.hover_at(p);
        h.drag_at(p);
        h.drop_at(p);
    }

    #[test]
    fn breakdown_rows_filter_and_toggle_back() {
        let mut h = harness();
        row(&h, "review").click();
        h.run();
        assert_eq!(h.state().view.filter, Filter::Activity("review".into()));
        row(&h, "review").click();
        h.run();
        assert_eq!(h.state().view.filter, Filter::All);

        row(&h, "work").click();
        h.run();
        assert_eq!(h.state().view.filter, Filter::Category(Some("work".into())));
        assert_eq!(h.query_all_by_label("guitar").count(), 0);
        assert!(h.query_all_by_label("coding").count() > 0);
    }

    #[test]
    fn arrow_keys_move_by_week_and_day_and_enter_opens_the_day() {
        let mut h = harness();
        h.key_press(egui::Key::ArrowLeft);
        h.run();
        h.key_press(egui::Key::ArrowUp);
        h.run();
        let d = today() - Duration::days(8);
        assert_eq!(h.state().view.sel, d);
        h.key_press(egui::Key::Enter);
        h.run();
        assert_eq!(opened(&h), vec![d]);

        // T comes back to today, and nothing goes past it.
        h.key_press(egui::Key::T);
        h.run();
        h.key_press(egui::Key::ArrowRight);
        h.run();
        assert_eq!(h.state().view.sel, today());
    }

    #[test]
    fn clicking_a_day_selects_it_and_double_clicking_opens_it() {
        let mut h = harness();
        let map = h.get_by_label("Day heatmap").rect();
        let (first, last) = h.state().view.days();
        let p = Grid::new(first, last, map.width(), map.min)
            .cell(week_ago())
            .center();
        click_at(&h, p);
        h.run();
        assert_eq!(h.state().view.sel, week_ago());
        assert!(opened(&h).is_empty());
        // The selected day lists what was done then.
        assert!(h.query_all_by_label("guitar").count() > 0);

        // Half a second later, so as not to make a triple click with the
        // first; then two clicks on consecutive frames.
        h.run_steps(30);
        click_at(&h, p);
        h.step();
        click_at(&h, p);
        h.run();
        assert_eq!(opened(&h), vec![week_ago()]);
    }

    #[test]
    fn every_day_has_its_own_cell() {
        let today = NaiveDate::from_ymd_opt(2026, 11, 8).unwrap();
        let (first, last) = Year::Last12.days(today);
        let g = Grid::new(first, last, 900.0, Pos2::new(10.0, 20.0));
        let mut d = first;
        while d <= last {
            assert_eq!(g.day_at(g.cell(d).center()), Some(d));
            d += Duration::days(1);
        }
        assert_eq!(g.day_at(g.cell(first - Duration::days(1)).center()), None);
        assert_eq!(g.day_at(g.cell(last + Duration::days(1)).center()), None);
        // Over the weekday names.
        assert_eq!(g.day_at(Pos2::new(15.0, 60.0)), None);
    }
}
