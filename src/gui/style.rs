//! Colors, type sizes and small painting helpers for the desktop window.
//!
//! The palette is bright, made for the dark theme. On the light theme the
//! helpers here deepen each color just enough to read against the window:
//! [`ink`] for text, [`graphic`] for bars, dots and other marks. On the
//! dark theme they return the palette unchanged.

use eframe::egui::{self, Color32, FontId, RichText, TextStyle, ThemePreference, Ui, Visuals};

/// Accent: the TUI's cyan, a little softer for large surfaces.
const ACCENT: Color32 = Color32::from_rgb(80, 200, 220);

/// Error text.
const ERROR: Color32 = Color32::from_rgb(229, 92, 92);

/// Contrast for text (WCAG AA for normal text).
const TEXT_CONTRAST: f32 = 4.5;
/// Contrast for bars, dots and outlines (WCAG for non-text graphics).
const GRAPHIC_CONTRAST: f32 = 3.0;

/// The accent color, for text and outlines.
pub fn accent(ui: &Ui) -> Color32 {
    ink(ui, ACCENT)
}

/// The color of errors and of the Day tab's "now" marker.
pub fn error(ui: &Ui) -> Color32 {
    ink(ui, ERROR)
}

/// An activity's color, for bars and dots; the same hue the TUI uses.
pub fn activity(ui: &Ui, name: &str) -> Color32 {
    graphic(ui, activity_rgb(name))
}

/// An activity's color for text in it.
pub fn activity_ink(ui: &Ui, name: &str) -> Color32 {
    ink(ui, activity_rgb(name))
}

fn activity_rgb(name: &str) -> Color32 {
    let [r, g, b] = enoch::palette::activity_rgb(name);
    Color32::from_rgb(r, g, b)
}

/// `color` as text: deepened (on light) or lightened until it reads
/// against the window's backgrounds.
pub fn ink(ui: &Ui, color: Color32) -> Color32 {
    legible(ui.visuals(), color, TEXT_CONTRAST)
}

/// `color` as a bar, dot or outline: like [`ink`], with a lower bar.
pub fn graphic(ui: &Ui, color: Color32) -> Color32 {
    legible(ui.visuals(), color, GRAPHIC_CONTRAST)
}

/// `color`, moved towards the strong text color in small steps until it
/// stands out `min` : 1 against both the panel and the text field
/// backgrounds (the lightest and darkest the window paints).
fn legible(v: &Visuals, color: Color32, min: f32) -> Color32 {
    let bgs = [v.panel_fill, v.extreme_bg_color];
    let enough = |c: Color32| bgs.iter().all(|&bg| contrast(c, bg) >= min);
    let toward = v.strong_text_color();
    (0..=20)
        .map(|step| color.lerp_to_gamma(toward, step as f32 / 20.0))
        .find(|&c| enough(c))
        .unwrap_or(toward)
}

/// WCAG contrast ratio between two opaque colors, from 1 to 21.
fn contrast(a: Color32, b: Color32) -> f32 {
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// WCAG relative luminance of an sRGB color.
fn luminance(c: Color32) -> f32 {
    let lin = |v: u8| {
        let s = v as f32 / 255.0;
        if s <= 0.04045 {
            s / 12.92
        } else {
            ((s + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * lin(c.r()) + 0.7152 * lin(c.g()) + 0.0722 * lin(c.b())
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
    // Selected tabs and labels are written in the accent over the
    // selection's light blue, where the cyan itself would barely show.
    ctx.style_mut_of(egui::Theme::Light, |s| {
        let on_selection = Visuals {
            panel_fill: s.visuals.selection.bg_fill,
            ..s.visuals.clone()
        };
        s.visuals.selection.stroke.color = legible(&on_selection, ACCENT, TEXT_CONTRAST);
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
    /// A sun, for the light theme.
    Light,
    /// Half light, half dark: follow the system.
    System,
    /// A crescent moon, for the dark theme.
    Dark,
}

/// A small square button with a painted icon. Painting avoids depending on
/// media-control glyphs, which egui's bundled fonts do not have.
pub fn icon_button(ui: &mut Ui, icon: Icon, label: &str) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(22.0, 20.0), egui::Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, label));
    let v = ui.style().interact(&resp);
    let mut bg = ui.visuals().panel_fill;
    if resp.hovered() || resp.has_focus() {
        bg = v.weak_bg_fill;
        ui.painter().rect_filled(rect, 4.0, bg);
    }
    paint_icon(ui, icon, rect.center(), v.fg_stroke.color, bg);
    resp.on_hover_text(label)
}

/// Light / System / Dark buttons that set the window's theme. Returns
/// whether the choice changed.
pub fn theme_switch(ui: &mut Ui, pref: &mut ThemePreference) -> bool {
    let system = match ui.input(|i| i.raw.system_theme) {
        Some(egui::Theme::Light) => "follow the system: light now",
        Some(egui::Theme::Dark) => "follow the system: dark now",
        None => "follow the system (unknown here, so dark)",
    };
    let before = *pref;
    let mut buttons = [
        (
            ThemePreference::Light,
            Icon::Light,
            "Light theme",
            "light theme",
        ),
        (
            ThemePreference::System,
            Icon::System,
            "System theme",
            system,
        ),
        (
            ThemePreference::Dark,
            Icon::Dark,
            "Dark theme",
            "dark theme",
        ),
    ];
    // Light on the left, dark on the right, wherever the switch is placed.
    if ui.layout().prefer_right_to_left() {
        buttons.reverse();
    }
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        for (choice, icon, name, tip) in buttons {
            if toggle_icon(ui, icon, name, *pref == choice)
                .on_hover_text(tip)
                .clicked()
            {
                *pref = choice;
            }
        }
    });
    *pref != before
}

/// An icon button that shows whether it is the selected one of a set.
fn toggle_icon(ui: &mut Ui, icon: Icon, label: &str, selected: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(22.0, 20.0), egui::Sense::click());
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, selected, label));
    let v = ui.style().interact_selectable(&resp, selected);
    let mut bg = ui.visuals().panel_fill;
    if selected {
        bg = ui.visuals().selection.bg_fill;
    } else if resp.hovered() || resp.has_focus() {
        bg = v.weak_bg_fill;
    }
    if bg != ui.visuals().panel_fill {
        ui.painter().rect_filled(rect, 4.0, bg);
    }
    paint_icon(ui, icon, rect.center(), v.fg_stroke.color, bg);
    resp
}

/// Paint `icon` centered on `c`; `bg` is what it sits on, for cut-outs.
fn paint_icon(ui: &Ui, icon: Icon, c: egui::Pos2, color: Color32, bg: Color32) {
    let p = ui.painter();
    match icon {
        Icon::Play => {
            let s = 5.0;
            let pts = vec![
                c + egui::vec2(-0.7 * s, -s),
                c + egui::vec2(-0.7 * s, s),
                c + egui::vec2(s, 0.0),
            ];
            p.add(egui::Shape::convex_polygon(pts, color, egui::Stroke::NONE));
        }
        Icon::Stop => {
            let r = egui::Rect::from_center_size(c, egui::vec2(9.0, 9.0));
            p.rect_filled(r, 1.5, color);
        }
        Icon::Light => {
            p.circle_filled(c, 3.0, color);
            for i in 0..8 {
                let dir = egui::Vec2::angled(i as f32 * std::f32::consts::FRAC_PI_4);
                p.line_segment(
                    [c + dir * 5.0, c + dir * 7.0],
                    egui::Stroke::new(1.4, color),
                );
            }
        }
        Icon::System => {
            p.circle_stroke(c, 5.5, egui::Stroke::new(1.4, color));
            // The left half filled: a convex fan of points on the arc.
            let pts: Vec<egui::Pos2> = (0..=16)
                .map(|i| {
                    let a = std::f32::consts::FRAC_PI_2 + i as f32 / 16.0 * std::f32::consts::PI;
                    c + egui::Vec2::angled(a) * 5.5
                })
                .collect();
            p.add(egui::Shape::convex_polygon(pts, color, egui::Stroke::NONE));
        }
        Icon::Dark => {
            p.circle_filled(c, 6.0, color);
            p.circle_filled(c + egui::vec2(3.0, -2.5), 5.0, bg);
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn palette() -> Vec<Color32> {
        let mut all: Vec<Color32> = enoch::palette::ACTIVITY
            .iter()
            .map(|&[r, g, b]| Color32::from_rgb(r, g, b))
            .collect();
        all.extend([ACCENT, ERROR]);
        all
    }

    #[test]
    fn the_dark_theme_keeps_the_palette() {
        let dark = Visuals::dark();
        for c in palette() {
            assert_eq!(legible(&dark, c, TEXT_CONTRAST), c, "{c:?}");
        }
    }

    #[test]
    fn the_light_theme_deepens_colors_until_they_read() {
        let light = Visuals::light();
        for c in palette() {
            for min in [GRAPHIC_CONTRAST, TEXT_CONTRAST] {
                let l = legible(&light, c, min);
                assert!(contrast(l, light.panel_fill) >= min, "{c:?} -> {l:?}");
                assert!(contrast(l, light.extreme_bg_color) >= min, "{c:?} -> {l:?}");
                // Deepened, not blackened: still recognisably that color.
                assert!(l.r().max(l.g()).max(l.b()) >= 90, "{c:?} -> {l:?}");
            }
        }
        // The palette's bright yellow, for one, comes out deeper.
        let yellow = Color32::from_rgb(255, 212, 76);
        assert_ne!(legible(&light, yellow, GRAPHIC_CONTRAST), yellow);
    }

    #[test]
    fn the_theme_switch_changes_the_theme() {
        use egui_kittest::{Harness, kittest::Queryable};
        let mut h = Harness::builder()
            .with_size(egui::vec2(200.0, 40.0))
            .build_ui_state(
                |ui, pref: &mut ThemePreference| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if theme_switch(ui, pref) {
                            ui.ctx().set_theme(*pref);
                        }
                    });
                },
                ThemePreference::System,
            );
        h.run();
        let x =
            |h: &Harness<'_, ThemePreference>, name: &str| h.get_by_label(name).rect().center().x;
        // Light, then System, then Dark, even laid out right to left.
        assert!(x(&h, "Light theme") < x(&h, "System theme"));
        assert!(x(&h, "System theme") < x(&h, "Dark theme"));

        h.get_by_label("Light theme").click();
        h.run();
        assert_eq!(*h.state(), ThemePreference::Light);
        assert_eq!(h.ctx.theme(), egui::Theme::Light);
        // The window repaints in the light colors.
        assert_ne!(
            h.ctx.global_style().visuals.panel_fill,
            Visuals::dark().panel_fill
        );

        h.get_by_label("Dark theme").click();
        h.run();
        assert_eq!(*h.state(), ThemePreference::Dark);
        assert_eq!(h.ctx.theme(), egui::Theme::Dark);
    }

    #[test]
    fn contrast_matches_wcag() {
        assert!((contrast(Color32::BLACK, Color32::WHITE) - 21.0).abs() < 0.01);
        assert!((contrast(Color32::WHITE, Color32::WHITE) - 1.0).abs() < 0.001);
        // #767676 on white is the classic 4.54 : 1.
        assert!((contrast(Color32::from_gray(0x76), Color32::WHITE) - 4.54).abs() < 0.01);
    }
}
