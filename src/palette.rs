//! Front-end-neutral colors, so an activity looks the same in the TUI and GUI.

/// The per-activity palette, as RGB.
pub const ACTIVITY: [[u8; 3]; 8] = [
    [229, 92, 92],
    [255, 153, 102],
    [255, 212, 76],
    [128, 203, 110],
    [80, 180, 229],
    [156, 165, 255],
    [204, 153, 255],
    [255, 121, 198],
];

/// Stable, distinguishable color per activity name (case-insensitive).
pub fn activity_rgb(name: &str) -> [u8; 3] {
    let mut h: u64 = 5381;
    for b in name.to_lowercase().bytes() {
        h = h.wrapping_mul(33).wrapping_add(b as u64);
    }
    ACTIVITY[(h % ACTIVITY.len() as u64) as usize]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn color_ignores_case() {
        assert_eq!(activity_rgb("Coding"), activity_rgb("coding"));
    }
}
