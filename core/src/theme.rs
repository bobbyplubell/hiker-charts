//! Host-injected colors: background, foreground, gridlines, and a series palette.
//!
//! Colors come from the host so a chart matches the app's light/dark appearance
//! (SPEC §6.1). A spec may override the series palette via its config (SPEC §6.3),
//! but absent that the backend uses `Theme::series` indexed by series position.

/// An 8-bit RGBA color. Small and `Copy`, so it is passed by value throughout.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Color {
    /// Construct an opaque color (alpha 255) from red/green/blue components.
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }
}

/// A full theme: structural colors plus a categorical palette indexed per series
/// (`theme.series[i % len]`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Theme {
    pub background: Color,
    pub foreground: Color,
    pub gridline: Color,
    pub series: Vec<Color>,
}

impl Default for Theme {
    /// A neutral light theme: white background, near-black foreground, light grid,
    /// and a six-color categorical palette.
    fn default() -> Self {
        Self {
            background: Color::rgb(255, 255, 255),
            foreground: Color::rgb(33, 33, 33),
            gridline: Color::rgb(221, 221, 221),
            series: vec![
                Color::rgb(31, 119, 180),
                Color::rgb(255, 127, 14),
                Color::rgb(44, 160, 44),
                Color::rgb(214, 39, 40),
                Color::rgb(148, 103, 189),
                Color::rgb(140, 86, 75),
            ],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Color, Theme};

    #[test]
    fn rgb_is_opaque() {
        assert_eq!(Color::rgb(1, 2, 3), Color { r: 1, g: 2, b: 3, a: 255 });
    }

    #[test]
    fn default_theme_has_palette() {
        let theme = Theme::default();
        assert!(!theme.series.is_empty());
        assert_eq!(theme.background, Color::rgb(255, 255, 255));
    }

    #[test]
    fn themes_compare_by_value() {
        assert_eq!(Theme::default(), Theme::default());
    }
}
