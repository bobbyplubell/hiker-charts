//! Type inference and self-contained string-to-typed coercion.
//!
//! Numbers parse via std; dates parse via our own ISO parser returning epoch
//! seconds — no chrono and no system locale (SPEC §3.4, §4.6). Inference sniffs a
//! whole column: all-numeric => quantitative, all-date => temporal, else nominal.
//! Ordinal is only ever declared, never inferred.

use crate::dsl::DataType;

/// A typed scalar after coercion. Temporal is epoch seconds (UTC). An unparseable
/// or empty cell becomes `Missing`, which the resolver drops with a diagnostic.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Number(f64),
    Time(i64),
    Category(String),
    Missing,
}

/// Infer a column's type by sniffing all non-empty cells. Returns `Quantitative`
/// if every non-empty cell parses as a number, `Temporal` if every one parses as
/// a supported date, otherwise `Nominal`. An all-empty column is `Nominal`.
pub fn infer_type(cells: &[String]) -> DataType {
    let non_empty: Vec<&str> = cells
        .iter()
        .map(|c| c.trim())
        .filter(|c| !c.is_empty())
        .collect();
    if non_empty.is_empty() {
        return DataType::Nominal;
    }
    if non_empty.iter().all(|c| c.parse::<f64>().is_ok()) {
        return DataType::Quantitative;
    }
    if non_empty.iter().all(|c| parse_date(c).is_some()) {
        return DataType::Temporal;
    }
    DataType::Nominal
}

/// Coerce one cell to the declared/inferred type. An empty or unparseable cell
/// yields `Value::Missing`; the caller records a diagnostic. Ordinal and Nominal
/// both coerce to `Category` (ordinal ordering is the resolver's concern).
pub fn coerce(cell: &str, ty: DataType) -> Value {
    let trimmed = cell.trim();
    if trimmed.is_empty() {
        return Value::Missing;
    }
    match ty {
        DataType::Quantitative => trimmed
            .parse::<f64>()
            .map_or(Value::Missing, Value::Number),
        DataType::Temporal => parse_date(trimmed).map_or(Value::Missing, Value::Time),
        DataType::Ordinal | DataType::Nominal => Value::Category(trimmed.to_string()),
    }
}

/// Parse a supported ISO date into epoch seconds (UTC, no timezone handling).
/// Accepts `YYYY-MM-DD`, `YYYY-MM`, `YYYY/MM/DD`, and `YYYY-MM-DDTHH:MM:SS`.
/// Returns `None` for any other shape or an out-of-range field.
pub fn parse_date(s: &str) -> Option<i64> {
    let s = s.trim();
    if let Some((date, time)) = s.split_once('T') {
        let (y, mo, d) = parse_ymd(date, '-')?;
        let secs = parse_hms(time)?;
        return Some(days_from_civil(y, mo, d) * 86_400 + secs);
    }
    if s.contains('/') {
        let (y, mo, d) = parse_ymd(s, '/')?;
        return Some(days_from_civil(y, mo, d) * 86_400);
    }
    let dashes = s.bytes().filter(|&b| b == b'-').count();
    if dashes == 1 {
        let (y, mo) = s.split_once('-')?;
        let year = parse_year(y)?;
        let month = parse_int(mo, 2)?;
        if !(1..=12).contains(&month) {
            return None;
        }
        return Some(days_from_civil(year, month as u32, 1) * 86_400);
    }
    let (y, mo, d) = parse_ymd(s, '-')?;
    Some(days_from_civil(y, mo, d) * 86_400)
}

/// Parse a `YYYY<sep>MM<sep>DD` date into validated year/month/day components.
fn parse_ymd(s: &str, sep: char) -> Option<(i64, u32, u32)> {
    let mut parts = s.split(sep);
    let year = parse_year(parts.next()?)?;
    let month = parse_int(parts.next()?, 2)? as u32;
    let day = parse_int(parts.next()?, 2)? as u32;
    if parts.next().is_some() || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    Some((year, month, day))
}

/// Parse `HH:MM:SS` into a second-of-day count, validating each field's range.
fn parse_hms(s: &str) -> Option<i64> {
    let mut parts = s.split(':');
    let h = parse_int(parts.next()?, 2)?;
    let m = parse_int(parts.next()?, 2)?;
    let sec = parse_int(parts.next()?, 2)?;
    if parts.next().is_some() || !(0..24).contains(&h) || !(0..60).contains(&m) || !(0..60).contains(&sec) {
        return None;
    }
    Some(h * 3600 + m * 60 + sec)
}

/// Parse a calendar year, which must be exactly four ASCII digits (`YYYY`).
fn parse_year(s: &str) -> Option<i64> {
    if s.len() != 4 || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse::<i64>().ok()
}

/// Parse a fixed-width-ish positive integer field, rejecting anything longer than
/// `max_len` digits or containing non-digit characters.
fn parse_int(s: &str, max_len: usize) -> Option<i64> {
    if s.is_empty() || s.len() > max_len || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse::<i64>().ok()
}

/// Days from the Unix epoch (1970-01-01) to the given civil date, via Howard
/// Hinnant's branch-free algorithm. Works for any proleptic Gregorian date.
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let m = i64::from(m);
    let d = i64::from(d);
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::{coerce, days_from_civil, infer_type, parse_date, Value};
    use crate::dsl::DataType;

    fn strings(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn infers_quantitative_temporal_nominal() {
        assert_eq!(infer_type(&strings(&["1", "2.5", "-3"])), DataType::Quantitative);
        assert_eq!(infer_type(&strings(&["2020-01-01", "2021-06"])), DataType::Temporal);
        assert_eq!(infer_type(&strings(&["jan", "feb"])), DataType::Nominal);
        assert_eq!(infer_type(&strings(&["", ""])), DataType::Nominal);
    }

    #[test]
    fn inference_ignores_empty_cells() {
        assert_eq!(infer_type(&strings(&["1", "", "2"])), DataType::Quantitative);
    }

    #[test]
    fn coerce_number_date_and_garbage() {
        assert_eq!(coerce("42.5", DataType::Quantitative), Value::Number(42.5));
        assert_eq!(coerce("not a num", DataType::Quantitative), Value::Missing);
        assert_eq!(coerce("", DataType::Quantitative), Value::Missing);
        assert_eq!(coerce("nope", DataType::Temporal), Value::Missing);
        assert_eq!(
            coerce("region", DataType::Nominal),
            Value::Category("region".to_string())
        );
    }

    #[test]
    fn epoch_anchor_and_known_dates() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(parse_date("1970-01-01"), Some(0));
        assert_eq!(parse_date("1970-01-02"), Some(86_400));
        // 2000-01-01 is 10957 days after the epoch.
        assert_eq!(parse_date("2000-01-01"), Some(10_957 * 86_400));
    }

    #[test]
    fn supported_date_formats() {
        let day = parse_date("2021-03-15").unwrap();
        assert_eq!(parse_date("2021/03/15"), Some(day));
        assert_eq!(parse_date("2021-03-15T00:00:00"), Some(day));
        assert_eq!(parse_date("2021-03-15T01:00:00"), Some(day + 3600));
        assert_eq!(parse_date("2021-03"), Some(parse_date("2021-03-01").unwrap()));
    }

    #[test]
    fn rejects_malformed_dates() {
        assert_eq!(parse_date("2021-13-01"), None);
        assert_eq!(parse_date("2021-00-01"), None);
        assert_eq!(parse_date("2021-01-32"), None);
        assert_eq!(parse_date("2021-01-01T25:00:00"), None);
        assert_eq!(parse_date("garbage"), None);
        assert_eq!(parse_date("21-1-1"), None);
    }
}
