//! plotters-backed SVG `Backend` implementation for `hiker-charts`.
//!
//! This is the single crate permitted to import `plotters` — the swappable-renderer seam
//! described in `SPEC.md` §4.1. It maps a renderer-neutral `ResolvedChart` from the core
//! crate onto plotters drawing calls over one `f64 x f64` coordinate system, emitting a
//! self-contained SVG string. See `IMPLEMENTATION.md` §3.

use std::ops::Range;

use hiker_charts_core::backend::{
    Axis, AxisKind, Backend, RenderError, RenderOutput, ResolvedChart, Series, Size,
};
use hiker_charts_core::dsl::Mark;
use hiker_charts_core::theme::{Color, Theme};
use plotters::chart::{ChartContext, ChartBuilder, SeriesLabelPosition};
use plotters::coord::types::RangedCoordf64;
use plotters::coord::cartesian::Cartesian2d;
use plotters::drawing::IntoDrawingArea;
use plotters::element::{Circle, PathElement, Rectangle};
use plotters::series::{AreaSeries, LineSeries};
use plotters::style::{Color as PlottersColor, RGBAColor, ShapeStyle};
use plotters::backend::SVGBackend;

/// The font family for all chart text. plotters' SVG backend emits only `<text>` elements
/// (no glyph paths, `ttf` is off), so the host's resvg picks the actual face (SPEC §4.2).
const FONT: &str = "sans-serif";

/// A plotters cartesian chart over `f64 x f64` drawing into the SVG string buffer. Named so
/// the per-mark draw helpers have a single concrete context type to borrow.
type Ctx<'a> = ChartContext<'a, SVGBackend<'a>, Cartesian2d<RangedCoordf64, RangedCoordf64>>;

/// The v1 plotters SVG backend: the only `Backend` impl in this workspace. A unit struct —
/// all inputs arrive through `render`; a first-party engine becomes a second impl later.
pub struct PlottersSvg;

impl Backend for PlottersSvg {
    fn render(
        &self,
        chart: &ResolvedChart,
        theme: &Theme,
        size: Size,
    ) -> Result<RenderOutput, RenderError> {
        let (x_range, y_range) = data_ranges(&chart.series).ok_or(RenderError::Empty)?;
        let mut svg = String::new();
        paint(chart, theme, size, x_range, y_range, &mut svg)?;
        Ok(RenderOutput { svg })
    }
}

/// Drive plotters end to end into `svg`: fill the background, set up the cartesian area and
/// mesh, draw the mark, then the legend. Split from `render` so the trait method owns only
/// the empty-check and the string's lifetime, keeping both functions under the length caps.
fn paint(
    chart: &ResolvedChart,
    theme: &Theme,
    size: Size,
    x_range: Range<f64>,
    y_range: Range<f64>,
    svg: &mut String,
) -> Result<(), RenderError> {
    let root = SVGBackend::with_string(svg, (size.width, size.height)).into_drawing_area();
    root.fill(&to_rgba(theme.background)).map_err(backend_err)?;

    let fg = to_rgba(theme.foreground);
    let mut builder = ChartBuilder::on(&root);
    builder.margin(14).x_label_area_size(42).y_label_area_size(54);
    if let Some(title) = chart.config.title.as_deref() {
        builder.caption(title, (FONT, 22, &fg));
    }
    let mut ctx = builder
        .build_cartesian_2d(x_range, y_range)
        .map_err(backend_err)?;

    configure_mesh(&mut ctx, &chart.x_axis, &chart.y_axis, theme)?;
    draw_mark(&mut ctx, chart, theme)?;
    if chart.config.legend && chart.series.iter().any(|s| !s.name.is_empty()) {
        draw_legend(&mut ctx, theme)?;
    }
    root.present().map_err(backend_err)?;
    Ok(())
}

/// Configure axis lines, gridlines and tick labels from the theme and each axis's kind. The
/// per-axis tick formatter is built once here and passed to plotters as a `&dyn Fn`.
fn configure_mesh(
    ctx: &mut Ctx<'_>,
    x_axis: &Axis,
    y_axis: &Axis,
    theme: &Theme,
) -> Result<(), RenderError> {
    let fg = to_rgba(theme.foreground);
    let grid = to_rgba(theme.gridline);
    let x_fmt = tick_formatter(&x_axis.kind);
    let y_fmt = tick_formatter(&y_axis.kind);
    ctx.configure_mesh()
        .axis_style(fg)
        .bold_line_style(grid)
        .light_line_style(grid.mix(0.5))
        .label_style((FONT, 14, &fg))
        .x_desc(x_axis.title.as_str())
        .y_desc(y_axis.title.as_str())
        .x_label_formatter(&|v| x_fmt(*v))
        .y_label_formatter(&|v| y_fmt(*v))
        .draw()
        .map_err(backend_err)
}

/// Dispatch on the chart's mark to the matching draw routine. `Bar` is grouped when there
/// are multiple series; the line/point/area marks draw one styled series per `Series`.
fn draw_mark(ctx: &mut Ctx<'_>, chart: &ResolvedChart, theme: &Theme) -> Result<(), RenderError> {
    match chart.mark {
        Mark::Line => draw_lines(ctx, chart, theme),
        Mark::Point => draw_points(ctx, chart, theme),
        Mark::Area => draw_areas(ctx, chart, theme),
        Mark::Bar => draw_bars(ctx, chart, theme),
    }
}

/// Draw each series as a connected `LineSeries`, annotating it for the legend.
fn draw_lines(ctx: &mut Ctx<'_>, chart: &ResolvedChart, theme: &Theme) -> Result<(), RenderError> {
    for (i, s) in chart.series.iter().enumerate() {
        let color = series_color(chart, theme, i);
        let anno = ctx
            .draw_series(LineSeries::new(s.points.iter().copied(), stroke(color, 2)))
            .map_err(backend_err)?;
        label_key(anno, &s.name, color);
    }
    Ok(())
}

/// Draw each series as one filled `Circle` per point (a scatter mark).
fn draw_points(ctx: &mut Ctx<'_>, chart: &ResolvedChart, theme: &Theme) -> Result<(), RenderError> {
    for (i, s) in chart.series.iter().enumerate() {
        let color = series_color(chart, theme, i);
        let style: ShapeStyle = color.filled();
        let anno = ctx
            .draw_series(s.points.iter().map(|&p| Circle::new(p, 3_i32, style)))
            .map_err(backend_err)?;
        label_key(anno, &s.name, color);
    }
    Ok(())
}

/// Draw each series as a semi-transparent `AreaSeries` filled down to the y baseline.
fn draw_areas(ctx: &mut Ctx<'_>, chart: &ResolvedChart, theme: &Theme) -> Result<(), RenderError> {
    let baseline = area_baseline(&chart.series);
    for (i, s) in chart.series.iter().enumerate() {
        let color = series_color(chart, theme, i);
        let anno = ctx
            .draw_series(AreaSeries::new(
                s.points.iter().copied(),
                baseline,
                color.mix(0.35),
            ))
            .map_err(backend_err)?;
        label_key(anno, &s.name, color);
    }
    Ok(())
}

/// Draw grouped vertical bars: within each x slot the series share the slot width, each
/// offset so multi-series bars sit side by side (grouped, never stacked — SPEC §2.3).
fn draw_bars(ctx: &mut Ctx<'_>, chart: &ResolvedChart, theme: &Theme) -> Result<(), RenderError> {
    let n = chart.series.len().max(1);
    let slot = bar_slot_width(&chart.series);
    let group = slot * 0.8;
    let bar_w = group / n as f64;
    for (i, s) in chart.series.iter().enumerate() {
        let color = series_color(chart, theme, i);
        let style: ShapeStyle = color.filled();
        let left = bar_w.mul_add(i as f64, -group / 2.0);
        let anno = ctx
            .draw_series(s.points.iter().map(|&(x, y)| {
                Rectangle::new([(x + left, 0.0), (x + left + bar_w, y)], style)
            }))
            .map_err(backend_err)?;
        label_key(anno, &s.name, color);
    }
    Ok(())
}

/// Attach a name + colored legend key to a just-drawn series so the legend can list it.
/// Skips unnamed series so a single-series chart shows no spurious blank legend entry.
fn label_key(anno: &mut plotters::chart::SeriesAnno<'_, SVGBackend<'_>>, name: &str, color: RGBAColor) {
    if name.is_empty() {
        return;
    }
    anno.label(name.to_string())
        .legend(move |(x, y)| PathElement::new([(x, y), (x + 18, y)], stroke(color, 3)));
}

/// Draw the series-label box (legend) styled with the theme's colors.
fn draw_legend(ctx: &mut Ctx<'_>, theme: &Theme) -> Result<(), RenderError> {
    ctx.configure_series_labels()
        .position(SeriesLabelPosition::UpperRight)
        .border_style(to_rgba(theme.foreground))
        .background_style(to_rgba(theme.background).mix(0.85))
        .label_font((FONT, 14, &to_rgba(theme.foreground)))
        .draw()
        .map_err(backend_err)
}

/// Convert a core `Color` to a plotters `RGBAColor`, mapping the 8-bit alpha to `0.0..=1.0`.
fn to_rgba(c: Color) -> RGBAColor {
    RGBAColor(c.r, c.g, c.b, f64::from(c.a) / 255.0)
}

/// A stroked (unfilled) `ShapeStyle` at `width` px — used for lines and shape borders.
const fn stroke(color: RGBAColor, width: u32) -> ShapeStyle {
    ShapeStyle { color, filled: false, stroke_width: width }
}

/// The color for series `i`: the spec's hex palette override if present and parseable,
/// else the theme's categorical palette indexed `i % len` (SPEC §6.3).
fn series_color(chart: &ResolvedChart, theme: &Theme, i: usize) -> RGBAColor {
    if let Some(palette) = chart.config.palette.as_ref()
        && !palette.is_empty()
        && let Some(c) = parse_hex(&palette[i % palette.len()])
    {
        return c;
    }
    if theme.series.is_empty() {
        return to_rgba(theme.foreground);
    }
    to_rgba(theme.series[i % theme.series.len()])
}

/// Parse a `#rgb` or `#rrggbb` hex string into an opaque `RGBAColor`. Returns `None` on any
/// malformed input so the caller can fall back to the theme palette.
fn parse_hex(s: &str) -> Option<RGBAColor> {
    let h = s.strip_prefix('#').unwrap_or(s);
    let (r, g, b) = match h.len() {
        3 => (dup(&h[0..1])?, dup(&h[1..2])?, dup(&h[2..3])?),
        6 => (byte(&h[0..2])?, byte(&h[2..4])?, byte(&h[4..6])?),
        _ => return None,
    };
    Some(RGBAColor(r, g, b, 1.0))
}

/// Parse a two-hex-digit byte (`"ff"` -> 255).
fn byte(s: &str) -> Option<u8> {
    u8::from_str_radix(s, 16).ok()
}

/// Expand a single hex digit to a byte (`"f"` -> 255), the `#rgb` shorthand.
fn dup(s: &str) -> Option<u8> {
    let v = u8::from_str_radix(s, 16).ok()?;
    Some(v * 16 + v)
}

/// Compute the `(x, y)` data ranges spanning every point across all series, padded so marks
/// are not flush against the axes. Returns `None` when there is no drawable point at all
/// (no series, or every series empty) — the `RenderError::Empty` signal.
fn data_ranges(series: &[Series]) -> Option<(Range<f64>, Range<f64>)> {
    let mut x_min = f64::INFINITY;
    let mut x_max = f64::NEG_INFINITY;
    let mut y_min = f64::INFINITY;
    let mut y_max = f64::NEG_INFINITY;
    for s in series {
        for &(x, y) in &s.points {
            x_min = x_min.min(x);
            x_max = x_max.max(x);
            y_min = y_min.min(y);
            y_max = y_max.max(y);
        }
    }
    if !x_min.is_finite() || !y_min.is_finite() {
        return None;
    }
    Some((pad(x_min, x_max), pad_y(y_min, y_max)))
}

/// Pad a min/max span by 5% on each side; widen a zero-width span to a unit interval.
fn pad(lo: f64, hi: f64) -> Range<f64> {
    let span = hi - lo;
    if span <= 0.0 {
        return (lo - 0.5)..(hi + 0.5);
    }
    let m = span * 0.05;
    (lo - m)..(hi + m)
}

/// Pad a y span like `pad`, but always anchor the lower bound at zero or below so bars and
/// areas read from a sensible baseline.
fn pad_y(lo: f64, hi: f64) -> Range<f64> {
    let base = lo.min(0.0);
    let span = hi - base;
    if span <= 0.0 {
        return base..(hi + 1.0);
    }
    base..(hi + span * 0.08)
}

/// The y baseline an area fills down to: zero when the data straddles or sits above it,
/// otherwise the minimum y so a wholly-negative series still fills toward its top.
fn area_baseline(series: &[Series]) -> f64 {
    let min = series
        .iter()
        .flat_map(|s| s.points.iter())
        .map(|&(_, y)| y)
        .fold(f64::INFINITY, f64::min);
    if min.is_finite() {
        min.min(0.0)
    } else {
        0.0
    }
}

/// The width of one x slot for grouped bars: the smallest gap between consecutive distinct
/// x positions, or `1.0` when there is a single point (categorical indices are unit-spaced).
fn bar_slot_width(series: &[Series]) -> f64 {
    let mut xs: Vec<f64> = series
        .iter()
        .flat_map(|s| s.points.iter().map(|&(x, _)| x))
        .collect();
    xs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    xs.dedup();
    let mut min_gap = f64::INFINITY;
    for w in xs.windows(2) {
        let gap = w[1] - w[0];
        if gap > 0.0 && gap < min_gap {
            min_gap = gap;
        }
    }
    if min_gap.is_finite() {
        min_gap
    } else {
        1.0
    }
}

/// Build a tick-label formatter for an axis kind: quantitative renders the number,
/// categorical indexes the label map (bounds-guarded), temporal formats epoch seconds back
/// to an ISO date string (our own formatter, the inverse of core's `parse_date`; no chrono).
fn tick_formatter(kind: &AxisKind) -> Box<dyn Fn(f64) -> String> {
    match kind {
        AxisKind::Quantitative => Box::new(format_number),
        AxisKind::Temporal => Box::new(|v| format_epoch(v as i64)),
        AxisKind::Categorical(labels) => {
            let labels = labels.clone();
            Box::new(move |v| {
                let i = v.round();
                if i >= 0.0 && (i as usize) < labels.len() {
                    labels[i as usize].clone()
                } else {
                    String::new()
                }
            })
        }
    }
}

/// Format a quantitative tick: integers without a decimal point, others trimmed of trailing
/// zeros so axis labels stay compact.
fn format_number(v: f64) -> String {
    if v.fract() == 0.0 && v.abs() < 1e15 {
        return format!("{}", v as i64);
    }
    let s = format!("{v:.3}");
    let trimmed = s.trim_end_matches('0').trim_end_matches('.');
    trimmed.to_string()
}

/// Format epoch seconds (UTC) back to an ISO date string `YYYY-MM-DD`, the inverse of core's
/// `parse_date`. Self-contained civil-date math (no chrono, SPEC §4.6); negative/pre-epoch
/// inputs floor toward the past so the conversion stays total.
fn format_epoch(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}-{m:02}-{d:02}")
}

/// Convert a count of days since the Unix epoch to a `(year, month, day)` civil date using
/// Howard Hinnant's `civil_from_days` algorithm — pure integer arithmetic, valid for the
/// proleptic Gregorian calendar and any sign of `z`.
const fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Map a plotters drawing error into the core `RenderError::Backend` variant, flattening the
/// backend-specific error type to a string so it never leaks into a `pub` signature.
fn backend_err<E: std::fmt::Debug>(err: E) -> RenderError {
    RenderError::Backend(format!("{err:?}"))
}

#[cfg(test)]
mod tests;
