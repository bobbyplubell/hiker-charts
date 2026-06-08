//! The swappable rendering seam: the `Backend` trait and its neutral inputs.
//!
//! `ResolvedChart` is renderer-neutral — pure `f64` coordinates plus axis label
//! maps — so no backend ever sees the original wide/long shape or a plotters type
//! (SPEC §4.1). A `Backend` paints a chart plus theme and size into a `RenderOutput`.

use crate::dsl::{Config, Mark, Scale};
use crate::theme::Theme;

/// The pixel dimensions a chart is painted into. Small and `Copy`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Size {
    pub width: u32,
    pub height: u32,
}

/// A fully resolved, renderer-neutral chart: a mark, series with `f64` points, two
/// axes with kinds/label maps, and the presentation config.
#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedChart {
    pub mark: Mark,
    pub series: Vec<Series>,
    /// Radial wedges for the `Arc` mark; empty for cartesian marks (where `series` is used).
    pub slices: Vec<Slice>,
    /// The grid for the `Table` mark; `None` for plotted marks. Cells are already coerced and
    /// formatted to display strings so the backend only lays them out.
    pub table: Option<TableView>,
    pub x_axis: Axis,
    pub y_axis: Axis,
    pub config: Config,
}

/// A renderer-neutral resolved table for the `Table` mark: column headers and row-major
/// formatted cells, in the natural one-row-per-record orientation. `transpose` asks the
/// backend to draw fields down the left and records across instead.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TableView {
    pub headers: Vec<String>,
    pub rows: Vec<Vec<String>>,
    pub transpose: bool,
}

/// One data series: a display name, its `(x, y)` points in `f64` coordinates, and optional
/// per-point sizes (parallel to `points`, empty unless a size channel is bound for a bubble
/// chart — the backend zips points with sizes for the `Point` mark).
#[derive(Clone, Debug, PartialEq)]
pub struct Series {
    pub name: String,
    pub points: Vec<(f64, f64)>,
    pub sizes: Vec<f32>,
}

/// One radial wedge of an `Arc` chart: its category label, its angular magnitude, and the
/// palette index that colors it.
#[derive(Clone, Debug, PartialEq)]
pub struct Slice {
    pub label: String,
    pub value: f64,
    pub color_index: usize,
}

/// An axis: its title, the kind that tells a backend how to format ticks, and the scale
/// transform (log/sqrt/domain/zero) the backend applies to coordinates and tick labels.
#[derive(Clone, Debug, PartialEq)]
pub struct Axis {
    pub title: String,
    pub kind: AxisKind,
    pub scale: Scale,
}

/// How an axis's `f64` coordinates should be interpreted and labeled.
#[derive(Clone, Debug, PartialEq)]
pub enum AxisKind {
    Quantitative,
    /// `f64` is epoch seconds; a backend formats it back to a date string.
    Temporal,
    /// `f64` is an index into these stable category labels.
    Categorical(Vec<String>),
}

/// A rendered chart artifact: a self-contained SVG string. Rasterization to RGBA
/// is the host's job via resvg (SPEC §4.2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RenderOutput {
    pub svg: String,
}

/// Why a render failed: an empty chart, or a backend-specific error message.
#[derive(Debug)]
pub enum RenderError {
    Empty,
    Backend(String),
}

impl std::fmt::Display for RenderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => write!(f, "chart has no drawable series or points"),
            Self::Backend(msg) => write!(f, "backend error: {msg}"),
        }
    }
}

impl std::error::Error for RenderError {}

/// The one seam plotters lives behind. A first-party engine is a second impl later
/// (SPEC §4.1); the core never names a concrete backend type.
pub trait Backend {
    /// Paint a resolved chart with the given theme at the given size.
    fn render(
        &self,
        chart: &ResolvedChart,
        theme: &Theme,
        size: Size,
    ) -> Result<RenderOutput, RenderError>;
}

#[cfg(test)]
mod tests {
    use super::{Axis, AxisKind, RenderError, RenderOutput, Series, Size, Slice};
    use crate::dsl::Scale;

    #[test]
    fn size_is_copy_and_comparable() {
        let s = Size { width: 640, height: 480 };
        let t = s;
        assert_eq!(s, t);
    }

    #[test]
    fn categorical_axis_holds_labels() {
        let axis = Axis {
            title: "month".to_string(),
            kind: AxisKind::Categorical(vec!["jan".to_string(), "feb".to_string()]),
            scale: Scale::default(),
        };
        match axis.kind {
            AxisKind::Categorical(ref labels) => assert_eq!(labels.len(), 2),
            _ => panic!("expected categorical"),
        }
    }

    #[test]
    fn render_error_messages_distinct() {
        assert_ne!(
            RenderError::Empty.to_string(),
            RenderError::Backend("x".to_string()).to_string()
        );
    }

    #[test]
    fn series_and_output_construct() {
        let series = Series { name: "a".to_string(), points: vec![(0.0, 1.0)], sizes: Vec::new() };
        assert_eq!(series.points.len(), 1);
        let out = RenderOutput { svg: "<svg/>".to_string() };
        assert!(out.svg.contains("svg"));
    }

    #[test]
    fn slice_holds_label_value_and_color_index() {
        let s = Slice { label: "north".to_string(), value: 42.0, color_index: 2 };
        assert_eq!(s.label, "north");
        assert_eq!(s.color_index, 2);
    }
}
