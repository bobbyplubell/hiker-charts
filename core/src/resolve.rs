//! Lower a `ChartSpec` plus a `Table` into a renderer-neutral `ResolvedChart`.
//!
//! This is where wide (`y: [a, b]`) and long (`y: v` + `color: c`) data collapse to
//! the same `Vec<Series>` (SPEC §2.3.1), so a backend never sees the original shape.
//! The x channel becomes `f64` (quantitative value, temporal epoch seconds, or a
//! stable categorical index), rows with a missing x or y are dropped with a
//! diagnostic, and axis kinds plus label maps are recorded for tick formatting.

use crate::backend::{Axis, AxisKind, ResolvedChart, Series};
use crate::data::Table;
use crate::diag::Diagnostic;
use crate::dsl::{ChartSpec, DataType};
use crate::typing::{coerce, infer_type, Value};

/// Resolve a spec and table to a renderer-neutral chart, or a list of errors.
/// Errors are returned when a required channel is absent or a referenced column is
/// missing; dropped rows yield warnings on the returned chart's behalf (logged via
/// the error path only when they leave no drawable data).
pub fn resolve(spec: &ChartSpec, table: &Table) -> Result<ResolvedChart, Vec<Diagnostic>> {
    let mut diags = Vec::new();
    let x = match resolve_x(spec, table, &mut diags) {
        Some(x) => x,
        None => return Err(diags),
    };
    let series = match build_series(spec, table, &x, &mut diags) {
        Some(series) => series,
        None => return Err(diags),
    };
    let x_axis = Axis {
        title: spec.config.x_title.clone().unwrap_or_else(|| x.field.clone()),
        kind: x.kind,
    };
    let y_axis = Axis {
        title: spec.config.y_title.clone().unwrap_or_else(|| y_axis_title(spec)),
        kind: AxisKind::Quantitative,
    };
    Ok(ResolvedChart {
        mark: spec.mark,
        series,
        x_axis,
        y_axis,
        config: spec.config.clone(),
    })
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
            .map(|(name, points)| Series { name, points })
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
}
