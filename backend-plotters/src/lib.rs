//! plotters-backed SVG `Backend` implementation for `hiker-charts`.
//!
//! This is the single crate permitted to import `plotters` — the swappable-renderer seam
//! described in `SPEC.md` §4.1. It maps a renderer-neutral `ResolvedChart` from the core
//! crate onto plotters drawing calls over one `f64 x f64` coordinate system, emitting a
//! self-contained SVG string. See `IMPLEMENTATION.md` §3.

use std::ops::Range;

use hiker_charts_core::backend::{
    Axis, AxisKind, Backend, RenderError, RenderOutput, ResolvedChart, Series, Size, TableView,
};
use hiker_charts_core::dsl::{Interpolate, Mark, Orientation, Scale};
use hiker_charts_core::display::{format_epoch, format_number};
use hiker_charts_core::registry::caps;
use hiker_charts_core::theme::{Color, Theme};
use plotters::chart::{ChartContext, ChartBuilder, SeriesLabelPosition};
use plotters::coord::types::RangedCoordf64;
use plotters::coord::cartesian::Cartesian2d;
use plotters::drawing::IntoDrawingArea;
use plotters::element::{Circle, PathElement, Rectangle};
use plotters::series::{AreaSeries, LineSeries};
use plotters::style::{Color as PlottersColor, IntoFont, RGBAColor, ShapeStyle, TextStyle};
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
        let mut svg = String::new();
        if chart.mark == Mark::Table {
            paint_table(chart, theme, size, &mut svg)?;
        } else if caps(chart.mark).cartesian {
            paint_cartesian(chart, theme, size, &mut svg)?;
        } else {
            paint_radial(chart, theme, size, &mut svg)?;
        }
        Ok(RenderOutput { svg })
    }
}

/// Apply the axis scales to a chart's coordinates and (for `Horizontal` bar/area) swap the
/// x/y roles, producing a cartesian-ready working chart plus the scaled axes. Each point is
/// mapped through `x_axis.scale.forward`/`y_axis.scale.forward`; points the transform cannot
/// map (log of `<= 0`) are dropped. Sizes ride along for surviving points.
fn prepare_cartesian(chart: &ResolvedChart) -> (ResolvedChart, Axis, Axis) {
    let horizontal = matches!(chart.config.orientation, Some(Orientation::Horizontal))
        && matches!(chart.mark, Mark::Bar | Mark::Area);
    let (x_axis, y_axis) = if horizontal {
        (chart.y_axis.clone(), chart.x_axis.clone())
    } else {
        (chart.x_axis.clone(), chart.y_axis.clone())
    };
    let series = chart
        .series
        .iter()
        .map(|s| scale_series(s, x_axis.scale, y_axis.scale, horizontal))
        .collect();
    let working = ResolvedChart {
        mark: chart.mark,
        series,
        slices: Vec::new(),
        table: None,
        x_axis: x_axis.clone(),
        y_axis: y_axis.clone(),
        config: chart.config.clone(),
    };
    (working, x_axis, y_axis)
}

/// Map one series' points through the x/y scales, dropping any point the transform rejects.
/// When `horizontal`, the source `(x, y)` is emitted as `(y, x)` so categories run down the
/// y axis. Per-point sizes are kept aligned to the surviving points.
fn scale_series(s: &Series, x_scale: Scale, y_scale: Scale, horizontal: bool) -> Series {
    let mut points = Vec::with_capacity(s.points.len());
    let mut sizes = Vec::with_capacity(s.sizes.len());
    for (i, &(px, py)) in s.points.iter().enumerate() {
        let (sx, sy) = if horizontal { (py, px) } else { (px, py) };
        let (Some(fx), Some(fy)) = (x_scale.forward(sx), y_scale.forward(sy)) else {
            continue;
        };
        points.push((fx, fy));
        if let Some(&sz) = s.sizes.get(i) {
            sizes.push(sz);
        }
    }
    Series { name: s.name.clone(), points, sizes }
}

/// Drive plotters end to end for a cartesian mark: prepare scaled coordinates, fill the
/// background, set up the area and mesh, draw the mark, then the legend.
fn paint_cartesian(
    chart: &ResolvedChart,
    theme: &Theme,
    size: Size,
    svg: &mut String,
) -> Result<(), RenderError> {
    let (working, x_axis, y_axis) = prepare_cartesian(chart);
    let horizontal = matches!(working.config.orientation, Some(Orientation::Horizontal))
        && matches!(working.mark, Mark::Bar | Mark::Area);
    let stacked = working.config.stack && matches!(working.mark, Mark::Bar | Mark::Area);
    let (auto_x, auto_y) =
        data_ranges(&working.series, stacked, horizontal).ok_or(RenderError::Empty)?;
    let x_range = axis_range(&x_axis, auto_x);
    let y_range = axis_range(&y_axis, auto_y);

    let root = SVGBackend::with_string(svg, (size.width, size.height)).into_drawing_area();
    root.fill(&to_rgba(theme.background)).map_err(backend_err)?;
    let fg = to_rgba(theme.foreground);
    let mut builder = ChartBuilder::on(&root);
    builder.margin(14).x_label_area_size(42).y_label_area_size(54);
    if let Some(title) = working.config.title.as_deref() {
        builder.caption(title, (FONT, 22, &fg));
    }
    let mut ctx = builder.build_cartesian_2d(x_range, y_range).map_err(backend_err)?;
    configure_mesh(&mut ctx, &x_axis, &y_axis, theme, working.config.show_grid)?;
    draw_mark(&mut ctx, &working, theme)?;
    if working.config.legend && working.series.iter().any(|s| !s.name.is_empty()) {
        draw_legend(&mut ctx, theme)?;
    }
    root.present().map_err(backend_err)?;
    Ok(())
}

/// The drawn range for an axis: the explicit `scale.domain` (mapped through `forward`) when
/// set, else the auto data range optionally extended to include the scaled zero when
/// `scale.zero` is requested. Domains the transform rejects fall back to the auto range.
fn axis_range(axis: &Axis, auto: Range<f64>) -> Range<f64> {
    if let Some((lo, hi)) = axis.scale.domain
        && let (Some(flo), Some(fhi)) = (axis.scale.forward(lo), axis.scale.forward(hi))
    {
        return flo.min(fhi)..flo.max(fhi);
    }
    if axis.scale.zero && let Some(zero) = axis.scale.forward(0.0) {
        return auto.start.min(zero)..auto.end.max(zero);
    }
    auto
}

/// Configure axis lines, gridlines and tick labels from the theme and each axis's kind. The
/// per-axis tick formatter is built once here and passed to plotters as a `&dyn Fn`. When
/// `config.show_grid` is false the interior mesh is suppressed (gridline color set fully
/// transparent) while the axis lines, ticks and labels are kept.
fn configure_mesh(
    ctx: &mut Ctx<'_>,
    x_axis: &Axis,
    y_axis: &Axis,
    theme: &Theme,
    show_grid: bool,
) -> Result<(), RenderError> {
    let fg = to_rgba(theme.foreground);
    let grid = if show_grid {
        to_rgba(theme.gridline)
    } else {
        to_rgba(theme.gridline).mix(0.0)
    };
    let light = if show_grid { grid.mix(0.5) } else { grid };
    let x_fmt = tick_formatter(&x_axis.kind, x_axis.scale);
    let y_fmt = tick_formatter(&y_axis.kind, y_axis.scale);
    ctx.configure_mesh()
        .axis_style(fg)
        .bold_line_style(grid)
        .light_line_style(light)
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
    let horizontal = matches!(chart.config.orientation, Some(Orientation::Horizontal));
    match chart.mark {
        Mark::Line => draw_lines(ctx, chart, theme),
        Mark::Point => draw_points(ctx, chart, theme),
        Mark::Area => draw_areas(ctx, chart, theme),
        Mark::Bar if horizontal => draw_horizontal_bars(ctx, chart, theme),
        Mark::Bar => draw_bars(ctx, chart, theme),
        Mark::Histogram => draw_histogram(ctx, chart, theme),
        // Arc and Table are non-cartesian and never reach the cartesian dispatch.
        Mark::Arc | Mark::Table => Ok(()),
    }
}

/// Draw each series as a connected `LineSeries`. `config.line_width` sets the stroke (default
/// 2 px); `config.interpolate == Step` expands each series into a staircase first.
fn draw_lines(ctx: &mut Ctx<'_>, chart: &ResolvedChart, theme: &Theme) -> Result<(), RenderError> {
    let width = line_width(chart);
    let step = matches!(chart.config.interpolate, Some(Interpolate::Step));
    for (i, s) in chart.series.iter().enumerate() {
        let color = series_color(chart, theme, i);
        let pts = if step { step_points(&s.points) } else { s.points.clone() };
        let anno = ctx
            .draw_series(LineSeries::new(pts, stroke(color, width)))
            .map_err(backend_err)?;
        label_key(anno, &s.name, color);
    }
    Ok(())
}

/// Draw each series as one filled `Circle` per point (a scatter mark). With no size channel
/// every circle uses `config.point_size` (default 3 px). When a series carries `sizes` the
/// per-point radius is scaled from the size value into a sensible px range — a bubble chart.
fn draw_points(ctx: &mut Ctx<'_>, chart: &ResolvedChart, theme: &Theme) -> Result<(), RenderError> {
    let radius = point_radius(chart);
    let bubble = bubble_scale(&chart.series);
    for (i, s) in chart.series.iter().enumerate() {
        let color = series_color(chart, theme, i);
        let style: ShapeStyle = color.filled();
        let radii = point_radii(s, radius, bubble);
        let anno = ctx
            .draw_series(
                s.points
                    .iter()
                    .zip(radii)
                    .map(|(&p, r)| Circle::new(p, r, style)),
            )
            .map_err(backend_err)?;
        label_key(anno, &s.name, color);
    }
    Ok(())
}

/// The radius for each point of a series: the fixed `default` when no size channel is bound,
/// otherwise the size value mapped into `bubble` (min/max px) by the shared bubble scale.
fn point_radii(s: &Series, default: i32, bubble: Option<(f32, f32)>) -> Vec<i32> {
    if s.sizes.len() != s.points.len() {
        return vec![default; s.points.len()];
    }
    let Some((lo, hi)) = bubble else { return vec![default; s.points.len()] };
    s.sizes.iter().map(|&v| bubble_radius(v, lo, hi)).collect()
}

/// The min/max of all bound size values across the bubble series, or `None` when no series
/// carries sizes — the signal that points should use the fixed radius instead.
fn bubble_scale(series: &[Series]) -> Option<(f32, f32)> {
    let mut lo = f32::INFINITY;
    let mut hi = f32::NEG_INFINITY;
    for s in series {
        if s.sizes.len() != s.points.len() {
            continue;
        }
        for &v in &s.sizes {
            lo = lo.min(v);
            hi = hi.max(v);
        }
    }
    if lo.is_finite() && hi.is_finite() {
        Some((lo, hi))
    } else {
        None
    }
}

/// Minimum and maximum bubble radius in pixels — a sane visible range for size encoding.
const BUBBLE_MIN_PX: f32 = 3.0;
const BUBBLE_MAX_PX: f32 = 26.0;

/// Map a size value within `[lo, hi]` linearly to a pixel radius in the bubble range. A
/// degenerate range (all sizes equal) renders the midpoint radius.
fn bubble_radius(v: f32, lo: f32, hi: f32) -> i32 {
    let span = hi - lo;
    let t = if span > 0.0 { (v - lo) / span } else { 0.5 };
    t.mul_add(BUBBLE_MAX_PX - BUBBLE_MIN_PX, BUBBLE_MIN_PX).round() as i32
}

/// Draw each series as a translucent `AreaSeries` with a solid border. Fill alpha comes from
/// `config.fill_opacity` (default 0.35) and the border width from `config.line_width`. When
/// `config.stack` is set, each series fills the band from its cumulative baseline up to that
/// baseline plus its value, rather than overlapping down to a shared baseline.
fn draw_areas(ctx: &mut Ctx<'_>, chart: &ResolvedChart, theme: &Theme) -> Result<(), RenderError> {
    let alpha = fill_opacity(chart);
    let width = line_width(chart);
    if chart.config.stack {
        return draw_stacked_areas(ctx, chart, theme, alpha, width);
    }
    let baseline = area_baseline(&chart.series);
    for (i, s) in chart.series.iter().enumerate() {
        let color = series_color(chart, theme, i);
        let anno = ctx
            .draw_series(
                AreaSeries::new(s.points.iter().copied(), baseline, color.mix(alpha))
                    .border_style(stroke(color, width)),
            )
            .map_err(backend_err)?;
        label_key(anno, &s.name, color);
    }
    Ok(())
}

/// Draw stacked areas: each series is a filled band between its running cumulative baseline
/// and that baseline plus its own value, drawn as a closed polygon path so the bands stack.
fn draw_stacked_areas(
    ctx: &mut Ctx<'_>,
    chart: &ResolvedChart,
    theme: &Theme,
    alpha: f64,
    width: u32,
) -> Result<(), RenderError> {
    let baselines = stacked_baselines(&chart.series);
    for (i, s) in chart.series.iter().enumerate() {
        let color = series_color(chart, theme, i);
        let base = &baselines[i];
        let tops: Vec<(f64, f64)> = s
            .points
            .iter()
            .zip(base)
            .map(|(&(x, y), &b)| (x, b + y))
            .collect();
        let mut poly = tops.clone();
        poly.extend(s.points.iter().zip(base).rev().map(|(&(x, _), &b)| (x, b)));
        let fill: ShapeStyle = color.mix(alpha).filled();
        ctx.draw_series(std::iter::once(plotters::element::Polygon::new(poly, fill)))
            .map_err(backend_err)?;
        let anno = ctx
            .draw_series(LineSeries::new(tops, stroke(color, width)))
            .map_err(backend_err)?;
        label_key(anno, &s.name, color);
    }
    Ok(())
}

/// Draw vertical bars. Grouped by default (series sit side by side within each x slot); when
/// `config.stack` is set they stack on a running per-x cumulative baseline. Fill alpha comes
/// from `config.fill_opacity` (default 1.0, fully opaque).
fn draw_bars(ctx: &mut Ctx<'_>, chart: &ResolvedChart, theme: &Theme) -> Result<(), RenderError> {
    let alpha = bar_fill_opacity(chart);
    if chart.config.stack {
        return draw_stacked_bars(ctx, chart, theme, alpha);
    }
    let n = chart.series.len().max(1);
    let slot = bar_slot_width(&chart.series);
    let group = slot * 0.8;
    let bar_w = group / n as f64;
    for (i, s) in chart.series.iter().enumerate() {
        let color = series_color(chart, theme, i);
        let style: ShapeStyle = color.mix(alpha).filled();
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

/// Draw stacked bars: one full-width bar per x slot per series, each rising from its running
/// cumulative baseline to that baseline plus its value.
fn draw_stacked_bars(
    ctx: &mut Ctx<'_>,
    chart: &ResolvedChart,
    theme: &Theme,
    alpha: f64,
) -> Result<(), RenderError> {
    let slot = bar_slot_width(&chart.series);
    let half = slot * 0.4;
    let baselines = stacked_baselines(&chart.series);
    for (i, s) in chart.series.iter().enumerate() {
        let color = series_color(chart, theme, i);
        let style: ShapeStyle = color.mix(alpha).filled();
        let base = &baselines[i];
        let anno = ctx
            .draw_series(s.points.iter().zip(base).map(|(&(x, y), &b)| {
                Rectangle::new([(x - half, b), (x + half, b + y)], style)
            }))
            .map_err(backend_err)?;
        label_key(anno, &s.name, color);
    }
    Ok(())
}

/// Draw a histogram: one bar per pre-binned `(bin_center, count)` point, each spanning the
/// full bin width from the y baseline up to its count. The bin width is inferred from the
/// spacing of consecutive centers (the resolver lays them at equal-width centers).
fn draw_histogram(
    ctx: &mut Ctx<'_>,
    chart: &ResolvedChart,
    theme: &Theme,
) -> Result<(), RenderError> {
    let alpha = bar_fill_opacity(chart);
    let color = series_color(chart, theme, 0);
    let style: ShapeStyle = color.mix(alpha).filled();
    let width = bar_slot_width(&chart.series);
    let half = width * 0.5;
    for s in &chart.series {
        ctx.draw_series(s.points.iter().map(|&(x, count)| {
            Rectangle::new([(x - half, 0.0), (x + half, count)], style)
        }))
        .map_err(backend_err)?;
    }
    Ok(())
}

/// Draw horizontal bars: the working chart already has x/y swapped (categories on the y
/// axis), so each bar spans from the x baseline rightward to its value at the category's y
/// slot. Grouped when multi-series (bars offset within each y slot). Stacking is unchanged
/// from the vertical path because the swap happened before this point.
fn draw_horizontal_bars(
    ctx: &mut Ctx<'_>,
    chart: &ResolvedChart,
    theme: &Theme,
) -> Result<(), RenderError> {
    let alpha = bar_fill_opacity(chart);
    let n = chart.series.len().max(1);
    let slot = bar_slot_width_axis(&chart.series, false);
    let group = slot * 0.8;
    let bar_h = group / n as f64;
    for (i, s) in chart.series.iter().enumerate() {
        let color = series_color(chart, theme, i);
        let style: ShapeStyle = color.mix(alpha).filled();
        let bottom = bar_h.mul_add(i as f64, -group / 2.0);
        let anno = ctx
            .draw_series(s.points.iter().map(|&(value, y)| {
                Rectangle::new([(0.0, y + bottom), (value, y + bottom + bar_h)], style)
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

/// The point radius for scatter marks: `config.point_size` rounded to pixels, else the
/// default 3 px. Negative/NaN sizes fall back to the default.
fn point_radius(chart: &ResolvedChart) -> i32 {
    match chart.config.point_size {
        Some(p) if p.is_finite() && p > 0.0 => p.round() as i32,
        _ => 3,
    }
}

/// The stroke width for lines and shape borders: `config.line_width` rounded to pixels, else
/// the default 2 px.
fn line_width(chart: &ResolvedChart) -> u32 {
    match chart.config.line_width {
        Some(w) if w.is_finite() && w > 0.0 => w.round() as u32,
        _ => 2,
    }
}

/// The fill alpha for area fills: `config.fill_opacity` clamped to `0.0..=1.0`, else the
/// default 0.35 area mix.
fn fill_opacity(chart: &ResolvedChart) -> f64 {
    match chart.config.fill_opacity {
        Some(a) if a.is_finite() => f64::from(a.clamp(0.0, 1.0)),
        _ => 0.35,
    }
}

/// The fill alpha for bars: `config.fill_opacity` clamped to `0.0..=1.0`, else fully opaque
/// (the previous bar default).
fn bar_fill_opacity(chart: &ResolvedChart) -> f64 {
    match chart.config.fill_opacity {
        Some(a) if a.is_finite() => f64::from(a.clamp(0.0, 1.0)),
        _ => 1.0,
    }
}

/// Per-series cumulative baselines for stacking: `baselines[i][k]` is the sum of every
/// earlier series' value at the k-th x position, so series `i` is drawn from that baseline up.
/// Series are aligned on x (the resolver pads them), so positions are matched by index.
fn stacked_baselines(series: &[Series]) -> Vec<Vec<f64>> {
    let len = series.iter().map(|s| s.points.len()).max().unwrap_or(0);
    let mut running = vec![0.0_f64; len];
    let mut out = Vec::with_capacity(series.len());
    for s in series {
        out.push(running.clone());
        for (k, &(_, y)) in s.points.iter().enumerate() {
            running[k] += y;
        }
    }
    out
}

/// Expand a polyline into a staircase: between each pair of points insert the corner so the
/// value is held flat then steps vertically to the next (horizontal-then-vertical).
fn step_points(points: &[(f64, f64)]) -> Vec<(f64, f64)> {
    let mut out = Vec::with_capacity(points.len() * 2);
    let mut prev: Option<(f64, f64)> = None;
    for &(x, y) in points {
        if let Some((_, py)) = prev {
            out.push((x, py));
        }
        out.push((x, y));
        prev = Some((x, y));
    }
    out
}

/// Compute the `(x, y)` data ranges spanning every point across all series, padded so marks
/// are not flush against the axes. When `stacked`, the value extent uses the per-slot
/// cumulative totals so a stacked chart's top is not clipped. `value_is_x` is true for
/// horizontal bars (the value runs along x); that axis is the one zero-anchored and
/// stack-extended. Returns `None` when there is no drawable point at all — the
/// `RenderError::Empty` signal.
fn data_ranges(series: &[Series], stacked: bool, value_is_x: bool) -> Option<(Range<f64>, Range<f64>)> {
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
    if stacked {
        let (lo, hi) = stacked_value_extent(series, value_is_x);
        if value_is_x {
            x_min = lo;
            x_max = hi;
        } else {
            y_min = lo;
            y_max = hi;
        }
    }
    if !x_min.is_finite() || !y_min.is_finite() {
        return None;
    }
    if value_is_x {
        return Some((pad_y(x_min, x_max), pad(y_min, y_max)));
    }
    Some((pad(x_min, x_max), pad_y(y_min, y_max)))
}

/// The min/max of per-slot cumulative totals across all series (the stacked value extent).
/// `value_is_x` sums the x coordinate (horizontal bars) else the y. Positive and negative
/// stacks are summed separately so a mix of signs still bounds correctly.
fn stacked_value_extent(series: &[Series], value_is_x: bool) -> (f64, f64) {
    let len = series.iter().map(|s| s.points.len()).max().unwrap_or(0);
    let mut pos = vec![0.0_f64; len];
    let mut neg = vec![0.0_f64; len];
    for s in series {
        for (k, &(x, y)) in s.points.iter().enumerate() {
            let v = if value_is_x { x } else { y };
            if v >= 0.0 {
                pos[k] += v;
            } else {
                neg[k] += v;
            }
        }
    }
    let hi = pos.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let lo = neg.iter().copied().fold(f64::INFINITY, f64::min);
    if hi.is_finite() {
        (lo.min(0.0), hi)
    } else {
        (f64::INFINITY, f64::NEG_INFINITY)
    }
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

/// The width of one slot for grouped bars along the x coordinate (vertical bars).
fn bar_slot_width(series: &[Series]) -> f64 {
    bar_slot_width_axis(series, true)
}

/// The smallest gap between consecutive distinct positions along the chosen coordinate
/// (`use_x` picks x, else y), or `1.0` when there is a single position — the slot width for
/// grouped bars. Horizontal bars group along y, so they pass `use_x = false`.
fn bar_slot_width_axis(series: &[Series], use_x: bool) -> f64 {
    let mut vs: Vec<f64> = series
        .iter()
        .flat_map(|s| s.points.iter().map(|&(x, y)| if use_x { x } else { y }))
        .collect();
    vs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    vs.dedup();
    let mut min_gap = f64::INFINITY;
    for w in vs.windows(2) {
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
/// The tick value arrives in *scaled* space, so it is first mapped back through
/// `scale.inverse` to data space before formatting (a log axis labels 10/100/1000, not 1/2/3).
fn tick_formatter(kind: &AxisKind, scale: Scale) -> Box<dyn Fn(f64) -> String> {
    match kind {
        AxisKind::Quantitative => Box::new(move |v| format_number(scale.inverse(v))),
        AxisKind::Temporal => Box::new(move |v| format_epoch(scale.inverse(v) as i64)),
        AxisKind::Categorical(labels) => {
            let labels = labels.clone();
            Box::new(move |v| {
                let i = scale.inverse(v).round();
                if i >= 0.0 && (i as usize) < labels.len() {
                    labels[i as usize].clone()
                } else {
                    String::new()
                }
            })
        }
    }
}

/// Draw a radial `Arc` chart (pie/donut): fill the background, then paint each `Slice` as a
/// wedge over a pixel-space cartesian area with no mesh or axes, then a swatch legend from
/// the slice labels. Empty when there are no slices (`RenderError::Empty`).
fn paint_radial(
    chart: &ResolvedChart,
    theme: &Theme,
    size: Size,
    svg: &mut String,
) -> Result<(), RenderError> {
    if chart.slices.is_empty() {
        return Err(RenderError::Empty);
    }
    let root = SVGBackend::with_string(svg, (size.width, size.height)).into_drawing_area();
    root.fill(&to_rgba(theme.background)).map_err(backend_err)?;
    let fg = to_rgba(theme.foreground);
    let mut builder = ChartBuilder::on(&root);
    builder.margin(14);
    if let Some(title) = chart.config.title.as_deref() {
        builder.caption(title, (FONT, 22, &fg));
    }
    // A pixel-space cartesian area so wedge geometry is computed directly in device units.
    let w = f64::from(size.width);
    let h = f64::from(size.height);
    let mut ctx = builder.build_cartesian_2d(0.0..w, 0.0..h).map_err(backend_err)?;
    draw_slices(&mut ctx, chart, theme, w, h)?;
    if chart.config.legend {
        draw_arc_legend(&mut ctx, chart, theme)?;
    }
    root.present().map_err(backend_err)?;
    Ok(())
}

/// The geometry shared by every wedge: center, outer radius, and donut inner radius in px.
#[derive(Clone, Copy)]
struct ArcGeom {
    cx: f64,
    cy: f64,
    outer: f64,
    inner: f64,
}

/// Compute the arc layout from the canvas size and `config.inner_radius` (a 0..1 fraction of
/// the outer radius, clamped). The pie is centered with a margin so the legend has room.
fn arc_geom(chart: &ResolvedChart, w: f64, h: f64) -> ArcGeom {
    let outer = (w.min(h) * 0.42).max(1.0);
    let frac = f64::from(chart.config.inner_radius.unwrap_or(0.0).clamp(0.0, 0.95));
    ArcGeom { cx: w * 0.5, cy: h * 0.5, outer, inner: outer * frac }
}

/// Draw each slice as a filled wedge from cumulative start to end angle. The total drives the
/// angular span; slices are colored by their palette index.
fn draw_slices(
    ctx: &mut Ctx<'_>,
    chart: &ResolvedChart,
    theme: &Theme,
    w: f64,
    h: f64,
) -> Result<(), RenderError> {
    let geom = arc_geom(chart, w, h);
    let total: f64 = chart.slices.iter().map(|s| s.value).sum();
    if total <= 0.0 {
        return Err(RenderError::Empty);
    }
    let alpha = bar_fill_opacity(chart);
    let mut start = -std::f64::consts::FRAC_PI_2; // start at 12 o'clock
    for slice in &chart.slices {
        let sweep = slice.value / total * std::f64::consts::TAU;
        let end = start + sweep;
        let color = series_color(chart, theme, slice.color_index);
        let fill: ShapeStyle = color.mix(alpha).filled();
        let poly = wedge_polygon(geom, start, end);
        ctx.draw_series(std::iter::once(plotters::element::Polygon::new(poly, fill)))
            .map_err(backend_err)?;
        start = end;
    }
    Ok(())
}

/// The number of straight segments approximating a full circle; a wedge uses a proportional
/// share so even a thin slice stays smooth.
const ARC_SEGMENTS: usize = 180;

/// Build the polygon outline of one wedge between `start` and `end` radians: the outer arc
/// forward then the inner arc back (or the center for a full pie), approximated by segments.
fn wedge_polygon(geom: ArcGeom, start: f64, end: f64) -> Vec<(f64, f64)> {
    let span = (end - start).abs();
    let steps = ((span / std::f64::consts::TAU * ARC_SEGMENTS as f64).ceil() as usize).max(2);
    let mut poly = Vec::with_capacity(steps * 2 + 2);
    for k in 0..=steps {
        let a = start + (end - start) * (k as f64 / steps as f64);
        poly.push((geom.cx + geom.outer * a.cos(), geom.cy - geom.outer * a.sin()));
    }
    if geom.inner > 0.0 {
        for k in (0..=steps).rev() {
            let a = start + (end - start) * (k as f64 / steps as f64);
            poly.push((geom.cx + geom.inner * a.cos(), geom.cy - geom.inner * a.sin()));
        }
    } else {
        poly.push((geom.cx, geom.cy));
    }
    poly
}

/// Draw a simple swatch legend for the arc slices in the upper-left, since the radial chart
/// has no plotters series to drive the built-in series-label box.
fn draw_arc_legend(
    ctx: &mut Ctx<'_>,
    chart: &ResolvedChart,
    theme: &Theme,
) -> Result<(), RenderError> {
    let area = ctx.plotting_area();
    let fg = to_rgba(theme.foreground);
    let (range_x, range_y) = (area.get_x_range(), area.get_y_range());
    let left = range_x.start + (range_x.end - range_x.start) * 0.02;
    let top = range_y.end - (range_y.end - range_y.start) * 0.04;
    let line_h = (range_y.end - range_y.start) * 0.05;
    for (row, slice) in chart.slices.iter().enumerate() {
        let y = line_h.mul_add(-(row as f64), top);
        let color = series_color(chart, theme, slice.color_index);
        let swatch: ShapeStyle = color.filled();
        area.draw(&Rectangle::new(
            [(left, y), (left + line_h * 0.7, y - line_h * 0.7)],
            swatch,
        ))
        .map_err(backend_err)?;
        let label_x = left + line_h;
        let style = TextStyle::from((FONT, 14).into_font()).color(&fg);
        area.draw(&plotters::element::Text::new(
            slice.label.clone(),
            (label_x, y - line_h * 0.55),
            style,
        ))
        .map_err(backend_err)?;
    }
    Ok(())
}

/// Draw the `Table` mark as a grid: fill the background, optional caption, then a header
/// band plus one cell per value over a pixel-space area (no axes/mesh). Column widths are
/// proportional to content length; `transpose` flips fields to rows and records to columns.
/// Empty when the resolved table has no header cells (`RenderError::Empty`).
fn paint_table(
    chart: &ResolvedChart,
    theme: &Theme,
    size: Size,
    svg: &mut String,
) -> Result<(), RenderError> {
    let view = chart.table.as_ref().ok_or(RenderError::Empty)?;
    let grid = table_grid(view);
    if grid.cells.is_empty() || grid.cells[0].is_empty() {
        return Err(RenderError::Empty);
    }
    let root = SVGBackend::with_string(svg, (size.width, size.height)).into_drawing_area();
    root.fill(&to_rgba(theme.background)).map_err(backend_err)?;
    let fg = to_rgba(theme.foreground);
    let mut builder = ChartBuilder::on(&root);
    builder.margin(14);
    if let Some(title) = chart.config.title.as_deref() {
        builder.caption(title, (FONT, 22, &fg));
    }
    let w = f64::from(size.width);
    let h = f64::from(size.height);
    let ctx = builder.build_cartesian_2d(0.0..w, 0.0..h).map_err(backend_err)?;
    draw_grid(ctx.plotting_area(), &grid, theme, w, h)?;
    root.present().map_err(backend_err)?;
    Ok(())
}

/// A laid-out display grid: row-major `cells`, and which single row/column (if any) is the
/// styled header band — the top row in natural layout, the left column when transposed.
struct Grid {
    cells: Vec<Vec<String>>,
    header_row: Option<usize>,
    header_col: Option<usize>,
}

/// Build the display [`Grid`] from a resolved [`TableView`]: natural layout puts headers on the
/// top row above the record rows; transposed layout runs each field down the left column with
/// its values spread across.
fn table_grid(view: &TableView) -> Grid {
    if view.transpose {
        let cells = view
            .headers
            .iter()
            .enumerate()
            .map(|(c, head)| {
                let mut row = Vec::with_capacity(view.rows.len() + 1);
                row.push(head.clone());
                row.extend(view.rows.iter().map(|r| r.get(c).cloned().unwrap_or_default()));
                row
            })
            .collect();
        Grid { cells, header_row: None, header_col: Some(0) }
    } else {
        let mut cells = Vec::with_capacity(view.rows.len() + 1);
        cells.push(view.headers.clone());
        cells.extend(view.rows.iter().cloned());
        Grid { cells, header_row: Some(0), header_col: None }
    }
}

/// The drawing area type a grid is painted onto — the chart's pixel-space cartesian plotting
/// area, the same `ctx.plotting_area()` the arc legend draws onto.
type Area<'a> = plotters::drawing::DrawingArea<SVGBackend<'a>, Cartesian2d<RangedCoordf64, RangedCoordf64>>;

/// Paint a [`Grid`] into `area` (logical extent `w` x `h`, y increasing upward). Column widths
/// are proportional to the longest cell text in each column; rows share the height evenly. Each
/// cell gets a border; header cells get a filled band; text is left-padded in the foreground.
fn draw_grid(area: &Area<'_>, grid: &Grid, theme: &Theme, w: f64, h: f64) -> Result<(), RenderError> {
    let rows = grid.cells.len();
    let cols = grid.cells[0].len();
    let widths = column_widths(grid, w, cols);
    let mut x_starts = Vec::with_capacity(cols + 1);
    let mut acc = 0.0;
    for &cw in &widths {
        x_starts.push(acc);
        acc += cw;
    }
    x_starts.push(acc);
    let rh = h / rows as f64;
    let fg = to_rgba(theme.foreground);
    let border = stroke(to_rgba(theme.gridline), 1);
    // A subtle header band: the gridline color blended toward the foreground.
    let header_fill: ShapeStyle = to_rgba(theme.gridline).mix(0.45).filled();
    let font = (h / rows as f64 * 0.5).clamp(9.0, 16.0) as i32;

    for (r, row) in grid.cells.iter().enumerate() {
        let y_top = h - r as f64 * rh;
        let y_bot = y_top - rh;
        for (c, text) in row.iter().enumerate() {
            let x0 = x_starts[c];
            let x1 = x_starts[c + 1];
            let is_header = grid.header_row == Some(r) || grid.header_col == Some(c);
            if is_header {
                area.draw(&Rectangle::new([(x0, y_bot), (x1, y_top)], header_fill))
                    .map_err(backend_err)?;
            }
            area.draw(&Rectangle::new([(x0, y_bot), (x1, y_top)], border)).map_err(backend_err)?;
            let style = TextStyle::from((FONT, font).into_font()).color(&fg);
            let ty = y_top - rh * 0.5 - f64::from(font) * 0.5;
            area.draw(&plotters::element::Text::new(text.clone(), (x0 + rh * 0.2, ty), style))
                .map_err(backend_err)?;
        }
    }
    Ok(())
}

/// Column widths summing to `w`, each proportional to the longest cell (in chars) in that
/// column so wide text gets more room. A floor of one keeps an all-empty column visible.
fn column_widths(grid: &Grid, w: f64, cols: usize) -> Vec<f64> {
    let mut weights = vec![1.0_f64; cols];
    for row in &grid.cells {
        for (c, text) in row.iter().enumerate() {
            weights[c] = weights[c].max(text.chars().count() as f64);
        }
    }
    let total: f64 = weights.iter().sum();
    weights.iter().map(|x| w * x / total).collect()
}

/// Map a plotters drawing error into the core `RenderError::Backend` variant, flattening the
/// backend-specific error type to a string so it never leaks into a `pub` signature.
fn backend_err<E: std::fmt::Debug>(err: E) -> RenderError {
    RenderError::Backend(format!("{err:?}"))
}

#[cfg(test)]
mod tests;
