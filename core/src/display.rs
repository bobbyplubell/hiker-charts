//! Self-contained number and date formatting, shared by the backend's axis ticks
//! and the GUI's parsed-data view (SPEC §4.6: no chrono, no system locale).
//!
//! These were originally private to the plotters backend; they live here so the
//! single source of truth for "how a value renders" is the core crate. A column's
//! cells are coerced via `typing::coerce` and then rendered with [`format_value`].

use crate::typing::Value;

/// Format a quantitative value: integers without a decimal point, others trimmed of
/// trailing zeros so labels and cells stay compact.
#[must_use]
pub fn format_number(v: f64) -> String {
    if v.fract() == 0.0 && v.abs() < 1e15 {
        return format!("{}", v as i64);
    }
    let s = format!("{v:.3}");
    let trimmed = s.trim_end_matches('0').trim_end_matches('.');
    trimmed.to_string()
}

/// Format epoch seconds (UTC) back to an ISO date string `YYYY-MM-DD`, the inverse of
/// `typing::parse_date`. Self-contained civil-date math (no chrono); negative/pre-epoch
/// inputs floor toward the past so the conversion stays total.
#[must_use]
pub fn format_epoch(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}-{m:02}-{d:02}")
}

/// Render a coerced [`Value`] for display: numbers and dates via the formatters above,
/// categories verbatim, and a missing/unparseable cell as an em dash.
#[must_use]
pub fn format_value(value: &Value) -> String {
    match value {
        Value::Number(n) => format_number(*n),
        Value::Time(t) => format_epoch(*t),
        Value::Category(s) => s.clone(),
        Value::Missing => "—".to_string(),
    }
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

#[cfg(test)]
mod tests {
    use super::{format_epoch, format_number, format_value};
    use crate::typing::Value;

    #[test]
    fn numbers_are_compact() {
        assert_eq!(format_number(42.0), "42");
        assert_eq!(format_number(42.5), "42.5");
        assert_eq!(format_number(1.250), "1.25");
        assert_eq!(format_number(-3.0), "-3");
    }

    #[test]
    fn epoch_round_trips_known_dates() {
        assert_eq!(format_epoch(0), "1970-01-01");
        assert_eq!(format_epoch(1_609_459_200), "2021-01-01");
    }

    #[test]
    fn value_rendering_covers_each_variant() {
        assert_eq!(format_value(&Value::Number(12.0)), "12");
        assert_eq!(format_value(&Value::Time(0)), "1970-01-01");
        assert_eq!(format_value(&Value::Category("north".to_string())), "north");
        assert_eq!(format_value(&Value::Missing), "—");
    }
}
