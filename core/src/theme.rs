//! Host-injected colors: background, foreground, gridlines, and a series palette.
//!
//! Colors come from the host so a chart matches the app's light/dark appearance
//! (SPEC §6.1). A spec may override the series palette via its config (SPEC §6.3),
//! but absent that the backend uses `Theme::series` indexed by series position.
//!
//! This mirrors how Hiker themes diagrams (SPEC §6): one injected `Theme` drives
//! every chart color, exactly as a single host theme drives all mermaid colors.
//! The host picks a light/dark preset (`Theme::light`/`Theme::dark`, or
//! `Theme::from_dark_mode`) and may globally swap the categorical scale to a named
//! `Palette` via `Theme::with_palette` without rebuilding the rest of the theme.

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
    /// The neutral light preset; see [`Theme::light`].
    fn default() -> Self {
        Self::light()
    }
}

/// A named categorical color scale the host can select globally (SPEC §6). Each
/// palette is a tasteful, well-separated set of ~6-8 colors applied to series by
/// position; swap it onto a theme with [`Theme::with_palette`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Palette {
    /// The classic Tableau/Vega "category10"-style scale; strong general default.
    Category10,
    /// Soft, low-saturation tints for gentle multi-series fills.
    Pastel,
    /// A warm reds-through-yellows ramp of categorical hues.
    Warm,
    /// A cool blues-through-greens ramp of categorical hues.
    Cool,
    /// A single-hue (blue) monochrome progression for ordered series.
    Mono,
}

impl Palette {
    /// The colors of this palette, in series order (6-8 entries, all distinct).
    pub fn colors(self) -> Vec<Color> {
        match self {
            Self::Category10 => vec![
                Color::rgb(31, 119, 180),
                Color::rgb(255, 127, 14),
                Color::rgb(44, 160, 44),
                Color::rgb(214, 39, 40),
                Color::rgb(148, 103, 189),
                Color::rgb(140, 86, 75),
                Color::rgb(227, 119, 194),
                Color::rgb(127, 127, 127),
            ],
            Self::Pastel => vec![
                Color::rgb(166, 206, 227),
                Color::rgb(178, 223, 138),
                Color::rgb(251, 154, 153),
                Color::rgb(253, 191, 111),
                Color::rgb(202, 178, 214),
                Color::rgb(255, 255, 153),
            ],
            Self::Warm => vec![
                Color::rgb(127, 0, 0),
                Color::rgb(179, 24, 24),
                Color::rgb(214, 69, 39),
                Color::rgb(241, 108, 32),
                Color::rgb(253, 160, 33),
                Color::rgb(254, 205, 92),
                Color::rgb(255, 237, 160),
            ],
            Self::Cool => vec![
                Color::rgb(8, 64, 129),
                Color::rgb(8, 104, 172),
                Color::rgb(43, 140, 190),
                Color::rgb(78, 179, 211),
                Color::rgb(123, 204, 196),
                Color::rgb(168, 221, 181),
                Color::rgb(204, 235, 197),
            ],
            Self::Mono => vec![
                Color::rgb(8, 48, 107),
                Color::rgb(33, 113, 181),
                Color::rgb(66, 146, 198),
                Color::rgb(107, 174, 214),
                Color::rgb(158, 202, 225),
                Color::rgb(198, 219, 239),
            ],
        }
    }
}

impl Theme {
    /// A light preset: white background, near-black foreground/axes, light-grey
    /// gridlines, and the strong `Category10` categorical scale.
    pub fn light() -> Self {
        Self {
            background: Color::rgb(255, 255, 255),
            foreground: Color::rgb(33, 33, 33),
            gridline: Color::rgb(221, 221, 221),
            series: Palette::Category10.colors(),
        }
    }

    /// A dark preset: near-black background, light foreground/axes, dark-grey
    /// gridlines, and the `Category10` categorical scale.
    pub fn dark() -> Self {
        Self {
            background: Color::rgb(24, 26, 27),
            foreground: Color::rgb(225, 225, 225),
            gridline: Color::rgb(64, 66, 68),
            series: Palette::Category10.colors(),
        }
    }

    /// Pick the dark preset when `dark`, else the light preset — the host's
    /// one-call light/dark switch.
    pub fn from_dark_mode(dark: bool) -> Self {
        if dark {
            Self::dark()
        } else {
            Self::light()
        }
    }

    /// Return this theme with its categorical scale replaced by `palette`,
    /// leaving the structural colors untouched. Lets a host globally swap the
    /// series palette without rebuilding the theme.
    #[must_use]
    pub fn with_palette(mut self, palette: Palette) -> Self {
        self.series = palette.colors();
        self
    }
}

#[cfg(test)]
mod tests {
    use super::{Color, Palette, Theme};

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

    #[test]
    fn light_and_dark_differ() {
        let light = Theme::light();
        let dark = Theme::dark();
        assert_ne!(light.background, dark.background);
        assert_ne!(light.foreground, dark.foreground);
        assert_ne!(light.gridline, dark.gridline);
        assert_eq!(Theme::default(), light);
        assert_eq!(Theme::from_dark_mode(true), dark);
        assert_eq!(Theme::from_dark_mode(false), light);
    }

    #[test]
    fn every_palette_is_nonempty_and_distinct() {
        let all = [
            Palette::Category10,
            Palette::Pastel,
            Palette::Warm,
            Palette::Cool,
            Palette::Mono,
        ];
        let mut lists = Vec::new();
        for p in all {
            let colors = p.colors();
            assert!(colors.len() >= 6, "palette should have >=6 colors");
            // All colors within a palette are distinct.
            for (i, a) in colors.iter().enumerate() {
                for b in &colors[i + 1..] {
                    assert_ne!(a, b, "duplicate color within {p:?}");
                }
            }
            lists.push(colors);
        }
        // The palettes differ from one another.
        for (i, a) in lists.iter().enumerate() {
            for b in &lists[i + 1..] {
                assert_ne!(a, b, "two palettes are identical");
            }
        }
    }

    #[test]
    fn with_palette_swaps_series_only() {
        let base = Theme::light();
        let swapped = Theme::light().with_palette(Palette::Warm);
        assert_eq!(base.background, swapped.background);
        assert_eq!(base.foreground, swapped.foreground);
        assert_eq!(swapped.series, Palette::Warm.colors());
        assert_ne!(base.series, swapped.series);
    }
}
