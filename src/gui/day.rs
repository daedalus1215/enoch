//! Day view: one lane per activity, sessions as bars you drag to edit.
//!
//! - drag a bar's edge to change its start or end, or its middle to move it
//! - drag in an empty stretch of a lane to record a new session
//! - right-click a bar to delete it or move it to another activity
//!
//! Times snap to 5 minutes (1 with Shift held). Running sessions are shown
//! up to now but can't be edited until stopped, as in the TUI.

use chrono::{Duration, Local, NaiveDate};
use eframe::egui::{
    self, Align, Align2, Color32, CursorIcon, FontId, Id, Layout, Pos2, Rect, RichText, Sense,
    Stroke, Ui, UiBuilder,
};
use enoch::db::SessionRow;
use enoch::timefmt::{day_bounds, fmt_hm, fmt_short};

use super::Action;
use super::style;
use super::timeline::{self, Axis, Grip, HOUR, Limits, MINUTE};

const LABEL_W: f32 = 190.0;
const HEAD_H: f32 = 22.0;
const LANE_H: f32 = 34.0;
const BAR_PAD: f32 = 7.0;
/// Pixels at each end of a bar that grab the edge instead of the body.
const EDGE: f32 = 6.0;

pub struct DayView {
    pub day: NaiveDate,
    /// Sessions overlapping `day`, reloaded with the rest of the window.
    pub sessions: Vec<SessionRow>,
    full_day: bool,
    /// Lanes the user added that have no sessions yet.
    extra_lanes: Vec<String>,
    lane_text: String,
    drag: Option<Drag>,
}

enum DragKind {
    Edit {
        id: i64,
        name: String,
        grip: Grip,
        orig: (i64, i64),
    },
    Create {
        name: String,
    },
}

struct Drag {
    kind: DragKind,
    /// Pointer time where the drag began, and where it is now.
    anchor: i64,
    current: i64,
}

impl Drag {
    /// The session this drag would produce if released now.
    fn preview(&self, lim: &Limits) -> (i64, i64) {
        match &self.kind {
            DragKind::Edit { grip, orig, .. } => {
                timeline::reshape(*grip, orig.0, orig.1, self.current - self.anchor, lim)
            }
            DragKind::Create { .. } => timeline::span(self.anchor, self.current, lim),
        }
    }

    fn finish(&self, lim: &Limits) -> Option<Action> {
        let (s, e) = self.preview(lim);
        match &self.kind {
            DragKind::Edit { id, name, orig, .. } => {
                (e > s && (s, e) != *orig).then(|| Action::UpdateSession {
                    id: *id,
                    name: name.clone(),
                    start: s,
                    end: e,
                })
            }
            DragKind::Create { name } => (e - s >= lim.step).then(|| Action::AddSession {
                name: name.clone(),
                start: s,
                end: e,
            }),
        }
    }
}

impl DayView {
    pub fn new() -> Self {
        Self {
            day: Local::now().date_naive(),
            sessions: Vec::new(),
            full_day: false,
            extra_lanes: Vec::new(),
            lane_text: String::new(),
            drag: None,
        }
    }

    fn set_day(&mut self, d: NaiveDate) {
        if d != self.day {
            self.day = d;
            self.drag = None;
            self.extra_lanes.clear();
        }
    }

    /// Activities with sessions this day in order of first start, then any
    /// lanes the user added.
    fn lanes(&self) -> Vec<String> {
        let mut lanes: Vec<String> = Vec::new();
        let names = self
            .sessions
            .iter()
            .map(|s| &s.name)
            .chain(self.extra_lanes.iter());
        for n in names {
            if !lanes.iter().any(|l| l.eq_ignore_ascii_case(n)) {
                lanes.push(n.clone());
            }
        }
        lanes
    }
}

pub fn show(
    ui: &mut Ui,
    view: &mut DayView,
    names: &[String],
    now: i64,
    actions: &mut Vec<Action>,
) {
    let (d0, d1) = day_bounds(view.day);
    let lim = Limits {
        origin: d0,
        step: if ui.input(|i| i.modifiers.shift) {
            MINUTE
        } else {
            5 * MINUTE
        },
        hi: now,
        min_len: MINUTE,
    };

    header(ui, view, d0, d1, now);
    ui.add_space(8.0);

    let range = if view.full_day {
        (d0, d1)
    } else {
        let today = (d0..d1).contains(&now);
        let points = view
            .sessions
            .iter()
            .flat_map(|s| [s.started_at, s.ended_at.unwrap_or(now)])
            .chain(today.then_some(now));
        timeline::fit((d0, d1), points, 6 * HOUR)
    };
    timeline_area(ui, view, range, (d0, d1), &lim, names, now, actions);

    ui.add_space(10.0);
    add_lane_row(ui, view);
}

fn header(ui: &mut Ui, view: &mut DayView, d0: i64, d1: i64, now: i64) {
    let total: i64 = view.sessions.iter().map(|s| clipped(s, d0, d1, now)).sum();
    let typing = ui.ctx().egui_wants_keyboard_input();
    let (prev, next, today) = ui.input(|i| {
        (
            !typing && i.key_pressed(egui::Key::ArrowLeft),
            !typing && i.key_pressed(egui::Key::ArrowRight),
            !typing && i.key_pressed(egui::Key::T),
        )
    });
    ui.horizontal(|ui| {
        let mut d = view.day;
        if ui
            .button(RichText::new("‹").size(18.0))
            .on_hover_text("previous day (Left)")
            .clicked()
            || prev
        {
            d -= Duration::days(1);
        }
        if ui
            .button(RichText::new("›").size(18.0))
            .on_hover_text("next day (Right)")
            .clicked()
            || next
        {
            d += Duration::days(1);
        }
        ui.label(RichText::new(view.day.format("%A %-d %B %Y").to_string()).heading());
        let is_today = view.day == Local::now().date_naive();
        if ui
            .add_enabled(!is_today, egui::Button::new("Today"))
            .on_hover_text("jump to today (T)")
            .clicked()
            || today
        {
            d = Local::now().date_naive();
        }
        view.set_day(d);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.label(RichText::new(fmt_short(total)).monospace().strong());
            ui.weak("total");
            ui.add_space(12.0);
            ui.checkbox(&mut view.full_day, "Full day");
        });
    });
}

/// Time `s` contributes to `[d0, d1)`, counting a running session up to now.
fn clipped(s: &SessionRow, d0: i64, d1: i64, now: i64) -> i64 {
    let e = s.ended_at.unwrap_or(now);
    (e.min(d1) - s.started_at.max(d0)).max(0)
}

#[allow(clippy::too_many_arguments)]
fn timeline_area(
    ui: &mut Ui,
    view: &mut DayView,
    range: (i64, i64),
    day: (i64, i64),
    lim: &Limits,
    names: &[String],
    now: i64,
    actions: &mut Vec<Action>,
) {
    let lanes = view.lanes();
    let rows = lanes.len().max(1);
    let size = egui::vec2(ui.available_width(), HEAD_H + LANE_H * rows as f32 + 4.0);
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let axis = Axis {
        from: range.0,
        to: range.1,
        x0: rect.left() + LABEL_W,
        x1: rect.right() - 14.0,
    };
    let painter = ui.painter_at(rect);
    let visuals = ui.visuals().clone();
    let weak = visuals.weak_text_color();
    let lanes_top = rect.top() + HEAD_H;

    // Follow the pointer while dragging. On release, commit at the end of
    // this frame so the bar is still drawn where it was dropped (the reload
    // that follows then shows it there too, without flashing back).
    let mut released = false;
    if let Some(d) = &mut view.drag {
        let (pos, down) = ui.input(|i| (i.pointer.interact_pos(), i.pointer.primary_down()));
        if let Some(p) = pos {
            d.current = axis.ms(p.x);
        }
        released = !down;
        ui.ctx().set_cursor_icon(match d.kind {
            DragKind::Edit {
                grip: Grip::Body, ..
            } => CursorIcon::Grabbing,
            DragKind::Edit { .. } => CursorIcon::ResizeHorizontal,
            DragKind::Create { .. } => CursorIcon::Text,
        });
    }

    // Hour grid and labels; thin out labels when hours are narrow.
    let px_per_hour = axis.x(range.0 + HOUR) - axis.x(range.0);
    let every = if px_per_hour >= 44.0 {
        1
    } else if px_per_hour >= 22.0 {
        2
    } else {
        3
    };
    // Hour labels give way to the "now" marker's label where they would collide.
    let now_x = (range.0..=range.1).contains(&now).then(|| axis.x(now));
    let mut h = 0;
    while day.0 + h * HOUR <= day.1 {
        let t = day.0 + h * HOUR;
        if t >= range.0 && t <= range.1 {
            let x = axis.x(t);
            painter.vline(
                x,
                lanes_top..=rect.bottom(),
                Stroke::new(1.0, visuals.widgets.noninteractive.bg_stroke.color),
            );
            let clear_of_now = now_x.is_none_or(|nx| (nx - x).abs() > 28.0);
            if h % every == 0 && clear_of_now {
                // Keep the first and last labels inside the timeline.
                let align = if x > axis.x1 - 18.0 {
                    Align2::RIGHT_TOP
                } else if x < axis.x0 + 18.0 {
                    Align2::LEFT_TOP
                } else {
                    Align2::CENTER_TOP
                };
                painter.text(
                    Pos2::new(x, rect.top() + 3.0),
                    align,
                    fmt_hm(t),
                    FontId::monospace(11.0),
                    weak,
                );
            }
        }
        h += 1;
    }

    if lanes.is_empty() {
        painter.text(
            Pos2::new((axis.x0 + axis.x1) / 2.0, lanes_top + LANE_H / 2.0),
            Align2::CENTER_CENTER,
            "No sessions this day. Add a lane below, then drag in it.",
            FontId::proportional(14.0),
            weak,
        );
    }

    let preview = view.drag.as_ref().map(|d| d.preview(lim));
    for (i, name) in lanes.iter().enumerate() {
        let y0 = lanes_top + i as f32 * LANE_H;
        let color = style::activity(name);
        if i % 2 == 0 {
            painter.rect_filled(
                Rect::from_min_max(
                    Pos2::new(rect.left(), y0),
                    Pos2::new(rect.right(), y0 + LANE_H),
                ),
                0.0,
                visuals.faint_bg_color,
            );
        }

        // Label column: dot, name, and the lane's total for the day.
        let total: i64 = view
            .sessions
            .iter()
            .filter(|s| s.name.eq_ignore_ascii_case(name))
            .map(|s| clipped(s, day.0, day.1, now))
            .sum();
        let label_rect = Rect::from_min_size(
            Pos2::new(rect.left() + 4.0, y0),
            egui::vec2(LABEL_W - 16.0, LANE_H),
        );
        let mut lui = ui.new_child(
            UiBuilder::new()
                .max_rect(label_rect)
                .layout(Layout::right_to_left(Align::Center)),
        );
        lui.label(RichText::new(fmt_short(total)).monospace().color(weak));
        lui.with_layout(Layout::left_to_right(Align::Center), |ui| {
            let (r, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), Sense::hover());
            ui.painter().circle_filled(r.center(), 4.5, color);
            ui.add(egui::Label::new(name.as_str()).truncate());
        });

        // Empty lane space: drag to create a session for this activity.
        let lane_rect = Rect::from_min_max(Pos2::new(axis.x0, y0), Pos2::new(axis.x1, y0 + LANE_H));
        let lane = ui
            .interact(
                lane_rect,
                Id::unique(("enoch-lane", name.to_lowercase())),
                Sense::drag(),
            )
            .on_hover_cursor(CursorIcon::Text);
        if lane.drag_started()
            && let Some(p) = ui.input(|i| i.pointer.press_origin())
        {
            let t = axis.ms(p.x);
            view.drag = Some(Drag {
                kind: DragKind::Create { name: name.clone() },
                anchor: t,
                current: t,
            });
        }
        if let (
            Some(Drag {
                kind: DragKind::Create { name: n },
                ..
            }),
            Some((s, e)),
        ) = (&view.drag, preview)
            && n == name
            && e > s
        {
            let r = bar_rect(&axis, s, e, y0);
            painter.rect_filled(r, 4.0, color.gamma_multiply(0.45));
            painter.rect_stroke(r, 4.0, Stroke::new(1.0, color), egui::StrokeKind::Inside);
            drag_label(&painter, r, s, e, &visuals);
        }

        for s in view
            .sessions
            .iter()
            .filter(|s| s.name.eq_ignore_ascii_case(name))
        {
            let running = s.ended_at.is_none();
            let dragging = matches!(&view.drag, Some(Drag { kind: DragKind::Edit { id, .. }, .. }) if *id == s.id);
            let (start, end) = match (dragging, preview) {
                (true, Some(p)) => p,
                _ => (s.started_at, s.ended_at.unwrap_or(now)),
            };
            if end <= range.0 || start >= range.1 {
                continue;
            }
            let r = bar_rect(&axis, start, end, y0);
            let fill = if dragging {
                color
            } else {
                color.gamma_multiply(0.85)
            };
            painter.rect_filled(r, 4.0, fill);
            if running {
                // A bright leading edge marks a session that is still going.
                painter.vline(
                    r.right() - 1.0,
                    r.y_range(),
                    Stroke::new(2.0, Color32::WHITE),
                );
            }
            if dragging {
                drag_label(&painter, r, start, end, &visuals);
            } else if r.width() > 92.0 {
                painter.text(
                    r.left_center() + egui::vec2(6.0, 0.0),
                    Align2::LEFT_CENTER,
                    format!(
                        "{}–{}",
                        fmt_hm(start),
                        if running { "now".into() } else { fmt_hm(end) }
                    ),
                    FontId::monospace(11.0),
                    Color32::from_black_alpha(200),
                );
            }

            let sense = if running {
                Sense::click()
            } else {
                Sense::click_and_drag()
            };
            let resp = ui.interact(
                r.expand2(egui::vec2(4.0, 0.0)),
                Id::unique(("enoch-session", s.id)),
                sense,
            );
            let start_ok = s.started_at >= range.0;
            let end_ok = s.ended_at.is_some_and(|e| e <= range.1);
            if !running
                && view.drag.is_none()
                && let Some(p) = resp.hover_pos()
            {
                let grip = timeline::grip_at(p.x, r.left(), r.right(), EDGE, start_ok, end_ok);
                ui.ctx().set_cursor_icon(match grip {
                    Grip::Body => CursorIcon::Grab,
                    _ => CursorIcon::ResizeHorizontal,
                });
            }
            if resp.drag_started()
                && let Some(p) = ui.input(|i| i.pointer.press_origin())
            {
                let grip = timeline::grip_at(p.x, r.left(), r.right(), EDGE, start_ok, end_ok);
                let t = axis.ms(p.x);
                view.drag = Some(Drag {
                    kind: DragKind::Edit {
                        id: s.id,
                        name: s.name.clone(),
                        grip,
                        orig: (s.started_at, s.ended_at.unwrap_or(now)),
                    },
                    anchor: t,
                    current: t,
                });
            }
            if view.drag.is_none() {
                let tip = format!(
                    "{}\n{}–{} · {}{}",
                    s.name,
                    fmt_hm(s.started_at),
                    if running { "now".into() } else { fmt_hm(end) },
                    fmt_short(end - s.started_at),
                    if running {
                        "\nrunning — stop it to edit"
                    } else {
                        ""
                    }
                );
                resp.clone().on_hover_text_at_pointer(tip);
            }
            session_menu(&resp, s, running, names, actions);
        }
    }

    // "Now" marker.
    if (range.0..=range.1).contains(&now) {
        let x = axis.x(now);
        painter.vline(x, lanes_top..=rect.bottom(), Stroke::new(1.5, style::ERROR));
        painter.text(
            Pos2::new(x, rect.top() + 3.0),
            Align2::CENTER_TOP,
            "now",
            FontId::proportional(11.0),
            style::ERROR,
        );
    }

    if released && let Some(a) = view.drag.take().and_then(|d| d.finish(lim)) {
        actions.push(a);
    }
}

fn bar_rect(axis: &Axis, start: i64, end: i64, lane_y: f32) -> Rect {
    let x0 = axis.x(start.max(axis.from));
    let x1 = axis.x(end.min(axis.to)).max(x0 + 3.0);
    Rect::from_min_max(
        Pos2::new(x0, lane_y + BAR_PAD),
        Pos2::new(x1, lane_y + LANE_H - BAR_PAD),
    )
}

/// The times a drag would commit, floating above the bar.
fn drag_label(painter: &egui::Painter, r: Rect, s: i64, e: i64, visuals: &egui::Visuals) {
    let text = format!("{}–{} · {}", fmt_hm(s), fmt_hm(e), fmt_short(e - s));
    let galley = painter.layout_no_wrap(text, FontId::monospace(12.0), visuals.strong_text_color());
    let pos = Pos2::new(
        r.center().x - galley.size().x / 2.0,
        r.top() - galley.size().y - 2.0,
    );
    let bg = Rect::from_min_size(pos, galley.size()).expand(3.0);
    painter.rect_filled(bg, 3.0, visuals.extreme_bg_color);
    painter.galley(pos, galley, visuals.strong_text_color());
}

fn session_menu(
    resp: &egui::Response,
    s: &SessionRow,
    running: bool,
    names: &[String],
    actions: &mut Vec<Action>,
) {
    resp.context_menu(|ui| {
        if running {
            ui.weak("running — stop it to edit");
            return;
        }
        let end = s.ended_at.unwrap_or(s.started_at);
        if ui.button("Delete session").clicked() {
            actions.push(Action::DeleteSession(s.id));
            ui.close();
        }
        ui.menu_button("Move to", |ui| {
            for n in names.iter().filter(|n| !n.eq_ignore_ascii_case(&s.name)) {
                if ui.button(n.as_str()).clicked() {
                    actions.push(Action::UpdateSession {
                        id: s.id,
                        name: n.clone(),
                        start: s.started_at,
                        end,
                    });
                    ui.close();
                }
            }
        });
    });
}

/// Add an empty lane for an activity, to draw its first session of the day.
fn add_lane_row(ui: &mut Ui, view: &mut DayView) {
    ui.horizontal(|ui| {
        let resp = ui.add(
            egui::TextEdit::singleline(&mut view.lane_text)
                .hint_text("activity")
                .desired_width(180.0),
        );
        let entered = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        let name = view.lane_text.trim().to_string();
        if (ui
            .add_enabled(!name.is_empty(), egui::Button::new("Add lane"))
            .clicked()
            || entered)
            && !name.is_empty()
        {
            if !view.lanes().iter().any(|l| l.eq_ignore_ascii_case(&name)) {
                view.extra_lanes.push(name);
            }
            view.lane_text.clear();
        }
    });
    ui.add_space(4.0);
    ui.horizontal_wrapped(|ui| {
        ui.weak(
            "Drag in a lane to add a session · drag a bar's edges to retime it, its middle \
             to move it · right-click a bar to delete or move it · hold Shift to snap to 1 \
             minute instead of 5.",
        );
    });
}
