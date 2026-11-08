//! Colors, type sizes and small painting helpers for the desktop window.

use eframe::egui::{self, Color32, FontId, RichText, TextStyle, Ui};

/// Accent: the TUI's cyan, a little softer for large surfaces.
pub const ACCENT: Color32 = Color32::from_rgb(80, 200, 220);

/// Error text.
pub const ERROR: Color32 = Color32::from_rgb(229, 92, 92);

/// An activity's color; the same one the TUI uses.
pub fn activity(name: &str) -> Color32 {
    let [r, g, b] = enoch::palette::activity_rgb(name);
    Color32::from_rgb(r, g, b)
}

/// Install type sizes and spacing on both the dark and light themes.
pub fn apply(ctx: &egui::Context) {
    ctx.all_styles_mut(|s| {
        s.text_styles = [
            (TextStyle::Small, FontId::proportional(11.5)),
            (TextStyle::Body, FontId::proportional(14.5)),
            (TextStyle::Button, FontId::proportional(14.0)),
            (TextStyle::Monospace, FontId::monospace(13.5)),
            (TextStyle::Heading, FontId::proportional(20.0)),
        ]
        .into();
        s.spacing.item_spacing = egui::vec2(8.0, 6.0);
        s.spacing.button_padding = egui::vec2(10.0, 4.0);
        s.visuals.selection.stroke.color = ACCENT;
    });
}

/// Small all-caps heading above a block, like the TUI's panel titles.
pub fn section(ui: &mut Ui, title: &str) {
    let color = ui.visuals().weak_text_color();
    ui.label(RichText::new(title).small().strong().color(color));
}

#[derive(Clone, Copy)]
pub enum Icon {
    Play,
    Stop,
}

/// A small square button with a painted icon. Painting avoids depending on
/// media-control glyphs, which egui's bundled fonts do not have.
pub fn icon_button(ui: &mut Ui, icon: Icon, label: &str) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(22.0, 20.0), egui::Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, label));
    let v = ui.style().interact(&resp);
    if resp.hovered() || resp.has_focus() {
        ui.painter().rect_filled(rect, 4.0, v.weak_bg_fill);
    }
    let c = rect.center();
    let color = v.fg_stroke.color;
    match icon {
        Icon::Play => {
            let s = 5.0;
            let pts = vec![
                c + egui::vec2(-0.7 * s, -s),
                c + egui::vec2(-0.7 * s, s),
                c + egui::vec2(s, 0.0),
            ];
            ui.painter()
                .add(egui::Shape::convex_polygon(pts, color, egui::Stroke::NONE));
        }
        Icon::Stop => {
            let r = egui::Rect::from_center_size(c, egui::vec2(9.0, 9.0));
            ui.painter().rect_filled(r, 1.5, color);
        }
    }
    resp.on_hover_text(label)
}

/// A rounded bar: `frac` of `rect` filled with `color` over a faint trough.
pub fn bar(ui: &Ui, rect: egui::Rect, frac: f32, color: Color32) {
    let p = ui.painter();
    p.rect_filled(rect, 3.0, ui.visuals().extreme_bg_color);
    let w = rect.width() * frac.clamp(0.0, 1.0);
    if w > 0.5 {
        p.rect_filled(
            egui::Rect::from_min_size(rect.min, egui::vec2(w, rect.height())),
            3.0,
            color,
        );
    }
}
