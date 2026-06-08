//! Structural render tests for `PlottersSvg`: build small `ResolvedChart`s by hand and
//! assert the SVG output is well-formed and contains the expected element kinds, plus the
//! empty-chart error path. No golden bytes — plotters' SVG is not byte-stable (SPEC §4.4).

use hiker_charts_core::backend::{
    Axis, AxisKind, Backend, RenderError, ResolvedChart, Series, Size, Slice, TableView,
};
use hiker_charts_core::dsl::{Config, Interpolate, Mark, Orientation, Scale, ScaleKind};
use hiker_charts_core::theme::Theme;

use super::PlottersSvg;

/// A quantitative axis with the given title and the identity (linear) scale.
fn quant_axis(title: &str) -> Axis {
    Axis { title: title.to_string(), kind: AxisKind::Quantitative, scale: Scale::default() }
}

/// A two-series chart of the given mark, with both series aligned on the same x positions.
fn two_series(mark: Mark) -> ResolvedChart {
    ResolvedChart {
        mark,
        series: vec![
            Series {
                name: "a".to_string(),
                points: vec![(0.0, 2.0), (1.0, 3.0), (2.0, 1.0)],
                sizes: Vec::new(),
            },
            Series {
                name: "b".to_string(),
                points: vec![(0.0, 1.0), (1.0, 2.0), (2.0, 4.0)],
                sizes: Vec::new(),
            },
        ],
        slices: Vec::new(),
        table: None,
        x_axis: quant_axis("x"),
        y_axis: quant_axis("y"),
        config: Config::default(),
    }
}

/// A standard render size for the tests.
fn size() -> Size {
    Size { width: 640, height: 480 }
}

/// Build a one-series chart of the given mark with a quantitative x and y axis.
fn single(mark: Mark, points: Vec<(f64, f64)>) -> ResolvedChart {
    ResolvedChart {
        mark,
        series: vec![Series { name: String::new(), points, sizes: Vec::new() }],
        slices: Vec::new(),
        table: None,
        x_axis: quant_axis("x"),
        y_axis: quant_axis("y"),
        config: Config::default(),
    }
}

/// Render a chart with the default theme, panicking on error (the happy-path tests).
fn render_ok(chart: &ResolvedChart) -> String {
    PlottersSvg
        .render(chart, &Theme::default(), size())
        .expect("render should succeed")
        .svg
}

#[test]
fn line_chart_emits_svg_with_path() {
    let chart = single(Mark::Line, vec![(0.0, 1.0), (1.0, 3.0), (2.0, 2.0)]);
    let svg = render_ok(&chart);
    assert!(svg.contains("<svg"), "missing svg root");
    assert!(svg.contains("</svg>"), "svg not closed");
    assert!(svg.contains("<polyline") || svg.contains("<path"), "no line geometry");
}

#[test]
fn point_chart_emits_circles() {
    let chart = single(Mark::Point, vec![(0.0, 1.0), (1.0, 4.0), (2.0, 2.0)]);
    let svg = render_ok(&chart);
    assert!(svg.contains("<circle"), "scatter should emit circles");
}

#[test]
fn area_chart_emits_filled_geometry() {
    let chart = single(Mark::Area, vec![(0.0, 1.0), (1.0, 3.0), (2.0, 2.0)]);
    let svg = render_ok(&chart);
    assert!(svg.contains("<path") || svg.contains("<polygon"), "area needs a filled shape");
}

#[test]
fn bar_chart_emits_rectangles() {
    let chart = single(Mark::Bar, vec![(0.0, 2.0), (1.0, 5.0), (2.0, 3.0)]);
    let svg = render_ok(&chart);
    assert!(svg.contains("<rect"), "bars should emit rectangles");
}

#[test]
fn title_is_rendered_as_text() {
    let mut chart = single(Mark::Line, vec![(0.0, 1.0), (1.0, 2.0)]);
    chart.config.title = Some("Quarterly Revenue".to_string());
    let svg = render_ok(&chart);
    assert!(svg.contains("Quarterly Revenue"), "title text missing from svg");
}

#[test]
fn empty_chart_is_an_error() {
    let chart = single(Mark::Line, Vec::new());
    let err = PlottersSvg
        .render(&chart, &Theme::default(), size())
        .expect_err("empty chart must error");
    assert!(matches!(err, RenderError::Empty));
}

#[test]
fn chart_with_no_series_is_empty() {
    let mut chart = single(Mark::Bar, vec![(0.0, 1.0)]);
    chart.series.clear();
    let err = PlottersSvg
        .render(&chart, &Theme::default(), size())
        .expect_err("no series must error");
    assert!(matches!(err, RenderError::Empty));
}

#[test]
fn wide_multi_series_draws_all_and_legend() {
    let chart = ResolvedChart {
        mark: Mark::Line,
        series: vec![
            Series { name: "revenue".to_string(), points: vec![(0.0, 1.0), (1.0, 4.0)], sizes: Vec::new() },
            Series { name: "profit".to_string(), points: vec![(0.0, 0.5), (1.0, 2.0)], sizes: Vec::new() },
        ],
        slices: Vec::new(),
        table: None,
        x_axis: quant_axis("month"),
        y_axis: quant_axis("value"),
        config: Config::default(),
    };
    let svg = render_ok(&chart);
    assert!(svg.contains("revenue"), "first series name should appear in the legend");
    assert!(svg.contains("profit"), "second series name should appear in the legend");
}

#[test]
fn grouped_bars_render_for_multiple_series() {
    let chart = ResolvedChart {
        mark: Mark::Bar,
        series: vec![
            Series { name: "a".to_string(), points: vec![(0.0, 2.0), (1.0, 3.0)], sizes: Vec::new() },
            Series { name: "b".to_string(), points: vec![(0.0, 1.0), (1.0, 4.0)], sizes: Vec::new() },
        ],
        slices: Vec::new(),
        table: None,
        x_axis: quant_axis("g"),
        y_axis: quant_axis("v"),
        config: Config::default(),
    };
    let svg = render_ok(&chart);
    assert!(svg.contains("<rect"), "grouped bars still emit rectangles");
    assert!(svg.contains('a') && svg.contains('b'), "both series labelled");
}

#[test]
fn categorical_axis_labels_appear() {
    let chart = ResolvedChart {
        mark: Mark::Bar,
        series: vec![Series {
            name: String::new(),
            points: vec![(0.0, 10.0), (1.0, 20.0), (2.0, 15.0)],
            sizes: Vec::new(),
        }],
        slices: Vec::new(),
        table: None,
        x_axis: Axis {
            title: "month".to_string(),
            kind: AxisKind::Categorical(vec![
                "jan".to_string(),
                "feb".to_string(),
                "mar".to_string(),
            ]),
            scale: Scale::default(),
        },
        y_axis: quant_axis("sales"),
        config: Config::default(),
    };
    let svg = render_ok(&chart);
    assert!(svg.contains("jan"), "categorical tick label jan missing");
    assert!(svg.contains("feb"), "categorical tick label feb missing");
}

#[test]
fn temporal_axis_formats_dates() {
    // 2021-01-01 and 2021-07-01 in epoch seconds (UTC).
    let chart = ResolvedChart {
        mark: Mark::Line,
        series: vec![Series {
            name: String::new(),
            points: vec![(1_609_459_200.0, 100.0), (1_625_097_600.0, 140.0)],
            sizes: Vec::new(),
        }],
        slices: Vec::new(),
        table: None,
        x_axis: Axis {
            title: "date".to_string(),
            kind: AxisKind::Temporal,
            scale: Scale::default(),
        },
        y_axis: quant_axis("v"),
        config: Config::default(),
    };
    let svg = render_ok(&chart);
    assert!(svg.contains("2021"), "temporal axis should show a year");
}

#[test]
fn palette_override_is_used() {
    let mut chart = single(Mark::Line, vec![(0.0, 1.0), (1.0, 2.0)]);
    chart.config.palette = Some(vec!["#ff0000".to_string()]);
    let svg = render_ok(&chart);
    // plotters writes stroke colors as `#RRGGBB`; the override red must surface.
    assert!(svg.to_uppercase().contains("FF0000"), "palette override color missing");
}

#[test]
fn legend_disabled_hides_names() {
    let mut chart = ResolvedChart {
        mark: Mark::Line,
        series: vec![
            Series { name: "alpha".to_string(), points: vec![(0.0, 1.0), (1.0, 2.0)], sizes: Vec::new() },
            Series { name: "beta".to_string(), points: vec![(0.0, 2.0), (1.0, 1.0)], sizes: Vec::new() },
        ],
        slices: Vec::new(),
        table: None,
        x_axis: quant_axis("x"),
        y_axis: quant_axis("y"),
        config: Config::default(),
    };
    chart.config.legend = false;
    let svg = render_ok(&chart);
    assert!(!svg.contains("alpha"), "legend off should not draw series names");
}

#[test]
fn epoch_formatter_is_inverse_of_iso_date() {
    // 1970-01-01 is epoch 0; verify our civil-date math directly.
    assert_eq!(super::format_epoch(0), "1970-01-01");
    assert_eq!(super::format_epoch(1_609_459_200), "2021-01-01");
}

#[test]
fn stacked_bars_differ_from_grouped() {
    let grouped = render_ok(&two_series(Mark::Bar));
    let mut chart = two_series(Mark::Bar);
    chart.config.stack = true;
    let stacked = render_ok(&chart);
    assert!(stacked.contains("<rect"), "stacked bars still emit rectangles");
    assert_ne!(grouped, stacked, "stacking must change the rendered geometry");
}

#[test]
fn stacked_areas_differ_from_overlaid() {
    let overlaid = render_ok(&two_series(Mark::Area));
    let mut chart = two_series(Mark::Area);
    chart.config.stack = true;
    let stacked = render_ok(&chart);
    assert_ne!(overlaid, stacked, "stacked areas must differ from overlaid");
}

#[test]
fn step_line_differs_from_linear() {
    let linear = render_ok(&single(Mark::Line, vec![(0.0, 1.0), (1.0, 3.0), (2.0, 2.0)]));
    let mut chart = single(Mark::Line, vec![(0.0, 1.0), (1.0, 3.0), (2.0, 2.0)]);
    chart.config.interpolate = Some(Interpolate::Step);
    let step = render_ok(&chart);
    assert_ne!(linear, step, "step interpolation must change the line path");
}

#[test]
fn show_grid_false_changes_svg() {
    let with_grid = render_ok(&single(Mark::Line, vec![(0.0, 1.0), (1.0, 2.0), (2.0, 3.0)]));
    let mut chart = single(Mark::Line, vec![(0.0, 1.0), (1.0, 2.0), (2.0, 3.0)]);
    chart.config.show_grid = false;
    let no_grid = render_ok(&chart);
    assert_ne!(with_grid, no_grid, "disabling the grid must change the svg");
    // Axis labels are kept either way.
    assert!(no_grid.contains("<text"), "axis labels stay when grid is off");
}

#[test]
fn point_size_affects_output() {
    let small = render_ok(&single(Mark::Point, vec![(0.0, 1.0), (1.0, 2.0)]));
    let mut chart = single(Mark::Point, vec![(0.0, 1.0), (1.0, 2.0)]);
    chart.config.point_size = Some(12.0);
    let large = render_ok(&chart);
    assert_ne!(small, large, "a larger point size must change the circles");
}

#[test]
fn line_width_affects_output() {
    let thin = render_ok(&single(Mark::Line, vec![(0.0, 1.0), (1.0, 2.0)]));
    let mut chart = single(Mark::Line, vec![(0.0, 1.0), (1.0, 2.0)]);
    chart.config.line_width = Some(8.0);
    let thick = render_ok(&chart);
    assert_ne!(thin, thick, "a wider stroke must change the line");
}

#[test]
fn fill_opacity_affects_area_output() {
    let default = render_ok(&single(Mark::Area, vec![(0.0, 1.0), (1.0, 3.0), (2.0, 2.0)]));
    let mut chart = single(Mark::Area, vec![(0.0, 1.0), (1.0, 3.0), (2.0, 2.0)]);
    chart.config.fill_opacity = Some(0.9);
    let opaque = render_ok(&chart);
    assert_ne!(default, opaque, "fill opacity must change the area fill");
}

#[test]
fn histogram_emits_bars() {
    let chart = single(Mark::Histogram, vec![(0.5, 3.0), (1.5, 5.0), (2.5, 2.0)]);
    let svg = render_ok(&chart);
    assert!(svg.contains("<rect"), "histogram should emit bar rectangles");
}

#[test]
fn horizontal_bars_differ_from_vertical() {
    let vertical = render_ok(&single(Mark::Bar, vec![(0.0, 2.0), (1.0, 5.0), (2.0, 3.0)]));
    let mut chart = single(Mark::Bar, vec![(0.0, 2.0), (1.0, 5.0), (2.0, 3.0)]);
    chart.config.orientation = Some(Orientation::Horizontal);
    let horizontal = render_ok(&chart);
    assert!(horizontal.contains("<rect"), "horizontal bars still emit rectangles");
    assert_ne!(vertical, horizontal, "orientation must change the geometry");
}

#[test]
fn bubble_sizes_change_circle_radii() {
    let plain = render_ok(&single(Mark::Point, vec![(0.0, 1.0), (1.0, 2.0), (2.0, 3.0)]));
    let mut chart = single(Mark::Point, vec![(0.0, 1.0), (1.0, 2.0), (2.0, 3.0)]);
    chart.series[0].sizes = vec![1.0, 50.0, 100.0];
    let bubble = render_ok(&chart);
    assert!(bubble.contains("<circle"), "bubble chart still emits circles");
    assert_ne!(plain, bubble, "per-point sizes must vary the radii");
}

#[test]
fn log_scale_render_differs_from_linear() {
    let linear = render_ok(&single(Mark::Line, vec![(1.0, 10.0), (2.0, 100.0), (3.0, 1000.0)]));
    let mut chart = single(Mark::Line, vec![(1.0, 10.0), (2.0, 100.0), (3.0, 1000.0)]);
    chart.y_axis.scale = Scale { kind: ScaleKind::Log, domain: None, zero: false };
    let log = render_ok(&chart);
    assert_ne!(linear, log, "a log y scale must change the rendered line");
}

#[test]
fn arc_renders_wedges_and_legend() {
    let chart = ResolvedChart {
        mark: Mark::Arc,
        series: Vec::new(),
        slices: vec![
            Slice { label: "north".to_string(), value: 30.0, color_index: 0 },
            Slice { label: "south".to_string(), value: 10.0, color_index: 1 },
            Slice { label: "east".to_string(), value: 20.0, color_index: 2 },
        ],
        table: None,
        x_axis: quant_axis(""),
        y_axis: quant_axis(""),
        config: Config::default(),
    };
    let svg = render_ok(&chart);
    assert!(svg.contains("<svg"), "arc must emit an svg");
    assert!(svg.contains("<polygon") || svg.contains("<path"), "wedges need filled shapes");
    assert!(svg.contains("north"), "slice label should appear in the legend");
}

#[test]
fn arc_with_no_slices_is_empty() {
    let chart = ResolvedChart {
        mark: Mark::Arc,
        series: Vec::new(),
        slices: Vec::new(),
        table: None,
        x_axis: quant_axis(""),
        y_axis: quant_axis(""),
        config: Config::default(),
    };
    let err = PlottersSvg
        .render(&chart, &Theme::default(), size())
        .expect_err("an arc with no slices must error");
    assert!(matches!(err, RenderError::Empty));
}

#[test]
fn donut_inner_radius_differs_from_pie() {
    let pie = ResolvedChart {
        mark: Mark::Arc,
        series: Vec::new(),
        slices: vec![
            Slice { label: "a".to_string(), value: 1.0, color_index: 0 },
            Slice { label: "b".to_string(), value: 1.0, color_index: 1 },
        ],
        table: None,
        x_axis: quant_axis(""),
        y_axis: quant_axis(""),
        config: Config::default(),
    };
    let pie_svg = render_ok(&pie);
    let mut donut = pie.clone();
    donut.config.inner_radius = Some(0.5);
    let donut_svg = render_ok(&donut);
    assert_ne!(pie_svg, donut_svg, "a donut hole must change the wedge geometry");
}

/// A small resolved table: two columns, two rows, in natural orientation.
fn table_chart(transpose: bool) -> ResolvedChart {
    ResolvedChart {
        mark: Mark::Table,
        series: Vec::new(),
        slices: Vec::new(),
        table: Some(TableView {
            headers: vec!["month".to_string(), "revenue".to_string()],
            rows: vec![
                vec!["jan".to_string(), "100".to_string()],
                vec!["feb".to_string(), "140".to_string()],
            ],
            transpose,
        }),
        x_axis: quant_axis(""),
        y_axis: quant_axis(""),
        config: Config::default(),
    }
}

#[test]
fn table_renders_grid_with_headers_and_cells() {
    let svg = render_ok(&table_chart(false));
    assert!(svg.contains("<svg"), "table must emit an svg");
    assert!(svg.contains("<rect"), "grid cells need rectangles");
    for needle in ["month", "revenue", "jan", "140"] {
        assert!(svg.contains(needle), "table svg should contain `{needle}`");
    }
}

#[test]
fn transpose_changes_table_layout() {
    let natural = render_ok(&table_chart(false));
    let transposed = render_ok(&table_chart(true));
    assert!(transposed.contains("month") && transposed.contains("140"));
    assert_ne!(natural, transposed, "transposing must change the drawn grid");
}

#[test]
fn table_with_no_view_is_empty() {
    let mut chart = table_chart(false);
    chart.table = None;
    let err = PlottersSvg
        .render(&chart, &Theme::default(), size())
        .expect_err("a table mark without a TableView must error");
    assert!(matches!(err, RenderError::Empty));
}
