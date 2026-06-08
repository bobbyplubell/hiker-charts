//! Lower a `ChartSpec` plus a `Table` into a renderer-neutral `ResolvedChart`.
//!
//! This is where wide (`y: [a, b]`) and long (`y: v` + `color: c`) data collapse to
//! the same `Vec<Series>` (SPEC §2.3.1), so a backend never sees the original shape.
//! The x channel becomes `f64` (quantitative value, temporal epoch seconds, or a
//! stable categorical index), rows with a missing x or y are dropped with a
//! diagnostic, and axis kinds plus label maps are recorded for tick formatting.

use crate::backend::{Axis, AxisKind, ResolvedChart, Series, Slice, TableView};
use crate::data::Table;
use crate::diag::Diagnostic;
use crate::dsl::{ChartSpec, DataType, Mark, Scale};
use crate::display::format_value;
use crate::registry::caps;
use crate::typing::{coerce, infer_type, Value};

/// The default histogram bucket count when `config.bins` is unset (IMPLEMENTATION §17.4).
const DEFAULT_BINS: usize = 20;

/// Resolve a spec and table to a renderer-neutral chart, or a list of errors.
/// Errors are returned when a required channel is absent or a referenced column is
/// missing; dropped rows yield warnings on the returned chart's behalf (logged via
/// the error path only when they leave no drawable data). The mark drives the lowering:
/// `Arc` aggregates radial slices, `Histogram` bins the x column, and the cartesian marks
/// build `(x, y)` series. Bound channels are validated against the mark's registry caps.
pub fn resolve(spec: &ChartSpec, table: &Table) -> Result<ResolvedChart, Vec<Diagnostic>> {
    let mut diags = Vec::new();
    validate_channels(spec, &mut diags);
    if spec.mark == Mark::Arc {
        return resolve_arc(spec, table, diags);
    }
    if spec.mark == Mark::Histogram {
        return resolve_histogram(spec, table, diags);
    }
    if spec.mark == Mark::Table {
        return resolve_table(spec, table, diags);
    }
    resolve_cartesian(spec, table, diags)
}

/// Resolve the `Table` mark: select columns (from `spec.columns`, defaulting to every column in
/// natural order), coerce each cell by its inferred type, and format it to a display string so
/// the backend only lays out text. Unknown column names are dropped with a warning. Errors when
/// no column resolves (nothing to draw). The transpose flag rides along on the `TableView`.
fn resolve_table(
    spec: &ChartSpec,
    table: &Table,
    mut diags: Vec<Diagnostic>,
) -> Result<ResolvedChart, Vec<Diagnostic>> {
    let chosen: Vec<&str> = match &spec.columns {
        Some(names) => names
            .iter()
            .filter(|name| {
                let present = table.column(name).is_some();
                if !present {
                    diags.push(Diagnostic::warning(format!(
                        "table column `{name}` is not in the data and was dropped"
                    )));
                }
                present
            })
            .map(String::as_str)
            .collect(),
        None => table.columns.iter().map(|c| c.name.as_str()).collect(),
    };
    if chosen.is_empty() {
        diags.push(Diagnostic::error("table has no columns to show"));
        return Err(diags);
    }

    // Pre-coerce each chosen column once (inferred type per column), formatting cells as we go.
    let formatted: Vec<Vec<String>> = chosen
        .iter()
        .map(|name| {
            let col = table.column(name).expect("presence checked above");
            let ty = infer_type(&col.cells);
            col.cells.iter().map(|cell| format_value(&coerce(cell, ty))).collect()
        })
        .collect();

    let headers: Vec<String> = chosen.iter().map(|s| (*s).to_string()).collect();
    let row_count = formatted.iter().map(Vec::len).max().unwrap_or(0);
    let rows: Vec<Vec<String>> = (0..row_count)
        .map(|r| formatted.iter().map(|col| col.get(r).cloned().unwrap_or_default()).collect())
        .collect();

    Ok(ResolvedChart {
        mark: spec.mark,
        series: Vec::new(),
        slices: Vec::new(),
        table: Some(TableView { headers, rows, transpose: spec.config.transpose }),
        x_axis: empty_axis(),
        y_axis: empty_axis(),
        config: spec.config.clone(),
    })
}

/// Resolve a cartesian mark (`Bar`/`Line`/`Point`/`Area`): x + y series, sizes, and scaled
/// axes. Pulled out of `resolve` so the top-level dispatch stays a terse three-way branch.
fn resolve_cartesian(
    spec: &ChartSpec,
    table: &Table,
    mut diags: Vec<Diagnostic>,
) -> Result<ResolvedChart, Vec<Diagnostic>> {
    let x = match resolve_x(spec, table, &mut diags) {
        Some(x) => x,
        None => return Err(diags),
    };
    let mut series = match build_series(spec, table, &x, &mut diags) {
        Some(series) => series,
        None => return Err(diags),
    };
    fill_sizes(spec, table, &x, &mut series, &mut diags);
    warn_scale_drops(spec, &series, &mut diags);
    let x_axis = Axis {
        title: spec.config.x_title.clone().unwrap_or_else(|| x.field.clone()),
        kind: x.kind,
        scale: spec.config.x_scale.unwrap_or_default(),
    };
    let y_axis = Axis {
        title: spec.config.y_title.clone().unwrap_or_else(|| y_axis_title(spec)),
        kind: AxisKind::Quantitative,
        scale: spec.config.y_scale.unwrap_or_default(),
    };
    Ok(ResolvedChart {
        mark: spec.mark,
        series,
        slices: Vec::new(),
        table: None,
        x_axis,
        y_axis,
        config: spec.config.clone(),
    })
}

/// Warn when a bound channel is not in the mark's capability set (SPEC §2.2): e.g. a `size`
/// channel on a `Bar`, or a `theta` channel on a cartesian mark. Forward-compat — the value
/// stays in the spec, it is simply ignored by the resolver and the panel hides its control.
fn validate_channels(spec: &ChartSpec, diags: &mut Vec<Diagnostic>) {
    let ch = caps(spec.mark).channels;
    let mark = spec.mark;
    let mut warn = |bound: bool, allowed: bool, name: &str| {
        if bound && !allowed {
            diags.push(Diagnostic::warning(format!(
                "channel `{name}` is ignored by mark `{mark:?}`"
            )));
        }
    };
    warn(spec.x.is_some(), ch.x, "x");
    warn(spec.y.is_some(), ch.y, "y");
    warn(spec.color.is_some(), ch.color, "color");
    warn(spec.size.is_some(), ch.size, "size");
    warn(spec.theta.is_some(), ch.theta, "theta");
}

/// Fill the (single) series' `sizes` from the size channel when the mark accepts it
/// (`Point`). The size column is coerced quantitative; one value per *surviving* point,
/// aligned to the rows that produced the points (those with both x and y present — the same
/// filter `zip_points` applies). Left empty when no size channel is bound or unsupported.
fn fill_sizes(
    spec: &ChartSpec,
    table: &Table,
    x: &ResolvedX,
    series: &mut [Series],
    diags: &mut Vec<Diagnostic>,
) {
    let Some((field, _)) = spec.size_field() else { return };
    if !caps(spec.mark).channels.size {
        return;
    }
    let Some(column) = table.column(field) else {
        diags.push(Diagnostic::warning(format!("size column `{field}` not found in data")));
        return;
    };
    // A size channel only maps one row per point in the single-series case; it applies to
    // the first series and is ignored under a color/wide split (multi-series bubble is out
    // of scope per IMPLEMENTATION §17.4).
    let (y_field, _) = spec.y_fields().into_iter().next().unwrap_or(("", None));
    let Some(y_col) = table.column(y_field) else { return };
    let Some(s) = series.first_mut() else { return };
    let mut sizes = Vec::with_capacity(s.points.len());
    for (i, x_val) in x.values.iter().enumerate() {
        let y_present = as_number(y_col.cells.get(i).map_or("", String::as_str)).is_some();
        if x_val.is_some() && y_present {
            let cell = column.cells.get(i).map_or("", String::as_str);
            sizes.push(as_number(cell).unwrap_or(0.0) as f32);
        }
    }
    if sizes.len() == s.points.len() {
        s.sizes = sizes;
    }
}

/// The resolved x channel: its source field name, per-row `f64` values (or `None`
/// for a missing cell), and the axis kind (carrying categorical labels).
struct ResolvedX {
    field: String,
    values: Vec<Option<f64>>,
    kind: AxisKind,
}

/// Resolve the x channel: locate the column, determine its type, and map each cell
/// to an `f64`. Pushes an error and returns `None` if x is unbound or missing.
fn resolve_x(spec: &ChartSpec, table: &Table, diags: &mut Vec<Diagnostic>) -> Option<ResolvedX> {
    let (field, declared) = match spec.x_field() {
        Some(parts) => parts,
        None => {
            diags.push(Diagnostic::error("chart spec has no `x` channel"));
            return None;
        }
    };
    let column = match table.column(field) {
        Some(col) => col,
        None => {
            diags.push(Diagnostic::error(format!("x column `{field}` not found in data")));
            return None;
        }
    };
    let ty = declared.unwrap_or_else(|| infer_type(&column.cells));
    let (values, kind) = map_axis(&column.cells, ty);
    Some(ResolvedX {
        field: field.to_string(),
        values,
        kind,
    })
}

/// Map a column's cells to `f64` axis coordinates plus the matching `AxisKind`.
/// Categorical values get stable indices `0..n` in first-seen order, with the
/// label vector recorded on the kind.
fn map_axis(cells: &[String], ty: DataType) -> (Vec<Option<f64>>, AxisKind) {
    match ty {
        DataType::Quantitative => (
            cells.iter().map(|c| as_number(c)).collect(),
            AxisKind::Quantitative,
        ),
        DataType::Temporal => (
            cells.iter().map(|c| as_time(c)).collect(),
            AxisKind::Temporal,
        ),
        DataType::Ordinal | DataType::Nominal => map_categorical(cells),
    }
}

/// Build categorical indices and labels in stable first-seen order.
fn map_categorical(cells: &[String]) -> (Vec<Option<f64>>, AxisKind) {
    let mut labels: Vec<String> = Vec::new();
    let mut values: Vec<Option<f64>> = Vec::with_capacity(cells.len());
    for cell in cells {
        match coerce(cell, DataType::Nominal) {
            Value::Category(label) => {
                let idx = labels.iter().position(|l| *l == label).unwrap_or_else(|| {
                    labels.push(label);
                    labels.len() - 1
                });
                values.push(Some(idx as f64));
            }
            _ => values.push(None),
        }
    }
    (values, AxisKind::Categorical(labels))
}

/// Coerce one cell to a quantitative `f64`, or `None` if missing/unparseable.
fn as_number(cell: &str) -> Option<f64> {
    match coerce(cell, DataType::Quantitative) {
        Value::Number(n) => Some(n),
        _ => None,
    }
}

/// Coerce one cell to a temporal `f64` (epoch seconds), or `None` if unparseable.
fn as_time(cell: &str) -> Option<f64> {
    match coerce(cell, DataType::Temporal) {
        Value::Time(t) => Some(t as f64),
        _ => None,
    }
}

/// Build the series for either wide or long encodings. Returns `None` (with an
/// error pushed) when no y is bound, a y column is missing, or nothing is drawable.
fn build_series(
    spec: &ChartSpec,
    table: &Table,
    x: &ResolvedX,
    diags: &mut Vec<Diagnostic>,
) -> Option<Vec<Series>> {
    let y_fields = spec.y_fields();
    if y_fields.is_empty() {
        diags.push(Diagnostic::error("chart spec has no `y` channel"));
        return None;
    }
    let series = if y_fields.len() > 1 {
        wide_series(table, x, &y_fields, diags)?
    } else if let Some((color_field, _)) = spec.color_field() {
        long_series(table, x, y_fields[0].0, color_field, diags)?
    } else {
        single_series(table, x, y_fields[0].0, diags)?
    };
    if series.iter().all(|s| s.points.is_empty()) {
        diags.push(Diagnostic::error("no drawable points after coercion"));
        return None;
    }
    Some(series)
}

/// Wide encoding: one series per named y column, series name = column name.
fn wide_series(
    table: &Table,
    x: &ResolvedX,
    y_fields: &[(&str, Option<DataType>)],
    diags: &mut Vec<Diagnostic>,
) -> Option<Vec<Series>> {
    let mut series = Vec::with_capacity(y_fields.len());
    for (field, _) in y_fields {
        let column = match table.column(field) {
            Some(col) => col,
            None => {
                diags.push(Diagnostic::error(format!("y column `{field}` not found in data")));
                return None;
            }
        };
        let points = zip_points(x, &column.cells, diags, field);
        series.push(Series {
            name: (*field).to_string(),
            points,
            sizes: Vec::new(),
        });
    }
    Some(series)
}

/// Single-series encoding: one y column, no color split.
fn single_series(
    table: &Table,
    x: &ResolvedX,
    y_field: &str,
    diags: &mut Vec<Diagnostic>,
) -> Option<Vec<Series>> {
    let column = match table.column(y_field) {
        Some(col) => col,
        None => {
            diags.push(Diagnostic::error(format!("y column `{y_field}` not found in data")));
            return None;
        }
    };
    let points = zip_points(x, &column.cells, diags, y_field);
    Some(vec![Series {
        name: y_field.to_string(),
        points,
        sizes: Vec::new(),
    }])
}

/// Long encoding: group rows by the distinct value of the color column; one series
/// per group, series name = the category value (groups in first-seen order).
fn long_series(
    table: &Table,
    x: &ResolvedX,
    y_field: &str,
    color_field: &str,
    diags: &mut Vec<Diagnostic>,
) -> Option<Vec<Series>> {
    let y_col = match table.column(y_field) {
        Some(col) => col,
        None => {
            diags.push(Diagnostic::error(format!("y column `{y_field}` not found in data")));
            return None;
        }
    };
    let color_col = match table.column(color_field) {
        Some(col) => col,
        None => {
            diags.push(Diagnostic::error(format!("color column `{color_field}` not found in data")));
            return None;
        }
    };
    let mut names: Vec<String> = Vec::new();
    let mut groups: Vec<Vec<(f64, f64)>> = Vec::new();
    for (i, x_val) in x.values.iter().enumerate() {
        let key = color_col.cells.get(i).map(String::as_str).unwrap_or("");
        let y_val = as_number(y_col.cells.get(i).map(String::as_str).unwrap_or(""));
        let (Some(xv), Some(yv)) = (x_val, y_val) else {
            diags.push(Diagnostic::warning(format!("dropped row {i}: missing x or y")));
            continue;
        };
        let slot = names.iter().position(|n| n == key).unwrap_or_else(|| {
            names.push(key.to_string());
            groups.push(Vec::new());
            names.len() - 1
        });
        groups[slot].push((*xv, yv));
    }
    Some(
        names
            .into_iter()
            .zip(groups)
            .map(|(name, points)| Series { name, points, sizes: Vec::new() })
            .collect(),
    )
}

/// Zip the resolved x values against a y column's cells into drawable points,
/// dropping (and warning about) any row with a missing x or y.
fn zip_points(
    x: &ResolvedX,
    y_cells: &[String],
    diags: &mut Vec<Diagnostic>,
    field: &str,
) -> Vec<(f64, f64)> {
    let mut points = Vec::new();
    for (i, x_val) in x.values.iter().enumerate() {
        let y_val = as_number(y_cells.get(i).map(String::as_str).unwrap_or(""));
        match (x_val, y_val) {
            (Some(xv), Some(yv)) => points.push((*xv, yv)),
            _ => diags.push(Diagnostic::warning(format!(
                "dropped row {i} for series `{field}`: missing x or y"
            ))),
        }
    }
    points
}

/// Warn when a non-linear axis scale will silently drop points the backend cannot transform
/// (log of `<= 0`, sqrt of `< 0`). The backend's `Scale::forward` is the actual filter; this
/// surfaces the loss to the author per IMPLEMENTATION §17.4.
fn warn_scale_drops(spec: &ChartSpec, series: &[Series], diags: &mut Vec<Diagnostic>) {
    let x_scale = spec.config.x_scale.unwrap_or_default();
    let y_scale = spec.config.y_scale.unwrap_or_default();
    let mut x_drops = 0_usize;
    let mut y_drops = 0_usize;
    for s in series {
        for &(x, y) in &s.points {
            if x_scale.forward(x).is_none() {
                x_drops += 1;
            }
            if y_scale.forward(y).is_none() {
                y_drops += 1;
            }
        }
    }
    if x_drops > 0 {
        diags.push(Diagnostic::warning(format!(
            "x scale drops {x_drops} non-positive point(s) the transform cannot map"
        )));
    }
    if y_drops > 0 {
        diags.push(Diagnostic::warning(format!(
            "y scale drops {y_drops} non-positive point(s) the transform cannot map"
        )));
    }
}

/// Resolve a `Histogram`: bin the numeric `x` column into equal-width buckets over its data
/// range and emit one series of `(bin_center, count)`. The x axis is quantitative (the
/// binned value) and the y axis is the computed count. `config.bins` sets the bucket count
/// (default 20). Errors when x is unbound/missing or no numeric value exists to bin.
fn resolve_histogram(
    spec: &ChartSpec,
    table: &Table,
    mut diags: Vec<Diagnostic>,
) -> Result<ResolvedChart, Vec<Diagnostic>> {
    let (field, _) = match spec.x_field() {
        Some(parts) => parts,
        None => {
            diags.push(Diagnostic::error("histogram spec has no `x` channel to bin"));
            return Err(diags);
        }
    };
    let Some(column) = table.column(field) else {
        diags.push(Diagnostic::error(format!("x column `{field}` not found in data")));
        return Err(diags);
    };
    let values: Vec<f64> = column.cells.iter().filter_map(|c| as_number(c)).collect();
    let bins = spec.config.bins.unwrap_or(DEFAULT_BINS).max(1);
    let points = match bin_counts(&values, bins) {
        Some(points) => points,
        None => {
            diags.push(Diagnostic::error("histogram has no numeric values to bin"));
            return Err(diags);
        }
    };
    if points.iter().all(|&(_, count)| count == 0.0) {
        diags.push(Diagnostic::warning("histogram produced only empty bins"));
    }
    let x_axis = Axis {
        title: spec.config.x_title.clone().unwrap_or_else(|| field.to_string()),
        kind: AxisKind::Quantitative,
        scale: spec.config.x_scale.unwrap_or_default(),
    };
    let y_axis = Axis {
        title: spec.config.y_title.clone().unwrap_or_else(|| "count".to_string()),
        kind: AxisKind::Quantitative,
        scale: spec.config.y_scale.unwrap_or_default(),
    };
    Ok(ResolvedChart {
        mark: spec.mark,
        series: vec![Series { name: String::new(), points, sizes: Vec::new() }],
        slices: Vec::new(),
        table: None,
        x_axis,
        y_axis,
        config: spec.config.clone(),
    })
}

/// Bin `values` into `bins` equal-width buckets over `[min, max]`, returning one
/// `(bin_center, count)` per bucket. `None` when there are no values. A zero-width range
/// (all values equal) collapses to a single populated bin centered on that value.
fn bin_counts(values: &[f64], bins: usize) -> Option<Vec<(f64, f64)>> {
    let min = values.iter().copied().fold(f64::INFINITY, f64::min);
    let max = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    if !min.is_finite() || !max.is_finite() {
        return None;
    }
    if (max - min).abs() < f64::EPSILON {
        return Some(vec![(min, values.len() as f64)]);
    }
    let width = (max - min) / bins as f64;
    let mut counts = vec![0.0_f64; bins];
    for &v in values {
        let mut idx = ((v - min) / width) as usize;
        if idx >= bins {
            idx = bins - 1; // the maximum value lands in the last bucket
        }
        counts[idx] += 1.0;
    }
    Some(
        counts
            .into_iter()
            .enumerate()
            .map(|(i, count)| (width.mul_add(i as f64 + 0.5, min), count))
            .collect(),
    )
}

/// Resolve an `Arc`: group rows by the `color`/category field, sum the `theta` field per
/// group, and emit one `Slice` per group (in first-seen order) with a stable color index.
/// `series` and the axes are left empty/unused. Errors when theta or color is unbound, the
/// columns are missing, or no positive-magnitude slice survives.
fn resolve_arc(
    spec: &ChartSpec,
    table: &Table,
    mut diags: Vec<Diagnostic>,
) -> Result<ResolvedChart, Vec<Diagnostic>> {
    let slices = match build_slices(spec, table, &mut diags) {
        Some(slices) => slices,
        None => return Err(diags),
    };
    Ok(ResolvedChart {
        mark: spec.mark,
        series: Vec::new(),
        slices,
        table: None,
        x_axis: empty_axis(),
        y_axis: empty_axis(),
        config: spec.config.clone(),
    })
}

/// An unused placeholder axis for radial marks (no cartesian plane).
fn empty_axis() -> Axis {
    Axis { title: String::new(), kind: AxisKind::Quantitative, scale: Scale::default() }
}

/// Group by the color category and sum theta per group into `Slice`s. Returns `None` (with
/// an error pushed) when theta or color is unbound, a column is missing, or nothing draws.
fn build_slices(
    spec: &ChartSpec,
    table: &Table,
    diags: &mut Vec<Diagnostic>,
) -> Option<Vec<Slice>> {
    let (theta_field, _) = match spec.theta_field() {
        Some(parts) => parts,
        None => {
            diags.push(Diagnostic::error("arc spec has no `theta` channel"));
            return None;
        }
    };
    let (color_field, _) = match spec.color_field() {
        Some(parts) => parts,
        None => {
            diags.push(Diagnostic::error("arc spec has no `color` channel to group slices"));
            return None;
        }
    };
    let theta_col = column_or_err(table, theta_field, "theta", diags)?;
    let color_col = column_or_err(table, color_field, "color", diags)?;
    let slices = aggregate_slices(&color_col.cells, &theta_col.cells);
    if slices.is_empty() {
        diags.push(Diagnostic::error("arc has no positive slices after aggregation"));
        return None;
    }
    Some(slices)
}

/// Sum theta per color category in first-seen order, dropping non-positive totals so a wedge
/// is always drawable. Each surviving group gets a sequential `color_index`.
fn aggregate_slices(color_cells: &[String], theta_cells: &[String]) -> Vec<Slice> {
    let mut labels: Vec<String> = Vec::new();
    let mut totals: Vec<f64> = Vec::new();
    for (i, label) in color_cells.iter().enumerate() {
        let Some(v) = as_number(theta_cells.get(i).map_or("", String::as_str)) else { continue };
        let slot = labels.iter().position(|l| l == label).unwrap_or_else(|| {
            labels.push(label.clone());
            totals.push(0.0);
            labels.len() - 1
        });
        totals[slot] += v;
    }
    labels
        .into_iter()
        .zip(totals)
        .filter(|&(_, value)| value > 0.0)
        .enumerate()
        .map(|(color_index, (label, value))| Slice { label, value, color_index })
        .collect()
}

/// Look up a column by name, pushing an error diagnostic and returning `None` if absent.
fn column_or_err<'a>(
    table: &'a Table,
    field: &str,
    role: &str,
    diags: &mut Vec<Diagnostic>,
) -> Option<&'a crate::data::Column> {
    match table.column(field) {
        Some(col) => Some(col),
        None => {
            diags.push(Diagnostic::error(format!("{role} column `{field}` not found in data")));
            None
        }
    }
}

/// Derive a default y-axis title: the single y field name, or "value" for multi.
fn y_axis_title(spec: &ChartSpec) -> String {
    let fields = spec.y_fields();
    if fields.len() == 1 {
        fields[0].0.to_string()
    } else {
        "value".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::resolve;
    use crate::backend::AxisKind;
    use crate::data::Table;
    use crate::dsl::ChartSpec;

    fn chart(spec_yaml: &str, csv: &[u8]) -> crate::backend::ResolvedChart {
        let spec = ChartSpec::from_yaml(spec_yaml).unwrap();
        let table = Table::from_csv(csv).unwrap();
        resolve(&spec, &table).unwrap()
    }

    #[test]
    fn table_defaults_to_all_columns_and_formats_cells() {
        let c = chart(
            "mark: table\n",
            b"day,revenue\n2021-01-01,1000\n2021-02-01,1400.5\n",
        );
        let t = c.table.expect("table mark resolves a TableView");
        assert_eq!(t.headers, vec!["day", "revenue"]);
        // Dates are formatted via the temporal inference; numbers normalized.
        assert_eq!(t.rows, vec![
            vec!["2021-01-01".to_string(), "1000".to_string()],
            vec!["2021-02-01".to_string(), "1400.5".to_string()],
        ]);
        assert!(!t.transpose);
        assert!(c.series.is_empty() && c.slices.is_empty());
    }

    #[test]
    fn table_selects_and_orders_columns_dropping_unknowns() {
        let c = chart(
            "mark: table\ncolumns: [revenue, month, nope]\nconfig:\n  transpose: true\n",
            b"month,revenue,profit\njan,100,20\nfeb,140,35\n",
        );
        let t = c.table.expect("table view");
        assert_eq!(t.headers, vec!["revenue", "month"]);
        assert_eq!(t.rows[0], vec!["100".to_string(), "jan".to_string()]);
        assert!(t.transpose);
    }

    #[test]
    fn table_with_no_known_columns_errors() {
        let spec = ChartSpec::from_yaml("mark: table\ncolumns: [missing]\n").unwrap();
        let table = Table::from_csv(b"month,revenue\njan,100\n").unwrap();
        assert!(resolve(&spec, &table).is_err());
    }

    #[test]
    fn wide_and_long_produce_identical_series() {
        let wide = chart(
            "mark: line\nx: month\ny: [revenue, profit]\n",
            b"month,revenue,profit\njan,100,20\nfeb,140,35\n",
        );
        let long = chart(
            "mark: line\nx: month\ny: value\ncolor: metric\n",
            b"month,metric,value\njan,revenue,100\njan,profit,20\nfeb,revenue,140\nfeb,profit,35\n",
        );
        assert_eq!(wide.series, long.series);
        assert_eq!(wide.series.len(), 2);
        assert_eq!(wide.series[0].name, "revenue");
        assert_eq!(wide.series[0].points, vec![(0.0, 100.0), (1.0, 140.0)]);
    }

    #[test]
    fn single_series_resolves() {
        let c = chart(
            "mark: bar\nx: month\ny: revenue\n",
            b"month,revenue\njan,100\nfeb,140\n",
        );
        assert_eq!(c.series.len(), 1);
        assert_eq!(c.series[0].name, "revenue");
        assert!(matches!(c.x_axis.kind, AxisKind::Categorical(_)));
    }

    #[test]
    fn categorical_x_indexes_stably() {
        let c = chart(
            "mark: line\nx: month\ny: v\n",
            b"month,v\njan,1\nfeb,2\njan,3\n",
        );
        match &c.x_axis.kind {
            AxisKind::Categorical(labels) => assert_eq!(labels, &["jan", "feb"]),
            other => panic!("expected categorical, got {other:?}"),
        }
        assert_eq!(c.series[0].points, vec![(0.0, 1.0), (1.0, 2.0), (0.0, 3.0)]);
    }

    #[test]
    fn temporal_x_is_epoch_seconds() {
        let c = chart(
            "mark: line\nx: { field: d, type: temporal }\ny: v\n",
            b"d,v\n1970-01-01,5\n1970-01-02,6\n",
        );
        assert!(matches!(c.x_axis.kind, AxisKind::Temporal));
        assert_eq!(c.series[0].points, vec![(0.0, 5.0), (86_400.0, 6.0)]);
    }

    #[test]
    fn missing_column_is_a_diagnostic() {
        let spec = ChartSpec::from_yaml("mark: bar\nx: nope\ny: v\n").unwrap();
        let table = Table::from_csv(b"month,v\njan,1\n").unwrap();
        let errs = resolve(&spec, &table).unwrap_err();
        assert!(errs.iter().any(|d| d.message.contains("nope")));
    }

    #[test]
    fn rows_with_missing_values_are_dropped() {
        let c = chart(
            "mark: line\nx: month\ny: v\n",
            b"month,v\njan,1\nfeb,\nmar,3\n",
        );
        assert_eq!(c.series[0].points, vec![(0.0, 1.0), (2.0, 3.0)]);
    }

    /// Histogram bins the x column into equal-width buckets; y axis is titled "count".
    #[test]
    fn histogram_bins_x_into_counts() {
        let c = chart(
            "mark: histogram\nx: v\nconfig:\n  bins: 2\n",
            b"v\n0\n1\n2\n3\n4\n",
        );
        assert_eq!(c.series.len(), 1);
        let counts: Vec<f64> = c.series[0].points.iter().map(|&(_, n)| n).collect();
        // range 0..4, 2 bins of width 2: [0,2) gets 0,1; the max 4 lands in the last bin.
        assert_eq!(counts.iter().sum::<f64>(), 5.0);
        assert_eq!(counts.len(), 2);
        assert_eq!(c.y_axis.title, "count");
        assert!(c.slices.is_empty());
    }

    /// Histogram bin centers sit at the middle of each equal-width bucket.
    #[test]
    fn histogram_bin_centers_are_midpoints() {
        let c = chart(
            "mark: histogram\nx: v\nconfig:\n  bins: 2\n",
            b"v\n0\n10\n",
        );
        let centers: Vec<f64> = c.series[0].points.iter().map(|&(x, _)| x).collect();
        assert_eq!(centers, vec![2.5, 7.5]);
    }

    /// Arc groups by color and sums theta per group into slices; series stays empty.
    #[test]
    fn arc_aggregates_slices_by_color() {
        let c = chart(
            "mark: arc\ntheta: amt\ncolor: region\n",
            b"region,amt\nnorth,10\nsouth,5\nnorth,15\n",
        );
        assert!(c.series.is_empty());
        assert_eq!(c.slices.len(), 2);
        assert_eq!(c.slices[0].label, "north");
        assert_eq!(c.slices[0].value, 25.0);
        assert_eq!(c.slices[0].color_index, 0);
        assert_eq!(c.slices[1].label, "south");
        assert_eq!(c.slices[1].value, 5.0);
        assert_eq!(c.slices[1].color_index, 1);
    }

    /// Arc without a theta channel is a hard error.
    #[test]
    fn arc_missing_theta_errors() {
        let spec = ChartSpec::from_yaml("mark: arc\ncolor: region\n").unwrap();
        let table = Table::from_csv(b"region,amt\nnorth,10\n").unwrap();
        let errs = resolve(&spec, &table).unwrap_err();
        assert!(errs.iter().any(|d| d.message.contains("theta")));
    }

    /// A bound size channel fills the (single) series' sizes for a point mark.
    #[test]
    fn size_channel_populates_sizes_for_points() {
        let c = chart(
            "mark: point\nx: a\ny: b\nsize: pop\n",
            b"a,b,pop\n1,2,100\n3,4,200\n",
        );
        assert_eq!(c.series[0].sizes, vec![100.0, 200.0]);
    }

    /// A size channel on a mark that does not accept it leaves sizes empty and warns.
    #[test]
    fn size_on_bar_is_ignored_with_warning() {
        let spec = ChartSpec::from_yaml("mark: bar\nx: a\ny: b\nsize: pop\n").unwrap();
        let table = Table::from_csv(b"a,b,pop\n1,2,100\n").unwrap();
        let c = resolve(&spec, &table).unwrap();
        assert!(c.series[0].sizes.is_empty());
    }

    /// Config axis scales are copied onto the resolved axes.
    #[test]
    fn config_scales_copied_into_axes() {
        let c = chart(
            "mark: line\nx: a\ny: b\nconfig:\n  y_scale:\n    kind: log\n",
            b"a,b\n1,10\n2,100\n",
        );
        assert_eq!(c.y_axis.scale.kind, crate::dsl::ScaleKind::Log);
        assert_eq!(c.x_axis.scale.kind, crate::dsl::ScaleKind::Linear);
    }
}
