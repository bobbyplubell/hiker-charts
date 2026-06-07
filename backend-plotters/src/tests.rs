//! Structural render tests for `PlottersSvg`: build small `ResolvedChart`s by hand and
//! assert the SVG output is well-formed and contains the expected element kinds, plus the
//! empty-chart error path. No golden bytes — plotters' SVG is not byte-stable (SPEC §4.4).

use hiker_charts_core::backend::{
    Axis, AxisKind, Backend, RenderError, ResolvedChart, Series, Size,
};
use hiker_charts_core::dsl::{Config, Mark};
use hiker_charts_core::theme::Theme;

use super::PlottersSvg;

/// A standard render size for the tests.
fn size() -> Size {
    Size { width: 640, height: 480 }
}

/// Build a one-series chart of the given mark with a quantitative x and y axis.
fn single(mark: Mark, points: Vec<(f64, f64)>) -> ResolvedChart {
    ResolvedChart {
        mark,
        series: vec![Series { name: String::new(), points }],
        x_axis: Axis { title: "x".to_string(), kind: AxisKind::Quantitative },
        y_axis: Axis { title: "y".to_string(), kind: AxisKind::Quantitative },
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
            Series { name: "revenue".to_string(), points: vec![(0.0, 1.0), (1.0, 4.0)] },
            Series { name: "profit".to_string(), points: vec![(0.0, 0.5), (1.0, 2.0)] },
        ],
        x_axis: Axis { title: "month".to_string(), kind: AxisKind::Quantitative },
        y_axis: Axis { title: "value".to_string(), kind: AxisKind::Quantitative },
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
            Series { name: "a".to_string(), points: vec![(0.0, 2.0), (1.0, 3.0)] },
            Series { name: "b".to_string(), points: vec![(0.0, 1.0), (1.0, 4.0)] },
        ],
        x_axis: Axis { title: "g".to_string(), kind: AxisKind::Quantitative },
        y_axis: Axis { title: "v".to_string(), kind: AxisKind::Quantitative },
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
        }],
        x_axis: Axis {
            title: "month".to_string(),
            kind: AxisKind::Categorical(vec![
                "jan".to_string(),
                "feb".to_string(),
                "mar".to_string(),
            ]),
        },
        y_axis: Axis { title: "sales".to_string(), kind: AxisKind::Quantitative },
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
        }],
        x_axis: Axis { title: "date".to_string(), kind: AxisKind::Temporal },
        y_axis: Axis { title: "v".to_string(), kind: AxisKind::Quantitative },
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
            Series { name: "alpha".to_string(), points: vec![(0.0, 1.0), (1.0, 2.0)] },
            Series { name: "beta".to_string(), points: vec![(0.0, 2.0), (1.0, 1.0)] },
        ],
        x_axis: Axis { title: "x".to_string(), kind: AxisKind::Quantitative },
        y_axis: Axis { title: "y".to_string(), kind: AxisKind::Quantitative },
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
