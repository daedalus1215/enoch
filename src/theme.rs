//! Centralized colors and small style helpers so panels stay visually consistent.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;

use crate::timefmt::now_ms;

/// Accent: selection, active mode, highlights.
pub const ACCENT: Color = Color::Cyan;
/// Dim text: chrome, hints, empty states, bar troughs.
pub const DIM: Color = Color::DarkGray;
/// Secondary text: labels, right-hand summaries.
pub const MUTED: Color = Color::Gray;
/// Primary foreground.
pub const FG: Color = Color::White;
/// Keys in status bar, messages.
pub const KEY: Color = Color::Yellow;

/// Subtle full-width background for the selected row.
pub const SELECTED_BG: Color = Color::Rgb(38, 38, 50);

/// Style for a selected row's text.
pub fn selected_style() -> Style {
    Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
}

/// Dim span for chrome text.
pub fn dim_span(t: &str) -> Span<'static> {
    Span::styled(t.to_string(), Style::default().fg(DIM))
}

/// Muted span for secondary text.
pub fn muted_span(t: &str) -> Span<'static> {
    Span::styled(t.to_string(), Style::default().fg(MUTED))
}

/// Key span (yellow) for status-bar hints.
pub fn key_span(t: &str) -> Span<'static> {
    Span::styled(t.to_string(), Style::default().fg(KEY))
}

/// Running-indicator color: the activity's color on even seconds, dim on odd —
/// a slow heartbeat so live indicators feel alive.
pub fn pulse(color: Color) -> Color {
    if now_ms().div_euclid(1000) % 2 == 0 {
        color
    } else {
        DIM
    }
}

/// Stable, distinguishable color per activity name.
pub fn activity_color(name: &str) -> Color {
    let mut h: u64 = 5381;
    for b in name.to_lowercase().bytes() {
        h = h.wrapping_mul(33).wrapping_add(b as u64);
    }
    const PALETTE: [Color; 8] = [
        Color::Rgb(229, 92, 92),
        Color::Rgb(255, 153, 102),
        Color::Rgb(255, 212, 76),
        Color::Rgb(128, 203, 110),
        Color::Rgb(80, 180, 229),
        Color::Rgb(156, 165, 255),
        Color::Rgb(204, 153, 255),
        Color::Rgb(255, 121, 198),
    ];
    PALETTE[(h % PALETTE.len() as u64) as usize]
}

/// Filled/empty bar of `width` chars representing `ms` out of `max_ms`.
pub fn bar_spans(ms: i64, max_ms: i64, width: usize, color: Color) -> Vec<Span<'static>> {
    let filled = if max_ms > 0 && width > 0 {
        ((ms as f64 * width as f64 / max_ms as f64).round() as usize).min(width)
    } else {
        0
    };
    (0..width)
        .map(|i| {
            if i < filled {
                Span::styled("█".to_string(), Style::default().fg(color))
            } else {
                Span::styled("░".to_string(), Style::default().fg(DIM))
            }
        })
        .collect()
}
